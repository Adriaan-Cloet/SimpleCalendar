use std::sync::Arc;

use tempfile::TempDir;

use super::*;
use crate::google::{ClientConfig, Endpoints};
use crate::model::{Occurrence, OccurrenceTime};
use crate::secrets::{MemoryStore, UnavailableReason, account};
use crate::sync::Requests;

const FEED: &str = "https://calendar.example.com/private/abc123token/basic.ics";

// ---- helpers -------------------------------------------------------------------------------

struct Setup {
    service: CalendarService,
    db: Db,
    store: Arc<MemoryStore>,
    requests: Requests,
    events: mpsc::UnboundedReceiver<CoreEvent>,
    _dir: TempDir,
}

fn setup() -> Setup {
    setup_with(None, Endpoints::google())
}

/// A service whose Google sign-in uses `client` and talks to `endpoints`.
fn setup_with(client: Option<ClientConfig>, endpoints: Endpoints) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("test.db")).unwrap();
    let store = Arc::new(MemoryStore::new());
    let secrets = Secrets::new(store.clone());
    let (sync, requests) = SyncHandle::detached();
    let (tx, events) = mpsc::unbounded_channel();
    let google = GoogleAuth::new(db.clone(), secrets.clone(), client, endpoints).unwrap();
    Setup {
        service: CalendarService::new(db.clone(), secrets, sync, tx, Arc::new(google)),
        db,
        store,
        requests,
        events,
        _dir: dir,
    }
}

impl Setup {
    async fn source_rows(&self) -> Vec<Vec<Option<String>>> {
        self.db
            .read(|c| {
                let mut stmt = c.prepare(
                    "SELECT id, kind, display_name, account_label, status, last_error,
                            CAST(created_at AS TEXT), CAST(sync_interval_s AS TEXT)
                     FROM sources",
                )?;
                stmt.query_map([], |r| (0..8).map(|i| r.get(i)).collect())?
                    .collect()
            })
            .await
            .unwrap()
    }

    /// A calendar with one event and `warnings` under `source_id`. Returns the calendar id.
    async fn calendar(&self, source_id: &str, remote_id: &str, synced: i64, warnings: &str) -> i64 {
        let (source_id, remote_id, warnings) = (
            source_id.to_owned(),
            remote_id.to_owned(),
            warnings.to_owned(),
        );
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT INTO calendars (source_id, remote_id, name, access, last_synced_at,
                                            warnings)
                     VALUES (?1, ?2, 'Feed', 'read', ?3, ?4)",
                    params![source_id, remote_id, synced, warnings],
                )?;
                let id = c.last_insert_rowid();
                c.execute(
                    "INSERT INTO events (calendar_id, remote_id, start_utc, end_utc)
                     VALUES (?1, 'e', 0, 0)",
                    [id],
                )?;
                Ok(id)
            })
            .await
            .unwrap()
    }

    async fn count(&self, table: &'static str) -> i64 {
        self.db
            .read(move |c| c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)))
            .await
            .unwrap()
    }
}

// ---- adding --------------------------------------------------------------------------------

#[tokio::test]
async fn the_url_goes_to_the_keychain_and_only_the_host_to_the_database() {
    let s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();

    assert_eq!(source.kind, SourceKind::Ics);
    assert_eq!(source.display_name, "Work");
    assert_eq!(
        source.account_label.as_deref(),
        Some("calendar.example.com")
    );
    assert_eq!(source.state, SyncState::Ok);
    assert_eq!(source.last_synced_at, None);

    let stored = s.store.raw(&account(&source.id)).expect("secret stored");
    assert!(stored.contains(FEED), "{stored}");
    for row in s.source_rows().await {
        for value in row.into_iter().flatten() {
            assert!(
                !value.contains("abc123token"),
                "the URL leaked into SQLite: {value}"
            );
        }
    }
}

#[tokio::test]
async fn without_a_name_the_host_is_the_name() {
    let s = setup();
    let source = s.service.add_ics_source(FEED, "  ").await.unwrap();
    assert_eq!(source.display_name, "calendar.example.com");
}

#[tokio::test]
async fn webcal_and_http_addresses_are_accepted_as_written() {
    let s = setup();
    for url in [
        "webcal://example.com/a.ics",
        "WEBCALS://example.com/b.ics",
        "http://example.com/c",
    ] {
        let source = s.service.add_ics_source(url, "").await.unwrap();
        assert_eq!(
            source.account_label.as_deref(),
            Some("example.com"),
            "{url}"
        );
        assert!(
            s.store.raw(&account(&source.id)).unwrap().contains(url),
            "{url}"
        );
    }
}

#[tokio::test]
async fn an_address_that_is_not_a_feed_url_is_refused_and_nothing_stored() {
    let s = setup();
    for url in [
        "",
        "example.com/feed.ics",
        "ftp://example.com/feed.ics",
        "https://",
        "mailto:a@b.c",
    ] {
        let err = s.service.add_ics_source(url, "x").await.unwrap_err();
        assert!(
            matches!(err, ServiceError::InvalidInput(_)),
            "{url}: {err:?}"
        );
    }
    assert!(s.source_rows().await.is_empty());
}

#[tokio::test]
async fn without_a_keychain_nothing_is_added() {
    let s = setup();
    s.store.set_unavailable(Some(UnavailableReason::Locked));

    let err = s.service.add_ics_source(FEED, "Work").await.unwrap_err();

    assert!(
        matches!(err, ServiceError::SecretsUnavailable(u) if u.reason == UnavailableReason::Locked),
        "{err:?}"
    );
    assert!(s.source_rows().await.is_empty());
}

#[tokio::test]
async fn a_new_source_is_synced_at_once() {
    let mut s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();
    assert_eq!(s.requests.manual(), vec![Some(source.id)]);
}

// ---- listing -------------------------------------------------------------------------------

#[tokio::test]
async fn sources_are_listed_oldest_first_with_their_calendars_state() {
    let s = setup();
    let first = s.service.add_ics_source(FEED, "First").await.unwrap();
    let second = s.service.add_ics_source(FEED, "Second").await.unwrap();
    s.calendar(&second.id, "a", 100, r#"["b warning","a warning"]"#)
        .await;
    s.calendar(&second.id, "b", 200, r#"["a warning"]"#).await;
    let id = second.id.clone();
    s.db.write(move |c| {
        c.execute(
            "UPDATE sources SET status = 'error', last_error = 'HTTP 404',
                                    created_at = created_at + 1
                 WHERE id = ?1",
            [id],
        )?;
        c.execute(
            "INSERT INTO sources (id, kind, display_name, created_at)
                 VALUES ('local', 'local', 'On this computer', 0)",
            [],
        )
    })
    .await
    .unwrap();

    let sources = s.service.list_sources().await.unwrap();

    let names: Vec<_> = sources.iter().map(|s| s.display_name.as_str()).collect();
    assert_eq!(names, vec!["First", "Second"]);
    assert_eq!(sources[0].state, SyncState::Ok);
    assert_eq!(sources[1].state, SyncState::Error);
    assert_eq!(sources[1].last_error.as_deref(), Some("HTTP 404"));
    assert_eq!(sources[1].last_synced_at, Some(200));
    assert_eq!(sources[1].warnings, vec!["a warning", "b warning"]);
    assert_eq!(sources[0].id, first.id);
}

// ---- removing ------------------------------------------------------------------------------

#[tokio::test]
async fn removing_a_source_removes_its_data_and_its_secret() {
    let mut s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();
    let a = s.calendar(&source.id, "a", 0, "[]").await;
    let b = s.calendar(&source.id, "b", 0, "[]").await;

    s.service.remove_source(&source.id).await.unwrap();

    assert!(s.source_rows().await.is_empty());
    assert_eq!(s.count("calendars").await, 0);
    assert_eq!(s.count("events").await, 0);
    assert_eq!(s.store.raw(&account(&source.id)), None);
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![a, b]
        }
    );
}

#[tokio::test]
async fn removing_an_unknown_source_is_not_found() {
    let s = setup();
    let err = s.service.remove_source("nope").await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn without_a_keychain_the_source_is_kept() {
    let s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();
    s.store.set_unavailable(Some(UnavailableReason::Locked));

    let err = s.service.remove_source(&source.id).await.unwrap_err();

    assert!(
        matches!(err, ServiceError::SecretsUnavailable(_)),
        "{err:?}"
    );
    assert_eq!(s.source_rows().await.len(), 1);
}

// ---- calendars -----------------------------------------------------------------------------

#[tokio::test]
async fn calendars_are_listed_per_source_with_their_resolved_colour() {
    let s = setup();
    let first = s.service.add_ics_source(FEED, "First").await.unwrap();
    let second = s.service.add_ics_source(FEED, "Second").await.unwrap();
    let ids = (first.id.clone(), second.id.clone());
    s.db.write(move |c| {
        let (first, second) = ids;
        c.execute(
            "UPDATE sources SET created_at = created_at + 1 WHERE id = ?1",
            [&second],
        )?;
        c.execute(
            "INSERT INTO calendars (source_id, remote_id, name, color, color_override, access,
                                    visible, is_primary)
             VALUES (?1, 'b', 'beta', '#111111', NULL, 'read', 1, 0),
                    (?1, 'a', 'Alpha', '#222222', '#333333', 'write', 0, 0),
                    (?1, 'p', 'Zulu', NULL, NULL, 'read', 1, 1),
                    (?2, 'x', 'Another', '#444444', NULL, 'read', 1, 0)",
            params![second, first],
        )
    })
    .await
    .unwrap();

    let calendars = s.service.list_calendars().await.unwrap();

    let names: Vec<_> = calendars.iter().map(|c| c.name.as_str()).collect();
    // Oldest source first; in a source the primary calendar, then by name.
    assert_eq!(names, vec!["Another", "Zulu", "Alpha", "beta"]);
    let alpha = &calendars[2];
    assert_eq!(alpha.source_id, second.id);
    assert_eq!(alpha.source_kind, SourceKind::Ics);
    assert_eq!(alpha.source_name, "Second");
    assert_eq!(alpha.color.as_deref(), Some("#333333"));
    assert!(alpha.editable);
    assert!(!alpha.visible);
    assert_eq!(calendars[1].color, None);
    assert_eq!(calendars[3].color.as_deref(), Some("#111111"));
    assert!(!calendars[3].editable);
}

fn update(json: serde_json::Value) -> CalendarUpdate {
    serde_json::from_value(json).unwrap()
}

#[tokio::test]
async fn hiding_a_calendar_hides_its_occurrences_and_says_it_changed() {
    let mut s = setup();
    timed_events(&s, &[instant(chrono_tz::UTC, 2026, 10, 20, 9, 0)]).await;
    let id = s.service.list_calendars().await.unwrap()[0].id;
    let week = || {
        s.service
            .list_occurrences("2026-10-19", "2026-10-26", "UTC")
    };
    assert_eq!(week().await.unwrap().len(), 1);
    while s.events.try_recv().is_ok() {}

    let hidden = s
        .service
        .update_calendar(id, update(serde_json::json!({"visible": false})))
        .await
        .unwrap();

    assert!(!hidden.visible);
    assert_eq!(week().await.unwrap(), vec![]);
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![id]
        }
    );

    s.service
        .update_calendar(id, update(serde_json::json!({"visible": true})))
        .await
        .unwrap();
    assert_eq!(week().await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_colour_override_wins_and_null_clears_it() {
    let s = setup();
    timed_events(&s, &[instant(chrono_tz::UTC, 2026, 10, 20, 9, 0)]).await;
    let id = s.service.list_calendars().await.unwrap()[0].id;
    s.db.write(move |c| c.execute("UPDATE calendars SET color = '#123456' WHERE id = ?1", [id]))
        .await
        .unwrap();
    let colour = || async {
        let week = s
            .service
            .list_occurrences("2026-10-19", "2026-10-26", "UTC")
            .await
            .unwrap();
        week[0].color.clone()
    };

    let changed = s
        .service
        .update_calendar(id, update(serde_json::json!({"color_override": "#FF9500"})))
        .await
        .unwrap();
    assert_eq!(changed.color.as_deref(), Some("#ff9500"));
    assert!(changed.visible, "a field that is left out stays as it was");
    assert_eq!(colour().await.as_deref(), Some("#ff9500"));

    let cleared = s
        .service
        .update_calendar(id, update(serde_json::json!({"color_override": null})))
        .await
        .unwrap();
    assert_eq!(cleared.color.as_deref(), Some("#123456"));
    assert_eq!(colour().await.as_deref(), Some("#123456"));
}

#[tokio::test]
async fn a_colour_that_is_not_rrggbb_is_invalid_input() {
    let s = setup();
    timed_events(&s, &[]).await;
    let id = s.service.list_calendars().await.unwrap()[0].id;
    for bad in ["blue", "#fff", "#12345g", "123456", "#1234567", ""] {
        let err = s
            .service
            .update_calendar(id, update(serde_json::json!({"color_override": bad})))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::InvalidInput(_)),
            "{bad}: {err:?}"
        );
    }
}

#[tokio::test]
async fn updating_a_calendar_that_does_not_exist_is_not_found() {
    let mut s = setup();
    let err = s
        .service
        .update_calendar(42, update(serde_json::json!({"visible": false})))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "{err:?}");
    assert!(s.events.try_recv().is_err());
}

// ---- occurrences ---------------------------------------------------------------------------

/// One timed event per instant in `starts` (epoch seconds), each 30 minutes long, titled by
/// its index.
async fn timed_events(s: &Setup, starts: &[i64]) {
    let source = s.service.add_ics_source(FEED, "Feed").await.unwrap();
    let id = s.calendar(&source.id, "cal", 0, "[]").await;
    let starts = starts.to_vec();
    s.db.write(move |c| {
        // The helper's own event lies at 1970; it never shows in these ranges.
        for (i, start) in starts.into_iter().enumerate() {
            c.execute(
                "INSERT INTO events (calendar_id, remote_id, title, start_utc, end_utc, start_tz)
                 VALUES (?1, ?2, ?2, ?3, ?4, 'UTC')",
                params![id, i.to_string(), start, start + 1800],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

fn titles(occurrences: &[Occurrence]) -> Vec<&str> {
    occurrences.iter().map(|o| o.title.as_str()).collect()
}

fn instant(tz: chrono_tz::Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    use chrono::TimeZone;
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .earliest()
        .unwrap()
        .timestamp()
}

#[tokio::test]
async fn the_range_runs_from_midnight_to_midnight_in_the_viewer_zone() {
    use chrono_tz::Europe::Brussels;
    let s = setup();
    // The week of 19 October 2026 ends on the day summer time does, so it is 169 hours long.
    timed_events(
        &s,
        &[
            instant(Brussels, 2026, 10, 18, 23, 0), // 0: ends at midnight, before the week
            instant(Brussels, 2026, 10, 19, 0, 0),  // 1: Monday at midnight
            instant(Brussels, 2026, 10, 25, 23, 30), // 2: the last half hour
            instant(Brussels, 2026, 10, 26, 0, 0),  // 3: the Monday after
        ],
    )
    .await;

    let week = s
        .service
        .list_occurrences("2026-10-19", "2026-10-26", "Europe/Brussels")
        .await
        .unwrap();

    assert_eq!(titles(&week), vec!["1", "2"]);
}

#[tokio::test]
async fn the_same_dates_cover_other_instants_in_another_zone() {
    use chrono::TimeZone;
    use chrono_tz::America::New_York;
    use chrono_tz::Europe::Brussels;
    let s = setup();
    timed_events(
        &s,
        &[
            instant(Brussels, 2026, 10, 19, 3, 0), // 0: Sunday 21:00 in New York
            instant(Brussels, 2026, 10, 26, 3, 0), // 1: Sunday 22:00 in New York
        ],
    )
    .await;

    let week = s
        .service
        .list_occurrences("2026-10-19", "2026-10-26", "America/New_York")
        .await
        .unwrap();

    assert_eq!(titles(&week), vec!["1"]);
    assert_eq!(
        week[0].start,
        OccurrenceTime::Timed(
            New_York
                .with_ymd_and_hms(2026, 10, 25, 22, 0, 0)
                .unwrap()
                .fixed_offset()
        )
    );
}

#[tokio::test]
async fn a_bad_zone_date_or_range_is_invalid_input() {
    let s = setup();
    for (from, to, tz) in [
        ("2026-10-19", "2026-10-26", "Mars/Olympus"),
        ("2026-10-19", "2026-10-26", ""),
        ("19/10/2026", "2026-10-26", "UTC"),
        ("2026-10-19", "2026-10-19", "UTC"),
        ("2026-10-26", "2026-10-19", "UTC"),
        ("2026-01-01", "2028-01-01", "UTC"),
    ] {
        let err = s.service.list_occurrences(from, to, tz).await.unwrap_err();
        assert!(
            matches!(err, ServiceError::InvalidInput(_)),
            "{from} {to} {tz}: {err:?}"
        );
    }
}

#[test]
fn occurrences_serialise_as_the_frontend_contract_says() {
    use chrono::{NaiveDate, TimeZone};
    use chrono_tz::Europe::Brussels;
    let occurrence = Occurrence {
        event_id: 7,
        original_start: 1_792_000_000,
        calendar_id: 3,
        title: "Standup".into(),
        location: None,
        color: Some("#336699".into()),
        start: OccurrenceTime::Timed(
            Brussels
                .with_ymd_and_hms(2026, 10, 20, 9, 0, 0)
                .unwrap()
                .fixed_offset(),
        ),
        end: OccurrenceTime::Date(NaiveDate::from_ymd_opt(2026, 10, 21).unwrap()),
        editable: false,
    };

    assert_eq!(
        serde_json::to_value(occurrence).unwrap(),
        serde_json::json!({
            "event_id": 7,
            "original_start": 1_792_000_000,
            "calendar_id": 3,
            "title": "Standup",
            "location": null,
            "color": "#336699",
            "start": "2026-10-20T09:00:00+02:00",
            "end": "2026-10-21",
            "editable": false
        })
    );
}

// ---- Google sign-in ------------------------------------------------------------------------

mod google_sign_in {
    use reqwest::Url;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const REFRESH: &str = "1//refresh-token";

    /// A fake Google: the token endpoint and the primary calendar of `account`.
    async fn google(account: &str) -> (Setup, MockServer) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.access",
                "expires_in": 3599,
                "refresh_token": REFRESH
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/calendar/v3/users/me/calendarList/primary"))
            .and(header("authorization", "Bearer ya29.access"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": account })),
            )
            .mount(&server)
            .await;
        let base = Url::parse(&server.uri()).unwrap();
        let endpoints = Endpoints {
            authorize: base.join("auth").unwrap(),
            token: base.join("token").unwrap(),
            api: base.join("calendar/v3/").unwrap(),
        };
        let client = ClientConfig {
            client_id: "app.apps.googleusercontent.com".into(),
            client_secret: Some("shh".into()),
        };
        (setup_with(Some(client), endpoints), server)
    }

    /// A browser that follows the consent page straight back to the app with `answer`
    /// (`code=...` or `error=...`).
    fn browser(answer: &'static str) -> impl FnOnce(&str) -> Result<(), String> + Send {
        move |url: &str| {
            let url = Url::parse(url).unwrap();
            let get = |key: &str| {
                url.query_pairs()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.into_owned())
                    .unwrap()
            };
            let back = format!("{}?state={}&{answer}", get("redirect_uri"), get("state"));
            tokio::spawn(async move {
                let http = crate::provider::http::client().unwrap();
                http.get(back).send().await.unwrap();
            });
            Ok(())
        }
    }

    async fn status(s: &Setup, id: &str) -> String {
        let id = id.to_owned();
        s.db.read(move |c| {
            c.query_row("SELECT status FROM sources WHERE id = ?1", [id], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn signing_in_adds_the_account_with_its_refresh_token_only_in_the_keychain() {
        let (mut s, _server) = google("me@example.com").await;

        let source = s
            .service
            .begin_google_sign_in(None, browser("code=4%2Fcode"))
            .await
            .unwrap();

        assert_eq!(source.kind, SourceKind::Google);
        assert_eq!(source.display_name, "me@example.com");
        assert_eq!(source.account_label.as_deref(), Some("me@example.com"));
        assert_eq!(source.state, SyncState::Ok);
        let stored = s.store.raw(&account(&source.id)).expect("secret stored");
        assert!(stored.contains(REFRESH), "{stored}");
        assert!(
            !stored.contains("ya29"),
            "the access token stays in memory: {stored}"
        );
        for row in s.source_rows().await {
            for value in row.into_iter().flatten() {
                assert!(!value.contains("refresh-token"), "token in SQLite: {value}");
            }
        }
        assert_eq!(s.requests.manual(), vec![Some(source.id)]);
    }

    #[tokio::test]
    async fn signing_in_to_the_same_account_again_reconnects_it() {
        let (s, _server) = google("me@example.com").await;
        let first = s
            .service
            .begin_google_sign_in(None, browser("code=c"))
            .await
            .unwrap();
        let id = first.id.clone();
        s.db.write(move |c| {
            c.execute(
                "UPDATE sources SET status = 'auth_required', last_error = 'x' WHERE id = ?1",
                [id],
            )
        })
        .await
        .unwrap();

        // Once from "Sign in again" on the source, once as if adding it anew.
        let again = s
            .service
            .begin_google_sign_in(Some(&first.id), browser("code=c"))
            .await
            .unwrap();
        let added = s
            .service
            .begin_google_sign_in(None, browser("code=c"))
            .await
            .unwrap();

        assert_eq!(again.id, first.id);
        assert_eq!(added.id, first.id);
        assert_eq!(status(&s, &first.id).await, "ok");
        assert_eq!(again.last_error, None);
        assert_eq!(s.source_rows().await.len(), 1);
    }

    #[tokio::test]
    async fn reconnecting_as_another_account_is_refused() {
        let (s, _server) = google("someone.else@example.com").await;
        let id = "11111111-2222-3333-4444-555555555555".to_owned();
        let row = id.clone();
        s.db.write(move |c| {
            c.execute(
                "INSERT INTO sources (id, kind, display_name, account_label, status, created_at)
                 VALUES (?1, 'google', 'me', 'me@example.com', 'auth_required', 0)",
                [row],
            )
        })
        .await
        .unwrap();

        let err = s
            .service
            .begin_google_sign_in(Some(&id), browser("code=c"))
            .await
            .unwrap_err();

        assert!(
            matches!(&err, ServiceError::SignIn(m) if m.contains("me@example.com")),
            "{err:?}"
        );
        assert_eq!(status(&s, &id).await, "auth_required");
        assert_eq!(s.store.raw(&account(&id)), None);
    }

    #[tokio::test]
    async fn a_refusal_in_the_browser_adds_nothing() {
        let (s, _server) = google("me@example.com").await;
        let err = s
            .service
            .begin_google_sign_in(None, browser("error=access_denied"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::SignIn(_)), "{err:?}");
        assert_eq!(s.count("sources").await, 0);
    }

    #[tokio::test]
    async fn a_waiting_sign_in_can_be_cancelled() {
        let (s, _server) = google("me@example.com").await;
        let service = s.service.clone();
        let (opened_tx, opened) = tokio::sync::oneshot::channel();
        let waiting = tokio::spawn(async move {
            service
                .begin_google_sign_in(None, move |_: &str| {
                    // The user never finishes in the browser.
                    let _ = opened_tx.send(());
                    Ok(())
                })
                .await
        });
        opened.await.unwrap();

        s.service.cancel_google_sign_in();

        assert!(matches!(
            waiting.await.unwrap(),
            Err(ServiceError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn without_a_client_sign_in_asks_for_one_and_opens_nothing() {
        let s = setup();
        let err = s
            .service
            .begin_google_sign_in(None, |_: &str| panic!("no browser without a client"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::GoogleClientMissing), "{err:?}");
    }

    #[tokio::test]
    async fn the_users_own_client_goes_to_settings_and_its_secret_to_the_keychain() {
        let s = setup();
        assert_eq!(
            s.service.google_client().await.unwrap(),
            GoogleClientInfo {
                custom_client_id: None,
                built_in: false
            }
        );

        let info = s
            .service
            .set_google_client(" mine.apps.googleusercontent.com ", "my-secret")
            .await
            .unwrap();

        assert_eq!(
            info.custom_client_id.as_deref(),
            Some("mine.apps.googleusercontent.com")
        );
        let stored = s.store.raw("app:google-client-secret").unwrap();
        assert!(stored.contains("my-secret"), "{stored}");

        let cleared = s.service.set_google_client("", "").await.unwrap();
        assert_eq!(cleared.custom_client_id, None);
        assert_eq!(s.store.raw("app:google-client-secret"), None);
    }

    #[tokio::test]
    async fn a_client_id_must_look_like_one() {
        let s = setup();
        for bad in [
            "mine",
            "a b.apps.googleusercontent.com",
            "https://example.com",
        ] {
            let err = s.service.set_google_client(bad, "").await.unwrap_err();
            assert!(
                matches!(err, ServiceError::InvalidInput(_)),
                "{bad}: {err:?}"
            );
        }
    }
}

// ---- errors for the frontend ---------------------------------------------------------------

#[test]
fn errors_serialise_with_a_kind_and_a_message() {
    let json = |e: ServiceError| serde_json::to_value(e).unwrap();
    assert_eq!(
        json(ServiceError::InvalidInput("Enter an address.".into())),
        serde_json::json!({"kind": "invalid_input", "message": "Enter an address."})
    );
    assert_eq!(
        json(ServiceError::NotFound(SOURCE_GONE))["kind"],
        "not_found"
    );
    assert_eq!(
        json(ServiceError::GoogleClientMissing)["kind"],
        "google_client_missing"
    );
    assert_eq!(json(ServiceError::SignIn("x".into()))["kind"], "sign_in");
    assert_eq!(json(ServiceError::Cancelled)["kind"], "cancelled");
    let locked = json(ServiceError::SecretsUnavailable(SecretsUnavailable {
        reason: UnavailableReason::Locked,
    }));
    assert_eq!(locked["kind"], "secrets_unavailable");
    assert_eq!(locked["reason"], "locked");
}
