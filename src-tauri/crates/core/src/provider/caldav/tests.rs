//! The CalDAV provider against a fake server. The answers in `fixtures/` were recorded from
//! Radicale 3 and Nextcloud 35 while `live::caldav_live` ran; only the run number in the test
//! calendar's address was taken out.

use chrono::TimeZone;
use chrono_tz::Europe::Brussels;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::provider::SeriesRole;

// ---- helpers -------------------------------------------------------------------------------

fn fixture(server: &str, name: &str) -> String {
    let file = format!(
        "{}/src/provider/caldav/fixtures/{server}/{name}.xml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file}: {e}"))
}

fn multistatus(body: impl Into<String>) -> ResponseTemplate {
    ResponseTemplate::new(207).set_body_raw(body.into(), "application/xml; charset=utf-8")
}

fn cx(url: String) -> SourceContext {
    SourceContext {
        source_id: "dav-1".into(),
        display_name: "Account".into(),
        secret: Some(Secret::Caldav {
            url,
            username: "alice".into(),
            password: "secret".into(),
        }),
    }
}

/// `PROPFIND` on `at` with `depth`, answered with `response`.
async fn propfind(server: &MockServer, at: &str, depth: &str, response: ResponseTemplate) {
    Mock::given(method("PROPFIND"))
        .and(path(at))
        .and(header("Depth", depth))
        .respond_with(response)
        .mount(server)
        .await;
}

/// `REPORT` on `at` whose body contains `kind`, answered with `response`.
async fn report(server: &MockServer, at: &str, kind: &str, response: ResponseTemplate) {
    Mock::given(method("REPORT"))
        .and(path(at))
        .and(body_string_contains(kind))
        .respond_with(response)
        .mount(server)
        .await;
}

fn redirect(to: &str) -> ResponseTemplate {
    ResponseTemplate::new(301).insert_header("Location", to)
}

fn reference(server: &MockServer, at: &str) -> CalendarRef {
    CalendarRef {
        remote_id: format!("{}{at}", server.uri()),
        time_zone: None,
    }
}

fn cursor_of(changes: &ChangeSet) -> SyncCursor {
    match &changes.next {
        Next::Done(cursor) => cursor.clone(),
        Next::Page(_) => panic!("a pull has one page"),
    }
}

fn titles(changes: &ChangeSet) -> Vec<&str> {
    let mut titles: Vec<&str> = changes.upserts.iter().map(|e| e.title.as_str()).collect();
    titles.sort();
    titles
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// The remote id of the test series' exception: the occurrence of 9 November, 14:00 Brussels.
fn exception_id(resource: &str) -> String {
    let original = Brussels
        .with_ymd_and_hms(2026, 11, 9, 14, 0, 0)
        .unwrap()
        .timestamp();
    format!("{resource}#{original}")
}

#[test]
fn capabilities_match_as_writable() {
    let provider = CaldavProvider::new().unwrap();
    let provider: &dyn CalendarProvider = &provider;
    assert_eq!(
        provider.capabilities().writable,
        provider.as_writable().is_some()
    );
    assert!(provider.capabilities().incremental_sync);
}

// ---- the address ---------------------------------------------------------------------------

#[test]
fn an_address_without_a_scheme_is_https() {
    assert_eq!(
        server_url(" caldav.one.com ").unwrap().as_str(),
        "https://caldav.one.com/"
    );
}

#[test]
fn plain_http_is_only_for_this_computer() {
    for local in [
        "http://localhost:5232/",
        "http://127.0.0.1/",
        "http://[::1]:8080/",
    ] {
        assert!(server_url(local).is_ok(), "{local}");
    }
    for remote in [
        "http://dav.example.com/",
        "http://192.168.1.5/",
        "ftp://dav.example.com/",
    ] {
        assert!(
            matches!(server_url(remote), Err(ProviderError::Permanent(_))),
            "{remote}"
        );
    }
}

// ---- discovery and the calendar list -------------------------------------------------------

/// Radicale: `/.well-known/caldav` redirects to `/`, whose principal is `/alice/`, which is
/// its own calendar home.
async fn radicale() -> MockServer {
    let server = MockServer::start().await;
    propfind(&server, "/.well-known/caldav", "0", redirect("/")).await;
    propfind(
        &server,
        "/",
        "0",
        multistatus(fixture("radicale", "principal")),
    )
    .await;
    propfind(
        &server,
        "/alice/",
        "0",
        multistatus(fixture("radicale", "home-set")),
    )
    .await;
    propfind(
        &server,
        "/alice/",
        "1",
        multistatus(fixture("radicale", "calendars")),
    )
    .await;
    server
}

/// Nextcloud: `/.well-known/caldav` redirects to a full URL, the principal is under
/// `principals/` and the home under `calendars/`.
async fn nextcloud() -> MockServer {
    let server = MockServer::start().await;
    let dav = format!("{}/remote.php/dav/", server.uri());
    propfind(&server, "/.well-known/caldav", "0", redirect(&dav)).await;
    propfind(
        &server,
        "/remote.php/dav/",
        "0",
        multistatus(fixture("nextcloud", "principal")),
    )
    .await;
    propfind(
        &server,
        "/remote.php/dav/principals/users/alice/",
        "0",
        multistatus(fixture("nextcloud", "home-set")),
    )
    .await;
    propfind(
        &server,
        "/remote.php/dav/calendars/alice/",
        "1",
        multistatus(fixture("nextcloud", "calendars")),
    )
    .await;
    server
}

fn names(calendars: &[RemoteCalendar]) -> Vec<&str> {
    let mut names: Vec<&str> = calendars.iter().map(|c| c.name.as_str()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn radicale_calendars_are_found_from_the_server_address() {
    let server = radicale().await;
    let provider = CaldavProvider::new().unwrap();
    let calendars = provider.list_calendars(&cx(server.uri())).await.unwrap();

    // The task list is left out.
    assert_eq!(names(&calendars), ["Work", "simplecalendar-test"]);
    let work = calendars.iter().find(|c| c.name == "Work").unwrap();
    assert_eq!(work.remote_id, format!("{}/alice/work/", server.uri()));
    assert_eq!(work.color.as_deref(), Some("#ff9500"));
    assert_eq!(work.time_zone.as_deref(), Some("Europe/Brussels"));
    // Read-only until the provider writes (#27), though the server allows writing.
    assert_eq!(work.access, Access::Read);
    assert!(!work.is_primary);
}

#[tokio::test]
async fn nextcloud_calendars_are_found_from_the_server_address() {
    let server = nextcloud().await;
    let provider = CaldavProvider::new().unwrap();
    let calendars = provider.list_calendars(&cx(server.uri())).await.unwrap();

    // The task list, the scheduling inbox and outbox and the trash bin are left out.
    assert_eq!(
        names(&calendars),
        [
            "Contact birthdays",
            "Personal",
            "Work",
            "simplecalendar-test"
        ]
    );
    let personal = calendars.iter().find(|c| c.name == "Personal").unwrap();
    assert_eq!(
        personal.remote_id,
        format!("{}/remote.php/dav/calendars/alice/personal/", server.uri())
    );
    assert_eq!(personal.color.as_deref(), Some("#00679e"));
}

#[tokio::test]
async fn every_request_carries_the_login() {
    let server = MockServer::start().await;
    // "alice:secret" in base64.
    Mock::given(header("Authorization", "Basic YWxpY2U6c2VjcmV0"))
        .respond_with(ResponseTemplate::new(404))
        .expect(2)
        .mount(&server)
        .await;
    let provider = CaldavProvider::new().unwrap();
    let _ = provider.list_calendars(&cx(server.uri())).await;
}

#[tokio::test]
async fn what_discovery_found_is_remembered() {
    let server = radicale().await;
    let provider = CaldavProvider::new().unwrap();
    let cx = cx(server.uri());
    provider.list_calendars(&cx).await.unwrap();
    provider.list_calendars(&cx).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let well_known = requests
        .iter()
        .filter(|r| r.url.path() == "/.well-known/caldav")
        .count();
    assert_eq!(well_known, 1);
}

#[tokio::test]
async fn a_home_that_moved_is_looked_for_again() {
    let server = radicale().await;
    let provider = CaldavProvider::new().unwrap();
    let cx = cx(server.uri());
    // What an earlier discovery found, which the server no longer has.
    provider.found.lock().unwrap().insert(
        cx.source_id.clone(),
        (
            server.uri(),
            Found::Home(Url::parse(&format!("{}/old-home/", server.uri())).unwrap()),
        ),
    );
    let calendars = provider.list_calendars(&cx).await.unwrap();
    assert_eq!(names(&calendars), ["Work", "simplecalendar-test"]);
}

#[tokio::test]
async fn the_address_of_one_calendar_is_that_calendar() {
    let server = MockServer::start().await;
    let one = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
        <d:response><d:href>/dav/cal/123/</d:href><d:propstat><d:prop>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
        <d:displayname>Agenda</d:displayname>
        </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    propfind(&server, "/dav/cal/123/", "0", multistatus(one)).await;
    let provider = CaldavProvider::new().unwrap();
    let calendars = provider
        .list_calendars(&cx(format!("{}/dav/cal/123/", server.uri())))
        .await
        .unwrap();
    assert_eq!(names(&calendars), ["Agenda"]);
    // A path was given, so the well-known address was never tried.
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().all(|r| r.url.path() == "/dav/cal/123/"));
}

#[tokio::test]
async fn a_redirect_to_another_host_is_refused() {
    let server = MockServer::start().await;
    propfind(
        &server,
        "/.well-known/caldav",
        "0",
        redirect("https://elsewhere.example.com/dav/"),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let err = provider
        .list_calendars(&cx(server.uri()))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ProviderError::Permanent(m) if m.contains("elsewhere.example.com")),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_rejected_login_is_auth() {
    let server = MockServer::start().await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let provider = CaldavProvider::new().unwrap();
    let err = provider
        .list_calendars(&cx(server.uri()))
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::Auth);
}

#[tokio::test]
async fn an_address_without_caldav_says_so() {
    let server = MockServer::start().await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let provider = CaldavProvider::new().unwrap();
    let err = provider
        .list_calendars(&cx(server.uri()))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ProviderError::Permanent(m) if m.contains("no CalDAV calendars")),
        "{err:?}"
    );
}

// ---- pulling with a sync-token -------------------------------------------------------------

/// A full pull, nothing changed, then an event added, one deleted and the series' exception
/// removed: what the live test did, replayed from `server`'s recorded answers.
async fn replay(name: &str, collection: &str) {
    let server = MockServer::start().await;
    let provider = CaldavProvider::new().unwrap();
    let cx = cx(server.uri());
    let calendar = reference(&server, collection);

    propfind(
        &server,
        collection,
        "1",
        multistatus(fixture(name, "members")),
    )
    .await;
    report(
        &server,
        collection,
        "calendar-multiget",
        multistatus(fixture(name, "multiget-all")),
    )
    .await;
    let all = provider.pull(&cx, &calendar, None, None).await.unwrap();
    assert!(all.full);
    assert_eq!(titles(&all), ["moved", "single", "weekly"]);
    let weekly = format!("{collection}weekly.ics");
    let master = all.upserts.iter().find(|e| e.title == "weekly").unwrap();
    assert_eq!(master.remote_id, weekly);
    assert!(matches!(master.series, SeriesRole::Master { .. }));
    let moved = all.upserts.iter().find(|e| e.title == "moved").unwrap();
    assert_eq!(moved.remote_id, exception_id(&weekly));
    assert!(master.etag.is_some());
    let cursor = cursor_of(&all);
    assert!(Cursor::load(&cursor).unwrap().token.is_some());

    server.reset().await;
    report(
        &server,
        collection,
        "sync-collection",
        multistatus(fixture(name, "sync-unchanged")),
    )
    .await;
    let same = provider
        .pull(&cx, &calendar, Some(&cursor), None)
        .await
        .unwrap();
    assert!(same.upserts.is_empty() && same.deletions.is_empty() && !same.full);
    let cursor = cursor_of(&same);

    server.reset().await;
    report(
        &server,
        collection,
        "sync-collection",
        multistatus(fixture(name, "sync-changes")),
    )
    .await;
    report(
        &server,
        collection,
        "calendar-multiget",
        multistatus(fixture(name, "multiget-changes")),
    )
    .await;
    let changes = provider
        .pull(&cx, &calendar, Some(&cursor), None)
        .await
        .unwrap();
    assert!(!changes.full);
    assert_eq!(titles(&changes), ["added", "weekly"]);
    assert_eq!(
        sorted(changes.deletions.clone()),
        [format!("{collection}single.ics"), exception_id(&weekly)]
    );
}

#[tokio::test]
async fn radicale_changes_come_from_its_sync_token() {
    replay("radicale", "/alice/simplecalendar-test/").await;
}

#[tokio::test]
async fn nextcloud_changes_come_from_its_sync_token() {
    replay(
        "nextcloud",
        "/remote.php/dav/calendars/alice/simplecalendar-test/",
    )
    .await;
}

// ---- pulling without a sync-token ----------------------------------------------------------

const COLLECTION: &str = "/cal/";

/// A collection's answer to `PROPFIND_VERSIONS`: the collection with `ctag`, then each member
/// with its ETag (`None`: without one).
fn versions_answer(ctag: &str, members: &[(&str, Option<&str>)]) -> String {
    let members: String = members
        .iter()
        .map(|(href, etag)| {
            let etag = etag
                .map(|e| format!("<d:getetag>&quot;{e}&quot;</d:getetag>"))
                .unwrap_or_default();
            format!(
                "<d:response><d:href>{COLLECTION}{href}</d:href><d:propstat><d:prop>\
                 <d:resourcetype/>{etag}</d:prop><d:status>HTTP/1.1 200 OK</d:status>\
                 </d:propstat></d:response>"
            )
        })
        .collect();
    format!(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
        <d:response><d:href>{COLLECTION}</d:href><d:propstat><d:prop>
        <d:resourcetype><d:collection/></d:resourcetype><cs:getctag>{ctag}</cs:getctag>
        </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>{members}
        </d:multistatus>"#
    )
}

/// A multiget answer with one event per resource, titled after it.
fn multiget_answer(resources: &[(&str, &str)]) -> String {
    let responses: String = resources
        .iter()
        .map(|(href, etag)| {
            format!(
                "<d:response><d:href>{COLLECTION}{href}</d:href><d:propstat><d:prop>\
                 <d:getetag>&quot;{etag}&quot;</d:getetag><c:calendar-data>BEGIN:VCALENDAR\n\
                 VERSION:2.0\nPRODID:-//Test//EN\nBEGIN:VEVENT\nUID:{href}\n\
                 DTSTART:20261102T090000Z\nSUMMARY:{href}\nEND:VEVENT\nEND:VCALENDAR\n\
                 </c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>\
                 </d:response>"
            )
        })
        .collect();
    format!(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">{responses}</d:multistatus>"#
    )
}

fn seen(ctag: &str, items: &[(&str, &str, &[i64])]) -> SyncCursor {
    Cursor {
        token: None,
        ctag: Some(ctag.into()),
        items: items
            .iter()
            .map(|(href, etag, exceptions)| {
                (
                    format!("{COLLECTION}{href}"),
                    Item {
                        etag: format!("\"{etag}\""),
                        exceptions: exceptions.to_vec(),
                    },
                )
            })
            .collect(),
    }
    .store()
}

#[tokio::test]
async fn an_unchanged_ctag_ends_the_pull() {
    let server = MockServer::start().await;
    propfind(
        &server,
        COLLECTION,
        "0",
        multistatus(versions_answer("c1", &[])),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let cursor = seen("c1", &[("a.ics", "1", &[])]);
    let changes = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&cursor),
            None,
        )
        .await
        .unwrap();
    assert!(changes.upserts.is_empty() && changes.deletions.is_empty() && !changes.full);
    assert_eq!(cursor_of(&changes), cursor);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_changed_ctag_fetches_only_what_changed() {
    let server = MockServer::start().await;
    let members = [
        ("same.ics", Some("1")),
        ("changed.ics", Some("2")),
        ("new.ics", Some("1")),
    ];
    propfind(
        &server,
        COLLECTION,
        "0",
        multistatus(versions_answer("c2", &[])),
    )
    .await;
    propfind(
        &server,
        COLLECTION,
        "1",
        multistatus(versions_answer("c2", &members)),
    )
    .await;
    report(
        &server,
        COLLECTION,
        "calendar-multiget",
        multistatus(multiget_answer(&[("changed.ics", "2"), ("new.ics", "1")])),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let cursor = seen(
        "c1",
        &[
            ("same.ics", "1", &[]),
            ("changed.ics", "1", &[]),
            ("gone.ics", "1", &[100]),
        ],
    );
    let changes = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&cursor),
            None,
        )
        .await
        .unwrap();

    assert!(!changes.full);
    assert_eq!(titles(&changes), ["changed.ics", "new.ics"]);
    assert_eq!(
        sorted(changes.deletions.clone()),
        ["/cal/gone.ics", "/cal/gone.ics#100"]
    );
    // Only the changed and new resources were asked for.
    let requests = server.received_requests().await.unwrap();
    let multiget = requests
        .iter()
        .find(|r| r.method.as_str() == "REPORT")
        .unwrap();
    let body = String::from_utf8_lossy(&multiget.body);
    assert!(body.contains("/cal/changed.ics") && body.contains("/cal/new.ics"));
    assert!(!body.contains("/cal/same.ics"));

    let next = Cursor::load(&cursor_of(&changes)).unwrap();
    assert_eq!(next.ctag.as_deref(), Some("c2"));
    let hrefs: Vec<&str> = next.items.keys().map(String::as_str).collect();
    assert_eq!(hrefs, ["/cal/changed.ics", "/cal/new.ics", "/cal/same.ics"]);
}

#[tokio::test]
async fn without_any_etag_the_calendar_is_read_whole() {
    let server = MockServer::start().await;
    let members = [("a.ics", None), ("b.ics", None)];
    propfind(
        &server,
        COLLECTION,
        "0",
        multistatus(versions_answer("c2", &[])),
    )
    .await;
    propfind(
        &server,
        COLLECTION,
        "1",
        multistatus(versions_answer("c2", &members)),
    )
    .await;
    report(
        &server,
        COLLECTION,
        "calendar-multiget",
        multistatus(multiget_answer(&[("a.ics", "1"), ("b.ics", "1")])),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let changes = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&seen("c1", &[("a.ics", "1", &[])])),
            None,
        )
        .await
        .unwrap();
    assert!(changes.full);
    assert_eq!(titles(&changes), ["a.ics", "b.ics"]);
}

// ---- sync-token edge cases -----------------------------------------------------------------

fn with_token(token: &str) -> SyncCursor {
    Cursor {
        token: Some(token.into()),
        ctag: None,
        items: BTreeMap::from([(
            "/cal/a.ics".to_owned(),
            Item {
                etag: "\"1\"".into(),
                exceptions: Vec::new(),
            },
        )]),
    }
    .store()
}

#[tokio::test]
async fn a_rejected_sync_token_starts_over() {
    let server = MockServer::start().await;
    let precondition = r#"<d:error xmlns:d="DAV:"><d:valid-sync-token/></d:error>"#;
    report(
        &server,
        COLLECTION,
        "sync-collection",
        ResponseTemplate::new(403).set_body_raw(precondition, "application/xml"),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let err = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&with_token("t1")),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::CursorInvalid);
}

#[tokio::test]
async fn a_server_without_sync_reports_compares_etags_instead() {
    let server = MockServer::start().await;
    report(
        &server,
        COLLECTION,
        "sync-collection",
        ResponseTemplate::new(501),
    )
    .await;
    let members = [("a.ics", Some("1"))];
    propfind(
        &server,
        COLLECTION,
        "1",
        multistatus(versions_answer("c1", &members)),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let changes = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&with_token("t1")),
            None,
        )
        .await
        .unwrap();
    assert!(changes.upserts.is_empty() && changes.deletions.is_empty());
    let next = Cursor::load(&cursor_of(&changes)).unwrap();
    assert_eq!(next.token, None);
    assert_eq!(next.ctag.as_deref(), Some("c1"));
}

#[tokio::test]
async fn a_cut_short_sync_report_is_continued() {
    let server = MockServer::start().await;
    let first = format!(
        r#"<d:multistatus xmlns:d="DAV:">
        <d:response><d:href>{COLLECTION}b.ics</d:href><d:propstat><d:prop>
        <d:getetag>"1"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        <d:response><d:href>{COLLECTION}</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>
        <d:sync-token>t2</d:sync-token></d:multistatus>"#
    );
    let second = format!(
        r#"<d:multistatus xmlns:d="DAV:">
        <d:response><d:href>{COLLECTION}a.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
        <d:sync-token>t3</d:sync-token></d:multistatus>"#
    );
    report(&server, COLLECTION, "<d:sync-token>t1<", multistatus(first)).await;
    report(
        &server,
        COLLECTION,
        "<d:sync-token>t2<",
        multistatus(second),
    )
    .await;
    report(
        &server,
        COLLECTION,
        "calendar-multiget",
        multistatus(multiget_answer(&[("b.ics", "1")])),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let changes = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&with_token("t1")),
            None,
        )
        .await
        .unwrap();
    assert_eq!(titles(&changes), ["b.ics"]);
    assert_eq!(changes.deletions, ["/cal/a.ics"]);
    let next = Cursor::load(&cursor_of(&changes)).unwrap();
    assert_eq!(next.token.as_deref(), Some("t3"));
}

// ---- fetching ------------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_the_multiget_leaves_out_is_gone() {
    let server = MockServer::start().await;
    let changed = format!(
        r#"<d:multistatus xmlns:d="DAV:">
        <d:response><d:href>{COLLECTION}a.ics</d:href><d:propstat><d:prop>
        <d:getetag>"2"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        <d:sync-token>t2</d:sync-token></d:multistatus>"#
    );
    report(&server, COLLECTION, "sync-collection", multistatus(changed)).await;
    // Deleted between the two requests.
    report(
        &server,
        COLLECTION,
        "calendar-multiget",
        multistatus(multiget_answer(&[])),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let changes = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&with_token("t1")),
            None,
        )
        .await
        .unwrap();
    assert!(changes.upserts.is_empty());
    assert_eq!(changes.deletions, ["/cal/a.ics"]);
    assert!(Cursor::load(&cursor_of(&changes)).unwrap().items.is_empty());
}

#[tokio::test]
async fn a_resource_that_does_not_parse_keeps_its_events() {
    let server = MockServer::start().await;
    let changed = format!(
        r#"<d:multistatus xmlns:d="DAV:">
        <d:response><d:href>{COLLECTION}a.ics</d:href><d:propstat><d:prop>
        <d:getetag>"2"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        <d:sync-token>t2</d:sync-token></d:multistatus>"#
    );
    let broken = format!(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
        <d:response><d:href>{COLLECTION}a.ics</d:href><d:propstat><d:prop>
        <d:getetag>"2"</d:getetag><c:calendar-data>not a calendar</c:calendar-data>
        </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
    );
    report(&server, COLLECTION, "sync-collection", multistatus(changed)).await;
    report(
        &server,
        COLLECTION,
        "calendar-multiget",
        multistatus(broken),
    )
    .await;
    let provider = CaldavProvider::new().unwrap();
    let changes = provider
        .pull(
            &cx(server.uri()),
            &reference(&server, COLLECTION),
            Some(&with_token("t1")),
            None,
        )
        .await
        .unwrap();
    assert!(changes.upserts.is_empty() && changes.deletions.is_empty());
    // The old ETag stays, so the next pull tries again.
    let next = Cursor::load(&cursor_of(&changes)).unwrap();
    assert_eq!(next.items["/cal/a.ics"].etag, "\"1\"");
}

#[tokio::test]
async fn a_missing_login_is_permanent() {
    let provider = CaldavProvider::new().unwrap();
    let cx = SourceContext {
        source_id: "dav-1".into(),
        display_name: "Account".into(),
        secret: None,
    };
    assert!(matches!(
        provider.list_calendars(&cx).await,
        Err(ProviderError::Permanent(_))
    ));
}

#[tokio::test]
async fn a_home_over_plain_http_elsewhere_gets_no_password() {
    let server = MockServer::start().await;
    let principal = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
        <d:response><d:href>/</d:href><d:propstat><d:prop>
        <c:calendar-home-set><d:href>http://dav.example.com/home/</d:href></c:calendar-home-set>
        </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    propfind(&server, "/.well-known/caldav", "0", multistatus(principal)).await;
    let provider = CaldavProvider::new().unwrap();
    let err = provider
        .list_calendars(&cx(server.uri()))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ProviderError::Permanent(m) if m.contains("without https")),
        "{err:?}"
    );
}
