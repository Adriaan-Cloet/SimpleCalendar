//! Writing against a fake server: which requests go out, with which conditions, and what
//! becomes of the answers.

use chrono::TimeZone;
use chrono_tz::Europe::Brussels;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::super::{Discovered, Found};
use super::*;
use crate::provider::{Field, Reminders, SeriesRole};
use crate::secrets::Secret;

const CALENDAR_PATH: &str = "/alice/work/";

fn cx(server: &MockServer) -> SourceContext {
    SourceContext {
        source_id: "dav-1".into(),
        display_name: "Account".into(),
        secret: Some(Secret::Caldav {
            url: server.uri(),
            username: "alice".into(),
            password: "secret".into(),
        }),
    }
}

fn calendar(server: &MockServer) -> CalendarRef {
    CalendarRef {
        remote_id: format!("{}{CALENDAR_PATH}", server.uri()),
        time_zone: None,
    }
}

/// A provider that found the account already, as after the calendar list, with the user's
/// address.
fn provider(server: &MockServer) -> CaldavProvider {
    let provider = CaldavProvider::new().unwrap();
    provider.found.lock().unwrap().insert(
        "dav-1".into(),
        Discovered {
            given: server.uri(),
            found: Found::Home(Url::parse(&format!("{}/alice/", server.uri())).unwrap()),
            addresses: vec!["alice@example.com".into()],
        },
    );
    provider
}

fn brussels(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Brussels
        .with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .unwrap()
        .timestamp()
}

fn resource(body: &str) -> String {
    let lines: Vec<&str> = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Other//EN"]
        .into_iter()
        .chain(body.lines().map(str::trim).filter(|l| !l.is_empty()))
        .chain(["END:VCALENDAR"])
        .collect();
    lines.join("\r\n") + "\r\n"
}

/// A weekly series with an exception on 9 November and a line from another app.
fn series() -> String {
    resource(
        "BEGIN:VEVENT\nUID:weekly\nDTSTAMP:20261001T080000Z
         DTSTART;TZID=Europe/Brussels:20261102T140000\nDTEND;TZID=Europe/Brussels:20261102T150000
         RRULE:FREQ=WEEKLY\nSUMMARY:Weekly\nX-OTHER-APP:keep me\nEND:VEVENT
         BEGIN:VEVENT\nUID:weekly\nDTSTAMP:20261001T080000Z
         RECURRENCE-ID;TZID=Europe/Brussels:20261109T140000
         DTSTART;TZID=Europe/Brussels:20261109T160000\nDTEND;TZID=Europe/Brussels:20261109T170000
         SUMMARY:Moved\nEND:VEVENT",
    )
}

const SERIES: &str = "/alice/work/weekly.ics";

fn calendar_data(body: String, etag: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("ETag", etag)
        .set_body_raw(body, "text/calendar; charset=utf-8")
}

/// `GET` on `at`, answered with `body` and `etag`.
async fn get(server: &MockServer, at: &str, body: String, etag: &str) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(calendar_data(body, etag))
        .mount(server)
        .await;
}

/// Every request with `method`, in order.
async fn sent(server: &MockServer, verb: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == verb)
        .collect()
}

fn body(request: &Request) -> String {
    String::from_utf8(request.body.clone()).unwrap()
}

fn header_of(request: &Request, name: &str) -> Option<String> {
    request
        .headers
        .get(name)
        .map(|v| v.to_str().unwrap().to_owned())
}

fn data(title: &str) -> EventData {
    EventData {
        title: title.into(),
        description: None,
        location: None,
        all_day: false,
        start_utc: brussels(2026, 11, 2, 14, 0),
        end_utc: brussels(2026, 11, 2, 15, 0),
        start_tz: Some("Europe/Brussels".into()),
        end_tz: Some("Europe/Brussels".into()),
        recurrence: None,
        reminders: Reminders {
            use_default: false,
            overrides: vec![],
        },
    }
}

fn title_change(remote_id: &str, title: &str, base: EventData) -> EventUpdate {
    EventUpdate {
        remote_id: remote_id.into(),
        desired: EventData {
            title: title.into(),
            ..base
        },
        changed: [Field::Title].into_iter().collect(),
    }
}

fn series_data() -> EventData {
    EventData {
        recurrence: Some("RRULE:FREQ=WEEKLY".into()),
        ..data("Weekly")
    }
}

// ---- remote ids ----------------------------------------------------------------------------

#[test]
fn ids_name_the_resource_and_the_occurrence() {
    assert_eq!(
        crate::provider::caldav::new_event_id("https://dav.example.com/alice/work/", "abc"),
        "/alice/work/abc.ics"
    );
    assert_eq!(
        crate::provider::caldav::new_event_id("https://dav.example.com/alice/work", "abc"),
        "/alice/work/abc.ics"
    );
    let occurrence = crate::provider::caldav::occurrence_id(SERIES, 1_794_000_000);
    assert_eq!(split(&occurrence), (SERIES, Some(1_794_000_000)));
    assert_eq!(split(SERIES), (SERIES, None));
    assert_eq!(uid_of("/alice/work/abc.ics"), "abc");
}

// ---- creating ------------------------------------------------------------------------------

#[tokio::test]
async fn a_new_event_is_put_where_nothing_is() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/alice/work/abc.ics"))
        .and(header("If-None-Match", "*"))
        .respond_with(ResponseTemplate::new(201).insert_header("ETag", "\"e1\""))
        .mount(&server)
        .await;
    let created = provider(&server)
        .create_event(
            &cx(&server),
            &calendar(&server),
            "/alice/work/abc.ics",
            &data("Planning"),
        )
        .await
        .unwrap();
    assert_eq!(created.remote_id, "/alice/work/abc.ics");
    assert_eq!(created.ical_uid.as_deref(), Some("abc"));
    assert_eq!(created.etag.as_deref(), Some("\"e1\""));
    assert_eq!(EventData::from(&created), data("Planning"));

    let put = &sent(&server, "PUT").await[0];
    assert_eq!(
        header_of(put, "Content-Type").as_deref(),
        Some("text/calendar; charset=utf-8")
    );
    assert!(body(put).contains("UID:abc\r\n"));
}

#[tokio::test]
async fn a_new_event_the_server_changed_is_fetched_for_its_etag() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    let stored = resource(
        "BEGIN:VEVENT\nUID:abc\nDTSTART;TZID=Europe/Brussels:20261102T140000
         DTEND;TZID=Europe/Brussels:20261102T150000\nSUMMARY:Planning\nEND:VEVENT",
    );
    get(&server, "/alice/work/abc.ics", stored, "\"e2\"").await;
    let created = provider(&server)
        .create_event(
            &cx(&server),
            &calendar(&server),
            "/alice/work/abc.ics",
            &data("Planning"),
        )
        .await
        .unwrap();
    assert_eq!(created.etag.as_deref(), Some("\"e2\""));
}

#[tokio::test]
async fn a_create_that_got_through_before_is_a_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(412))
        .mount(&server)
        .await;
    let earlier = ical::create("abc", &data("Planning"), 0);
    get(&server, "/alice/work/abc.ics", earlier, "\"e1\"").await;
    let created = provider(&server)
        .create_event(
            &cx(&server),
            &calendar(&server),
            "/alice/work/abc.ics",
            &data("Planning"),
        )
        .await
        .unwrap();
    assert_eq!(created.title, "Planning");
    assert_eq!(created.etag.as_deref(), Some("\"e1\""));
}

#[tokio::test]
async fn a_new_event_without_a_path_is_refused() {
    let server = MockServer::start().await;
    let result = provider(&server)
        .create_event(&cx(&server), &calendar(&server), "abc", &data("Planning"))
        .await;
    assert!(
        matches!(result, Err(ProviderError::Permanent(_))),
        "{result:?}"
    );
    assert!(sent(&server, "PUT").await.is_empty());
}

// ---- changing ------------------------------------------------------------------------------

#[tokio::test]
async fn a_change_puts_the_whole_object_back_over_its_etag() {
    let server = MockServer::start().await;
    get(&server, SERIES, series(), "\"e1\"").await;
    Mock::given(method("PUT"))
        .and(path(SERIES))
        .and(header("If-Match", "\"e1\""))
        .respond_with(ResponseTemplate::new(204).insert_header("ETag", "\"e2\""))
        .mount(&server)
        .await;
    let event = provider(&server)
        .update_event(
            &cx(&server),
            &calendar(&server),
            &title_change(SERIES, "Weekly sync", series_data()),
            Some(&Etag("\"e1\"".into())),
        )
        .await
        .unwrap();
    assert_eq!(event.title, "Weekly sync");
    assert_eq!(event.etag.as_deref(), Some("\"e2\""));
    assert!(matches!(event.series, SeriesRole::Master { .. }));

    let put = body(&sent(&server, "PUT").await[0]);
    assert!(put.contains("SUMMARY:Weekly sync\r\n"));
    assert!(put.contains("X-OTHER-APP:keep me\r\n"));
    assert!(put.contains("SUMMARY:Moved\r\n"));
}

#[tokio::test]
async fn a_change_to_an_object_the_server_changed_is_a_conflict() {
    let server = MockServer::start().await;
    get(&server, SERIES, series(), "\"e9\"").await;
    let result = provider(&server)
        .update_event(
            &cx(&server),
            &calendar(&server),
            &title_change(SERIES, "Weekly sync", series_data()),
            Some(&Etag("\"e1\"".into())),
        )
        .await;
    assert!(matches!(result, Err(ProviderError::Conflict)), "{result:?}");
    assert!(sent(&server, "PUT").await.is_empty());
}

#[tokio::test]
async fn a_change_to_an_event_that_is_gone_is_not_found() {
    let server = MockServer::start().await;
    let result = provider(&server)
        .update_event(
            &cx(&server),
            &calendar(&server),
            &title_change(SERIES, "Weekly sync", series_data()),
            Some(&Etag("\"e1\"".into())),
        )
        .await;
    assert!(matches!(result, Err(ProviderError::NotFound)), "{result:?}");
}

#[tokio::test]
async fn an_object_that_changes_while_it_is_written_is_fetched_again() {
    let server = MockServer::start().await;
    get(&server, SERIES, series(), "\"e1\"").await;
    // Someone else wrote between the fetch and the put, once.
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(412))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(204).insert_header("ETag", "\"e2\""))
        .mount(&server)
        .await;
    let occurrence = crate::provider::caldav::occurrence_id(SERIES, brussels(2026, 11, 16, 14, 0));
    // A first change to an occurrence has no ETag of its own.
    let event = provider(&server)
        .update_event(
            &cx(&server),
            &calendar(&server),
            &title_change(&occurrence, "Just this one", data("Weekly")),
            None,
        )
        .await
        .unwrap();
    assert_eq!(event.remote_id, occurrence);
    assert_eq!(event.title, "Just this one");
    assert_eq!(sent(&server, "GET").await.len(), 2);
}

#[tokio::test]
async fn an_etag_this_app_replaced_itself_is_no_conflict() {
    let server = MockServer::start().await;
    // The object as it was, then after the first write.
    Mock::given(method("GET"))
        .respond_with(calendar_data(series(), "\"e1\""))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(calendar_data(series(), "\"e2\""))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(header("If-Match", "\"e1\""))
        .respond_with(ResponseTemplate::new(204).insert_header("ETag", "\"e2\""))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(header("If-Match", "\"e2\""))
        .respond_with(ResponseTemplate::new(204).insert_header("ETag", "\"e3\""))
        .mount(&server)
        .await;
    let provider = provider(&server);
    let occurrence = crate::provider::caldav::occurrence_id(SERIES, brussels(2026, 11, 9, 14, 0));
    provider
        .update_event(
            &cx(&server),
            &calendar(&server),
            &title_change(&occurrence, "Moved again", data("Moved")),
            Some(&Etag("\"e1\"".into())),
        )
        .await
        .unwrap();
    // The series was stored with "e1" too: the edit was based on what the server had.
    let series = provider
        .update_event(
            &cx(&server),
            &calendar(&server),
            &title_change(SERIES, "Weekly sync", series_data()),
            Some(&Etag("\"e1\"".into())),
        )
        .await
        .unwrap();
    assert_eq!(series.etag.as_deref(), Some("\"e3\""));
}

#[tokio::test]
async fn an_invitation_the_user_organizes_comes_back_editable() {
    let server = MockServer::start().await;
    let invitation = resource(
        "BEGIN:VEVENT\nUID:inv\nDTSTART:20261102T130000Z\nDTEND:20261102T140000Z\nSUMMARY:Call
         ORGANIZER:mailto:Alice@example.com\nATTENDEE:mailto:bob@example.com\nEND:VEVENT",
    );
    get(&server, "/alice/work/inv.ics", invitation, "\"e1\"").await;
    let event = provider(&server)
        .get_event(&cx(&server), &calendar(&server), "/alice/work/inv.ics")
        .await
        .unwrap()
        .unwrap();
    assert!(event.is_organizer);
}

// ---- deleting ------------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_series_deletes_its_resource_over_its_etag() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(SERIES))
        .and(header("If-Match", "\"e1\""))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    provider(&server)
        .delete_event(
            &cx(&server),
            &calendar(&server),
            SERIES,
            Some(&Etag("\"e1\"".into())),
        )
        .await
        .unwrap();
    assert_eq!(sent(&server, "DELETE").await.len(), 1);
}

#[tokio::test]
async fn a_delete_answers_by_status() {
    for (status, expected) in [
        (404, "ok"),
        (410, "ok"),
        (412, "conflict"),
        (500, "transient"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        let result = provider(&server)
            .delete_event(
                &cx(&server),
                &calendar(&server),
                SERIES,
                Some(&Etag("\"e1\"".into())),
            )
            .await;
        let got = match result {
            Ok(()) => "ok",
            Err(ProviderError::Conflict) => "conflict",
            Err(ProviderError::Transient { .. }) => "transient",
            Err(other) => panic!("{status}: {other:?}"),
        };
        assert_eq!(got, expected, "HTTP {status}");
    }
}

#[tokio::test]
async fn a_deleted_occurrence_is_excluded_from_its_series() {
    let server = MockServer::start().await;
    get(&server, SERIES, series(), "\"e5\"").await;
    Mock::given(method("PUT"))
        .and(header("If-Match", "\"e5\""))
        .respond_with(ResponseTemplate::new(204).insert_header("ETag", "\"e6\""))
        .mount(&server)
        .await;
    let occurrence = crate::provider::caldav::occurrence_id(SERIES, brussels(2026, 11, 9, 14, 0));
    // Based on an older ETag: an occurrence is deleted whatever else changed.
    provider(&server)
        .delete_event(
            &cx(&server),
            &calendar(&server),
            &occurrence,
            Some(&Etag("\"e1\"".into())),
        )
        .await
        .unwrap();
    let put = body(&sent(&server, "PUT").await[0]);
    assert!(put.contains("EXDATE;TZID=Europe/Brussels:20261109T140000\r\n"));
    assert!(!put.contains("RECURRENCE-ID"));
}

#[tokio::test]
async fn an_occurrence_already_gone_needs_no_write() {
    let server = MockServer::start().await;
    let excluded = series().replace(
        "RRULE:FREQ=WEEKLY\r\n",
        "RRULE:FREQ=WEEKLY\r\nEXDATE;TZID=Europe/Brussels:20261116T140000\r\n",
    );
    get(&server, SERIES, excluded, "\"e5\"").await;
    let provider = provider(&server);
    let occurrence = crate::provider::caldav::occurrence_id(SERIES, brussels(2026, 11, 16, 14, 0));
    provider
        .delete_event(&cx(&server), &calendar(&server), &occurrence, None)
        .await
        .unwrap();
    // Nor does one of a series that is gone.
    let gone = crate::provider::caldav::occurrence_id("/alice/work/gone.ics", 0);
    provider
        .delete_event(&cx(&server), &calendar(&server), &gone, None)
        .await
        .unwrap();
    assert!(sent(&server, "PUT").await.is_empty());
}

// ---- reading one event ---------------------------------------------------------------------

#[tokio::test]
async fn one_event_is_read_from_its_resource() {
    let server = MockServer::start().await;
    get(&server, SERIES, series(), "\"e1\"").await;
    let provider = provider(&server);
    let exception = crate::provider::caldav::occurrence_id(SERIES, brussels(2026, 11, 9, 14, 0));
    let event = provider
        .get_event(&cx(&server), &calendar(&server), &exception)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.title, "Moved");
    assert_eq!(event.etag.as_deref(), Some("\"e1\""));
    // An occurrence that is no exception has no event of its own; a resource that is gone
    // has none at all.
    let plain = crate::provider::caldav::occurrence_id(SERIES, brussels(2026, 11, 16, 14, 0));
    for id in [plain.as_str(), "/alice/work/gone.ics"] {
        let none = provider
            .get_event(&cx(&server), &calendar(&server), id)
            .await
            .unwrap();
        assert!(none.is_none(), "{id}");
    }
}
