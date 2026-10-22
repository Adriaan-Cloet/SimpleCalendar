use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone;
use reqwest::Url;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::db::Db;
use crate::google::{ClientConfig, Endpoints};
use crate::model::{Access, EventStatus};
use crate::provider::{
    EventData, Field, Reminder, ReminderMethod, Reminders, RemoteEvent, SeriesRole,
};
use crate::secrets::{MemoryStore, Secret, Secrets};

const EVENTS: &str = "/calendar/v3/calendars/primary@example.com/events";

struct Setup {
    provider: GoogleProvider,
    server: MockServer,
    _dir: TempDir,
}

/// A provider talking to a fake Google whose token endpoint always hands out `ya29.token`.
async fn setup() -> Setup {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "ya29.token",
            "expires_in": 3599
        })))
        .named("token")
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("test.db")).unwrap();
    let secrets = Secrets::new(Arc::new(MemoryStore::new()));
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
    let auth = GoogleAuth::new(db, secrets, Some(client), endpoints).unwrap();
    Setup {
        provider: GoogleProvider::new(Arc::new(auth)),
        server,
        _dir: dir,
    }
}

fn cx() -> SourceContext {
    SourceContext {
        source_id: "src-g".into(),
        display_name: "me@example.com".into(),
        secret: Some(Secret::GoogleRefreshToken {
            refresh_token: "1//refresh".into(),
        }),
    }
}

fn calendar() -> CalendarRef {
    CalendarRef {
        remote_id: "primary@example.com".into(),
        time_zone: Some("Europe/Brussels".into()),
    }
}

fn events_page(items: Value, next: Value) -> ResponseTemplate {
    let mut body = json!({ "kind": "calendar#events", "items": items });
    for (key, value) in next.as_object().unwrap() {
        body[key] = value.clone();
    }
    ResponseTemplate::new(200).set_body_json(body)
}

/// The changes of one page with `items`, from a first sync.
async fn changes_for(items: Value) -> ChangeSet {
    let s = setup().await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .respond_with(events_page(items, json!({ "nextSyncToken": "t1" })))
        .mount(&s.server)
        .await;
    s.provider
        .pull(&cx(), &calendar(), None, None)
        .await
        .unwrap()
}

fn brussels(y: i32, mo: u32, d: u32, h: u32) -> i64 {
    chrono_tz::Europe::Brussels
        .with_ymd_and_hms(y, mo, d, h, 0, 0)
        .unwrap()
        .timestamp()
}

fn midnight_utc(y: i32, mo: u32, d: u32) -> i64 {
    chrono::Utc
        .with_ymd_and_hms(y, mo, d, 0, 0, 0)
        .unwrap()
        .timestamp()
}

// ---- calendar list -------------------------------------------------------------------------

#[tokio::test]
async fn calendars_come_from_every_page_of_the_calendar_list() {
    let s = setup().await;
    Mock::given(method("GET"))
        .and(path("/calendar/v3/users/me/calendarList"))
        .and(query_param_is_missing("pageToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": "me@example.com",
                "summary": "me@example.com",
                "backgroundColor": "#9fe1e7",
                "timeZone": "Europe/Brussels",
                "accessRole": "owner",
                "primary": true,
                "defaultReminders": [
                    {"method": "popup", "minutes": 10},
                    {"method": "email", "minutes": 60}
                ]
            }, {
                "id": "boss@example.com",
                "summary": "Boss",
                "accessRole": "freeBusyReader"
            }],
            "nextPageToken": "p2"
        })))
        .mount(&s.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/calendar/v3/users/me/calendarList"))
        .and(query_param("pageToken", "p2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": "en.be#holiday@group.v.calendar.google.com",
                "summary": "Holidays in Belgium",
                "summaryOverride": "Feestdagen",
                "backgroundColor": "not a colour",
                "accessRole": "reader"
            }]
        })))
        .mount(&s.server)
        .await;

    let calendars = s.provider.list_calendars(&cx()).await.unwrap();

    assert_eq!(
        calendars,
        vec![
            RemoteCalendar {
                remote_id: "me@example.com".into(),
                name: "me@example.com".into(),
                color: Some("#9fe1e7".into()),
                time_zone: Some("Europe/Brussels".into()),
                // An owner may write, and since #18 this provider does.
                access: Access::Write,
                is_primary: true,
                default_reminders: vec![10],
            },
            RemoteCalendar {
                remote_id: "en.be#holiday@group.v.calendar.google.com".into(),
                name: "Feestdagen".into(),
                color: None,
                time_zone: None,
                access: Access::Read,
                is_primary: false,
                default_reminders: vec![],
            },
        ]
    );
}

#[test]
fn owners_and_writers_may_write_once_the_provider_does() {
    let entry = |role: &str| -> json::CalendarEntry {
        serde_json::from_value(json!({"id": "c", "accessRole": role})).unwrap()
    };
    let access = |role: &str, can_write| entry(role).into_remote(can_write).map(|c| c.access);
    assert_eq!(access("owner", true), Some(Access::Write));
    assert_eq!(access("writer", true), Some(Access::Write));
    assert_eq!(access("reader", true), Some(Access::Read));
    assert_eq!(access("freeBusyReader", true), None);
    assert_eq!(access("owner", false), Some(Access::Read));
}

// ---- paging and sync tokens ----------------------------------------------------------------

#[tokio::test]
async fn a_first_sync_pages_through_unexpanded_events_until_the_sync_token() {
    let s = setup().await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .and(query_param("singleEvents", "false"))
        .and(query_param("maxResults", "250"))
        .and(query_param_is_missing("syncToken"))
        .and(query_param_is_missing("pageToken"))
        .respond_with(events_page(json!([]), json!({ "nextPageToken": "p2" })))
        .expect(1)
        .mount(&s.server)
        .await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .and(query_param("singleEvents", "false"))
        .and(query_param("pageToken", "p2"))
        .respond_with(events_page(json!([]), json!({ "nextSyncToken": "t1" })))
        .expect(1)
        .mount(&s.server)
        .await;

    let first = s
        .provider
        .pull(&cx(), &calendar(), None, None)
        .await
        .unwrap();
    assert!(first.full);
    assert_eq!(first.next, Next::Page(PageToken("p2".into())));

    let PageToken(token) = match first.next {
        Next::Page(p) => p,
        Next::Done(_) => unreachable!(),
    };
    let last = s
        .provider
        .pull(&cx(), &calendar(), None, Some(&PageToken(token)))
        .await
        .unwrap();
    assert!(last.full);
    assert_eq!(last.next, Next::Done(SyncCursor("t1".into())));
}

#[tokio::test]
async fn a_later_sync_sends_the_token_with_the_same_parameters() {
    let s = setup().await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .and(query_param("singleEvents", "false"))
        .and(query_param("maxResults", "250"))
        .and(query_param("syncToken", "t1"))
        .respond_with(events_page(json!([]), json!({ "nextSyncToken": "t2" })))
        .expect(1)
        .mount(&s.server)
        .await;

    let changes = s
        .provider
        .pull(&cx(), &calendar(), Some(&SyncCursor("t1".into())), None)
        .await
        .unwrap();

    assert!(!changes.full);
    assert_eq!(changes.next, Next::Done(SyncCursor("t2".into())));
}

#[tokio::test]
async fn calendar_ids_are_escaped_in_the_path() {
    let s = setup().await;
    Mock::given(method("GET"))
        .respond_with(events_page(json!([]), json!({ "nextSyncToken": "t" })))
        .mount(&s.server)
        .await;
    let holidays = CalendarRef {
        remote_id: "en.be#holiday@group.v.calendar.google.com".into(),
        time_zone: None,
    };

    s.provider.pull(&cx(), &holidays, None, None).await.unwrap();

    let requests = s.server.received_requests().await.unwrap();
    let get = requests
        .iter()
        .find(|r| r.method.as_str() == "GET")
        .unwrap();
    assert_eq!(
        get.url.path(),
        "/calendar/v3/calendars/en.be%23holiday@group.v.calendar.google.com/events"
    );
}

// ---- errors --------------------------------------------------------------------------------

async fn error_for(response: ResponseTemplate) -> ProviderError {
    let s = setup().await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .respond_with(response)
        .mount(&s.server)
        .await;
    s.provider
        .pull(&cx(), &calendar(), Some(&SyncCursor("t1".into())), None)
        .await
        .unwrap_err()
}

fn google_error(status: u16, reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "error": {
            "code": status,
            "message": "Something about the request.",
            "errors": [{ "reason": reason }]
        }
    }))
}

#[tokio::test]
async fn an_expired_sync_token_asks_for_a_full_pull() {
    assert_eq!(
        error_for(google_error(410, "fullSyncRequired")).await,
        ProviderError::CursorInvalid
    );
}

#[tokio::test]
async fn rate_limits_and_server_errors_are_retried_later() {
    assert_eq!(
        error_for(google_error(403, "rateLimitExceeded")).await,
        ProviderError::Transient { retry_after: None }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(503).insert_header("retry-after", "30")).await,
        ProviderError::Transient {
            retry_after: Some(Duration::from_secs(30))
        }
    );
    assert_eq!(
        error_for(ResponseTemplate::new(429)).await,
        ProviderError::Transient { retry_after: None }
    );
}

#[tokio::test]
async fn other_refusals_are_permanent_with_googles_message() {
    assert_eq!(
        error_for(google_error(403, "forbidden")).await,
        ProviderError::Permanent("Google refused the request: Something about the request.".into())
    );
    assert_eq!(
        error_for(ResponseTemplate::new(404)).await,
        ProviderError::NotFound
    );
}

#[tokio::test]
async fn a_rejected_access_token_is_refreshed_once() {
    let s = setup().await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .mount(&s.server)
        .await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .respond_with(events_page(json!([]), json!({ "nextSyncToken": "t" })))
        .mount(&s.server)
        .await;

    s.provider
        .pull(&cx(), &calendar(), None, None)
        .await
        .unwrap();

    let refreshes = s
        .server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/token")
        .count();
    assert_eq!(refreshes, 2);
}

#[tokio::test]
async fn two_rejections_in_a_row_need_a_new_sign_in() {
    assert_eq!(
        error_for(ResponseTemplate::new(401)).await,
        ProviderError::Auth
    );
}

// ---- events --------------------------------------------------------------------------------

fn only_upsert(changes: ChangeSet) -> RemoteEvent {
    assert!(changes.warnings.is_empty(), "{:?}", changes.warnings);
    assert_eq!(changes.upserts.len(), 1, "{:?}", changes.upserts);
    changes.upserts.into_iter().next().unwrap()
}

#[tokio::test]
async fn a_timed_event_becomes_a_zoned_event() {
    let event = only_upsert(
        changes_for(json!([{
            "id": "e1",
            "status": "confirmed",
            "etag": "\"3456\"",
            "iCalUID": "e1@google.com",
            "summary": "Standup",
            "description": "Daily",
            "location": "Room 1",
            "colorId": "11",
            "start": {"dateTime": "2026-10-20T09:00:00+02:00", "timeZone": "Europe/Brussels"},
            "end": {"dateTime": "2026-10-20T09:15:00+02:00", "timeZone": "Europe/Brussels"},
            "organizer": {"email": "me@example.com", "self": true},
            "reminders": {
                "useDefault": false,
                "overrides": [{"method": "popup", "minutes": 5}, {"method": "email", "minutes": 60}]
            },
            "updated": "2026-10-19T08:00:00.000Z"
        }]))
        .await,
    );

    assert_eq!(
        event,
        RemoteEvent {
            remote_id: "e1".into(),
            ical_uid: Some("e1@google.com".into()),
            status: EventStatus::Confirmed,
            title: "Standup".into(),
            description: Some("Daily".into()),
            location: Some("Room 1".into()),
            color: Some("#d50000".into()),
            all_day: false,
            start_utc: brussels(2026, 10, 20, 9),
            end_utc: brussels(2026, 10, 20, 9) + 15 * 60,
            start_tz: Some("Europe/Brussels".into()),
            end_tz: Some("Europe/Brussels".into()),
            series: SeriesRole::Single,
            remote_updated_at: Some(
                chrono::Utc
                    .with_ymd_and_hms(2026, 10, 19, 8, 0, 0)
                    .unwrap()
                    .timestamp()
            ),
            etag: Some("\"3456\"".into()),
            is_organizer: true,
            guests_can_modify: false,
            reminders: Reminders {
                use_default: false,
                overrides: vec![
                    Reminder {
                        method: ReminderMethod::Popup,
                        minutes_before: 5
                    },
                    Reminder {
                        method: ReminderMethod::Email,
                        minutes_before: 60
                    },
                ],
            },
        }
    );
}

#[tokio::test]
async fn an_all_day_event_is_stored_as_dates() {
    let event = only_upsert(
        changes_for(json!([{
            "id": "d1",
            "summary": "Holiday",
            "start": {"date": "2026-12-25"},
            "end": {"date": "2026-12-26"}
        }]))
        .await,
    );
    assert!(event.all_day);
    assert_eq!(event.start_utc, midnight_utc(2026, 12, 25));
    assert_eq!(event.end_utc, midnight_utc(2026, 12, 26));
    assert_eq!((event.start_tz, event.end_tz), (None, None));
    // Absent organizer and reminders: not the organizer, and the calendar's defaults.
    assert!(!event.is_organizer);
    assert_eq!(event.reminders, Reminders::default());
}

#[tokio::test]
async fn without_its_own_zone_an_event_takes_the_calendars_then_its_offset() {
    let changes = changes_for(json!([
        {"id": "a", "start": {"dateTime": "2026-10-20T09:00:00+02:00"},
                    "end": {"dateTime": "2026-10-20T10:00:00+02:00"}}
    ]))
    .await;
    assert_eq!(
        changes.upserts[0].start_tz.as_deref(),
        Some("Europe/Brussels")
    );

    let s = setup().await;
    Mock::given(method("GET"))
        .respond_with(events_page(
            json!([{"id": "b", "start": {"dateTime": "2026-10-20T09:00:00+05:30"},
                                 "end": {"dateTime": "2026-10-20T10:00:00+05:30"}}]),
            json!({ "nextSyncToken": "t" }),
        ))
        .mount(&s.server)
        .await;
    let zoneless = CalendarRef {
        remote_id: "primary@example.com".into(),
        time_zone: None,
    };
    let changes = s.provider.pull(&cx(), &zoneless, None, None).await.unwrap();
    assert_eq!(changes.upserts[0].start_tz.as_deref(), Some("+05:30"));
}

#[tokio::test]
async fn masters_and_exceptions_keep_their_series() {
    let changes = changes_for(json!([
        // The exception comes first: Google does not order them (§7.2).
        {
            "id": "m1_20261021T070000Z",
            "recurringEventId": "m1",
            "originalStartTime": {"dateTime": "2026-10-21T09:00:00+02:00", "timeZone": "Europe/Brussels"},
            "start": {"dateTime": "2026-10-21T10:00:00+02:00", "timeZone": "Europe/Brussels"},
            "end": {"dateTime": "2026-10-21T10:15:00+02:00", "timeZone": "Europe/Brussels"}
        },
        {
            "id": "m1",
            "recurrence": ["RRULE:FREQ=DAILY;COUNT=5", "EXDATE;TZID=Europe/Brussels:20261023T090000"],
            "start": {"dateTime": "2026-10-20T09:00:00+02:00", "timeZone": "Europe/Brussels"},
            "end": {"dateTime": "2026-10-20T09:15:00+02:00", "timeZone": "Europe/Brussels"}
        }
    ]))
    .await;

    assert_eq!(
        changes.upserts[0].series,
        SeriesRole::Exception {
            master_remote_id: "m1".into(),
            original_start_utc: brussels(2026, 10, 21, 9),
        }
    );
    assert_eq!(changes.upserts[0].start_utc, brussels(2026, 10, 21, 10));
    assert_eq!(
        changes.upserts[1].series,
        SeriesRole::Master {
            recurrence: "RRULE:FREQ=DAILY;COUNT=5\nEXDATE;TZID=Europe/Brussels:20261023T090000"
                .into()
        }
    );
}

#[tokio::test]
async fn a_cancelled_occurrence_is_an_exception_and_a_cancelled_event_a_deletion() {
    let changes = changes_for(json!([
        // An incremental sync sends cancelled entries with little more than these fields.
        {
            "id": "m1_20261022",
            "status": "cancelled",
            "recurringEventId": "m1",
            "originalStartTime": {"date": "2026-10-22"}
        },
        {"id": "gone", "status": "cancelled"}
    ]))
    .await;

    assert_eq!(changes.deletions, vec!["gone".to_owned()]);
    let cancelled = &changes.upserts[0];
    assert_eq!(cancelled.status, EventStatus::Cancelled);
    assert!(cancelled.all_day);
    assert_eq!(
        cancelled.series,
        SeriesRole::Exception {
            master_remote_id: "m1".into(),
            original_start_utc: midnight_utc(2026, 10, 22),
        }
    );
    assert_eq!(cancelled.start_utc, midnight_utc(2026, 10, 22));
}

#[tokio::test]
async fn locked_and_special_events_are_never_editable() {
    let organised = json!({"self": true});
    let changes = changes_for(json!([
        {"id": "own", "organizer": organised, "guestsCanModify": true,
         "start": {"date": "2026-10-20"}, "end": {"date": "2026-10-21"}},
        {"id": "locked", "organizer": organised, "locked": true,
         "start": {"date": "2026-10-20"}, "end": {"date": "2026-10-21"}},
        {"id": "birthday", "organizer": organised, "eventType": "birthday",
         "start": {"date": "2026-10-20"}, "end": {"date": "2026-10-21"}},
        {"id": "guest", "guestsCanModify": true,
         "start": {"date": "2026-10-20"}, "end": {"date": "2026-10-21"}}
    ]))
    .await;

    let flags: Vec<_> = changes
        .upserts
        .iter()
        .map(|e| (e.remote_id.as_str(), e.is_organizer, e.guests_can_modify))
        .collect();
    assert_eq!(
        flags,
        vec![
            ("own", true, true),
            ("locked", false, false),
            ("birthday", false, false),
            ("guest", false, true),
        ]
    );
}

#[tokio::test]
async fn an_event_that_cannot_be_read_is_skipped_with_a_warning() {
    let changes = changes_for(json!([
        {"id": "broken", "start": {"dateTime": "yesterday"}, "end": {"dateTime": "today"}},
        {"id": "fine", "start": {"date": "2026-10-20"}, "end": {"date": "2026-10-21"}}
    ]))
    .await;
    assert_eq!(changes.upserts.len(), 1);
    assert_eq!(changes.warnings.len(), 1);
    assert!(
        changes.warnings[0].contains("yesterday"),
        "{:?}",
        changes.warnings
    );
}

// ---- writing (§7.5) ------------------------------------------------------------------------

/// The standup on 20 October 2026, 09:00 to 09:15 in Brussels, as the app stores it.
fn standup() -> EventData {
    EventData {
        title: "Standup".into(),
        description: Some("Daily".into()),
        location: None,
        all_day: false,
        start_utc: brussels(2026, 10, 20, 9),
        end_utc: brussels(2026, 10, 20, 9) + 15 * 60,
        start_tz: Some("Europe/Brussels".into()),
        end_tz: Some("Europe/Brussels".into()),
        recurrence: None,
        reminders: Reminders {
            use_default: false,
            overrides: vec![Reminder {
                method: ReminderMethod::Popup,
                minutes_before: 10,
            }],
        },
    }
}

/// What Google answers to a write: the event with a new ETag.
fn answer(id: &str, etag: &str, summary: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": id,
        "status": "confirmed",
        "etag": etag,
        "summary": summary,
        "start": {"dateTime": "2026-10-20T09:00:00+02:00", "timeZone": "Europe/Brussels"},
        "end": {"dateTime": "2026-10-20T09:15:00+02:00", "timeZone": "Europe/Brussels"},
        "organizer": {"self": true},
    }))
}

/// The body of the only API request with `method` the fake Google received.
async fn sent_body(server: &MockServer, method: &str) -> Value {
    let requests = server.received_requests().await.unwrap();
    let sent: Vec<_> = requests
        .iter()
        .filter(|r| r.method.as_str() == method && r.url.path() != "/token")
        .collect();
    assert_eq!(sent.len(), 1, "{method} requests: {}", sent.len());
    serde_json::from_slice(&sent[0].body).unwrap()
}

fn update(changed: &[Field], desired: EventData) -> EventUpdate {
    EventUpdate {
        remote_id: "e1".into(),
        desired,
        changed: changed.iter().copied().collect(),
    }
}

#[test]
fn only_the_fields_that_differ_have_changed() {
    let base = standup();
    let mut edited = standup();
    assert!(edited.changed_from(&base).is_empty());
    edited.title = "Retro".into();
    edited.end_utc += 15 * 60;
    edited.reminders.overrides.clear();
    assert_eq!(
        edited.changed_from(&base).into_iter().collect::<Vec<_>>(),
        vec![Field::Title, Field::Times, Field::Reminders]
    );
}

#[tokio::test]
async fn a_new_event_is_inserted_whole_with_its_own_id_and_no_email() {
    let s = setup().await;
    Mock::given(method("POST"))
        .and(path(EVENTS))
        .and(query_param("sendUpdates", "none"))
        .respond_with(answer("abc123", "\"e1\"", "Standup"))
        .expect(1)
        .mount(&s.server)
        .await;

    let created = s
        .provider
        .create_event(&cx(), &calendar(), "abc123", &standup())
        .await
        .unwrap();

    assert_eq!(created.remote_id, "abc123");
    assert_eq!(created.etag.as_deref(), Some("\"e1\""));
    assert_eq!(
        sent_body(&s.server, "POST").await,
        json!({
            "id": "abc123",
            "summary": "Standup",
            "description": "Daily",
            "start": {"date": null, "dateTime": "2026-10-20T09:00:00+02:00",
                      "timeZone": "Europe/Brussels"},
            "end": {"date": null, "dateTime": "2026-10-20T09:15:00+02:00",
                    "timeZone": "Europe/Brussels"},
            "reminders": {"useDefault": false, "overrides": [{"method": "popup", "minutes": 10}]},
        })
    );
}

#[tokio::test]
async fn an_insert_whose_answer_was_lost_is_found_on_retry() {
    let s = setup().await;
    // The first attempt got through; Google refuses the same id a second time.
    Mock::given(method("POST"))
        .and(path(EVENTS))
        .respond_with(google_error(409, "duplicate"))
        .mount(&s.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{EVENTS}/abc123")))
        .respond_with(answer("abc123", "\"e1\"", "Standup"))
        .expect(1)
        .mount(&s.server)
        .await;

    let created = s
        .provider
        .create_event(&cx(), &calendar(), "abc123", &standup())
        .await
        .unwrap();
    assert_eq!(created.etag.as_deref(), Some("\"e1\""));
}

#[tokio::test]
async fn a_patch_sends_exactly_the_changed_fields_with_if_match_and_no_email() {
    let s = setup().await;
    Mock::given(method("PATCH"))
        .and(path(format!("{EVENTS}/e1")))
        .and(query_param("sendUpdates", "none"))
        .and(header("If-Match", "\"e1\""))
        .respond_with(answer("e1", "\"e2\"", "Retro"))
        .expect(1)
        .mount(&s.server)
        .await;
    let mut desired = standup();
    desired.title = "Retro".into();
    desired.description = None;

    let updated = s
        .provider
        .update_event(
            &cx(),
            &calendar(),
            &update(&[Field::Title, Field::Description], desired),
            Some(&Etag("\"e1\"".into())),
        )
        .await
        .unwrap();

    assert_eq!(updated.etag.as_deref(), Some("\"e2\""));
    // A cleared text is `null`; nothing the app does not own (attendees, organizer,
    // guestsCanModify) is ever named, so the patch leaves it alone.
    assert_eq!(
        sent_body(&s.server, "PATCH").await,
        json!({"summary": "Retro", "description": null})
    );
}

#[tokio::test]
async fn a_patch_to_all_day_clears_the_time_and_sends_owned_arrays_whole() {
    let s = setup().await;
    Mock::given(method("PATCH"))
        .respond_with(answer("e1", "\"e2\"", "Standup"))
        .mount(&s.server)
        .await;
    let mut desired = standup();
    desired.all_day = true;
    desired.start_utc = midnight_utc(2026, 10, 20);
    desired.end_utc = midnight_utc(2026, 10, 21);
    desired.start_tz = None;
    desired.end_tz = None;
    desired.recurrence = Some("RRULE:FREQ=WEEKLY".into());

    s.provider
        .update_event(
            &cx(),
            &calendar(),
            &update(
                &[Field::Times, Field::Recurrence, Field::Reminders],
                desired,
            ),
            None,
        )
        .await
        .unwrap();

    let body = sent_body(&s.server, "PATCH").await;
    assert_eq!(
        body["start"],
        json!({"date": "2026-10-20", "dateTime": null, "timeZone": null})
    );
    assert_eq!(body["end"]["date"], "2026-10-21");
    assert_eq!(body["recurrence"], json!(["RRULE:FREQ=WEEKLY"]));
    assert_eq!(
        body["reminders"],
        json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": 10}]})
    );
    // Without an ETag, as for an occurrence never changed before, the patch is unconditional.
    let requests = s.server.received_requests().await.unwrap();
    let patch = requests
        .iter()
        .find(|r| r.method.as_str() == "PATCH")
        .unwrap();
    assert!(!patch.headers.contains_key("if-match"));
}

#[tokio::test]
async fn a_stale_etag_is_a_conflict_for_patch_and_delete() {
    let s = setup().await;
    Mock::given(method("PATCH"))
        .respond_with(google_error(412, "conditionNotMet"))
        .mount(&s.server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(google_error(412, "conditionNotMet"))
        .mount(&s.server)
        .await;
    let etag = Etag("\"old\"".into());

    let patched = s
        .provider
        .update_event(
            &cx(),
            &calendar(),
            &update(&[Field::Title], standup()),
            Some(&etag),
        )
        .await;
    let deleted = s
        .provider
        .delete_event(&cx(), &calendar(), "e1", Some(&etag))
        .await;
    assert_eq!(patched.unwrap_err(), ProviderError::Conflict);
    assert_eq!(deleted.unwrap_err(), ProviderError::Conflict);
}

#[tokio::test]
async fn a_delete_carries_if_match_and_an_event_already_gone_is_deleted() {
    let s = setup().await;
    Mock::given(method("DELETE"))
        .and(path(format!("{EVENTS}/e1")))
        .and(query_param("sendUpdates", "none"))
        .and(header("If-Match", "\"e1\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&s.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{EVENTS}/gone")))
        .respond_with(google_error(410, "deleted"))
        .mount(&s.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{EVENTS}/never")))
        .respond_with(google_error(404, "notFound"))
        .mount(&s.server)
        .await;

    let etag = Etag("\"e1\"".into());
    for id in ["e1", "gone", "never"] {
        s.provider
            .delete_event(&cx(), &calendar(), id, Some(&etag))
            .await
            .unwrap();
    }
}

#[test]
fn an_occurrence_is_named_by_its_series_and_original_start() {
    assert_eq!(
        instance_id("abc", brussels(2026, 10, 20, 9), false),
        "abc_20261020T070000Z"
    );
    assert_eq!(
        instance_id("abc", midnight_utc(2026, 12, 25), true),
        "abc_20261225"
    );
}

// ---- through the sync engine ---------------------------------------------------------------

mod engine {
    use tokio::sync::mpsc;

    use super::*;
    use crate::CoreEvent;
    use crate::db::occurrences;
    use crate::model::OccurrenceTime;
    use crate::provider::Providers;
    use crate::sync::{SyncEngine, SyncHandle, SyncState};

    struct Running {
        db: Db,
        handle: SyncHandle,
        events: mpsc::UnboundedReceiver<CoreEvent>,
        server: MockServer,
        _dir: TempDir,
    }

    /// The real engine with a Google provider, a fake Google and one signed-in source `g`.
    async fn running() -> Running {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "ya29.token", "expires_in": 3599
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/calendar/v3/users/me/calendarList"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [{
                "id": "primary@example.com", "summary": "Me", "accessRole": "owner",
                "timeZone": "Europe/Brussels", "primary": true
            }]})))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.db")).unwrap();
        db.write(|c| {
            c.execute(
                "INSERT INTO sources (id, kind, display_name, account_label, created_at)
                 VALUES ('g', 'google', 'me@example.com', 'me@example.com', 0)",
                [],
            )
        })
        .await
        .unwrap();
        let secrets = Secrets::new(Arc::new(MemoryStore::new()));
        secrets
            .set(
                "g",
                &Secret::GoogleRefreshToken {
                    refresh_token: "1//refresh".into(),
                },
            )
            .await
            .unwrap();
        let base = Url::parse(&server.uri()).unwrap();
        let endpoints = Endpoints {
            authorize: base.join("auth").unwrap(),
            token: base.join("token").unwrap(),
            api: base.join("calendar/v3/").unwrap(),
        };
        let client = ClientConfig {
            client_id: "app.apps.googleusercontent.com".into(),
            client_secret: None,
        };
        let auth = GoogleAuth::new(db.clone(), secrets.clone(), Some(client), endpoints).unwrap();
        let providers = Providers::new().with(Arc::new(GoogleProvider::new(Arc::new(auth))));
        let (tx, events) = mpsc::unbounded_channel();
        let (engine, handle) = SyncEngine::new(db.clone(), secrets, providers, tx);
        tokio::spawn(engine.run());
        Running {
            db,
            handle,
            events,
            server,
            _dir: dir,
        }
    }

    impl Running {
        /// Syncs `g` and returns how the cycle ended.
        async fn sync(&mut self) -> (SyncState, Option<String>) {
            self.handle.sync_now(Some("g".into()));
            let wait = async {
                while let Some(event) = self.events.recv().await {
                    if let CoreEvent::SyncStatus(s) = event
                        && s.state != SyncState::Syncing
                    {
                        return (s.state, s.last_error);
                    }
                }
                panic!("the engine stopped");
            };
            tokio::time::timeout(Duration::from_secs(10), wait)
                .await
                .expect("the cycle finished")
        }

        async fn cursor(&self) -> Option<String> {
            self.db
                .read(|c| c.query_row("SELECT sync_cursor FROM calendars", [], |r| r.get(0)))
                .await
                .unwrap()
        }
    }

    /// A full pull in two pages: the moved occurrence on the first, its series on the second.
    async fn full_pull_with_the_exception_first(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path(EVENTS))
            .and(query_param_is_missing("syncToken"))
            .and(query_param_is_missing("pageToken"))
            .respond_with(events_page(
                json!([{
                    "id": "m1_20261021T070000Z",
                    "recurringEventId": "m1",
                    "summary": "Standup",
                    "originalStartTime": {"dateTime": "2026-10-21T09:00:00+02:00",
                                          "timeZone": "Europe/Brussels"},
                    "start": {"dateTime": "2026-10-21T10:00:00+02:00", "timeZone": "Europe/Brussels"},
                    "end": {"dateTime": "2026-10-21T10:15:00+02:00", "timeZone": "Europe/Brussels"}
                }]),
                json!({"nextPageToken": "p2"}),
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(EVENTS))
            .and(query_param_is_missing("syncToken"))
            .and(query_param("pageToken", "p2"))
            .respond_with(events_page(
                json!([{
                    "id": "m1",
                    "summary": "Standup",
                    "recurrence": ["RRULE:FREQ=DAILY;COUNT=3"],
                    "start": {"dateTime": "2026-10-20T09:00:00+02:00", "timeZone": "Europe/Brussels"},
                    "end": {"dateTime": "2026-10-20T09:15:00+02:00", "timeZone": "Europe/Brussels"}
                }]),
                json!({"nextSyncToken": "t1"}),
            ))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn an_exception_on_an_earlier_page_than_its_series_moves_its_occurrence() {
        let mut r = running().await;
        full_pull_with_the_exception_first(&r.server).await;

        assert_eq!(r.sync().await, (SyncState::Ok, None));
        assert_eq!(r.cursor().await.as_deref(), Some("t1"));

        let week =
            r.db.read(|c| {
                occurrences::list_occurrences(
                    c,
                    brussels(2026, 10, 19, 0),
                    brussels(2026, 10, 26, 0),
                    chrono_tz::Europe::Brussels,
                )
            })
            .await
            .unwrap();
        let starts: Vec<_> = week
            .iter()
            .map(|o| match o.start {
                OccurrenceTime::Timed(t) => t.timestamp(),
                OccurrenceTime::Date(_) => unreachable!(),
            })
            .collect();
        assert_eq!(
            starts,
            vec![
                brussels(2026, 10, 20, 9),
                brussels(2026, 10, 21, 10),
                brussels(2026, 10, 22, 9),
            ]
        );
        // The account owns the calendar, but nothing is editable before Google write (#18).
        assert!(!week[0].editable);
    }

    #[tokio::test]
    async fn an_expired_sync_token_is_followed_by_a_full_pull() {
        let mut r = running().await;
        full_pull_with_the_exception_first(&r.server).await;
        r.sync().await;
        Mock::given(method("GET"))
            .and(path(EVENTS))
            .and(query_param("syncToken", "t1"))
            .respond_with(ResponseTemplate::new(410))
            .expect(1)
            .mount(&r.server)
            .await;

        assert_eq!(r.sync().await, (SyncState::Ok, None));
        assert_eq!(r.cursor().await.as_deref(), Some("t1"));
        let full_pulls = r
            .server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|q| {
                q.url.path() == EVENTS && q.url.query().is_some_and(|q| !q.contains("syncToken"))
            })
            .filter(|q| !q.url.query().unwrap().contains("pageToken"))
            .count();
        assert_eq!(full_pulls, 2, "the first sync and the one after the 410");
    }
}
