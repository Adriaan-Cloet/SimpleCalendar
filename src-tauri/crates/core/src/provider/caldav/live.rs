//! A test against a real CalDAV server, opt-in:
//!
//! ```sh
//! SIMPLECALENDAR_CALDAV_URL=https://dav.example.com SIMPLECALENDAR_CALDAV_USER=alice \
//! SIMPLECALENDAR_CALDAV_PASSWORD=... cargo test -p simplecalendar-core caldav_live -- --ignored
//! ```
//!
//! It creates a calendar named `simplecalendar-test` in the account's home, reads it, changes
//! it on the server and reads the changes, writes through the provider (#27), then deletes it
//! again. Nothing else is touched. Its address differs per run: Nextcloud keeps a deleted
//! calendar in its trash bin and refuses to create one at the same address.

use chrono::TimeZone;
use chrono_tz::Europe::Brussels;
use reqwest::Method;

use super::*;
use crate::provider::{
    Etag, EventData, EventUpdate, Field, Reminder, ReminderMethod, Reminders, SeriesRole,
};

const CALENDAR: &str = "simplecalendar-test";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

fn event(uid: &str, extra: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//SimpleCalendar test//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261001T080000Z\r\n\
         DTSTART;TZID=Europe/Brussels:20261102T140000\r\n\
         DTEND;TZID=Europe/Brussels:20261102T150000\r\nSUMMARY:{uid}\r\n{extra}END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

const EXCEPTION: &str = "BEGIN:VEVENT\r\nUID:weekly\r\nDTSTAMP:20261001T080000Z\r\n\
    RECURRENCE-ID;TZID=Europe/Brussels:20261109T140000\r\n\
    DTSTART;TZID=Europe/Brussels:20261109T160000\r\n\
    DTEND;TZID=Europe/Brussels:20261109T170000\r\nSUMMARY:moved\r\nEND:VEVENT\r\n";

/// The test's own requests, outside the provider.
struct Admin {
    client: reqwest::Client,
    user: String,
    password: String,
}

impl Admin {
    async fn send(&self, method: &str, url: &Url, body: String) -> StatusCode {
        self.client
            .request(Method::from_bytes(method.as_bytes()).unwrap(), url.clone())
            .basic_auth(&self.user, Some(&self.password))
            .header(CONTENT_TYPE, "text/calendar; charset=utf-8")
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn get(&self, url: &Url) -> String {
        self.client
            .get(url.clone())
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    async fn put(&self, collection: &Url, name: &str, body: String) {
        let status = self
            .send("PUT", &collection.join(name).unwrap(), body)
            .await;
        assert!(status.is_success(), "PUT {name}: {status}");
    }
}

#[tokio::test]
#[ignore = "needs a CalDAV server: see the module comment"]
async fn caldav_live() {
    let cx = SourceContext {
        source_id: "live".into(),
        display_name: "Live".into(),
        secret: Some(Secret::Caldav {
            url: env("SIMPLECALENDAR_CALDAV_URL"),
            username: env("SIMPLECALENDAR_CALDAV_USER"),
            password: env("SIMPLECALENDAR_CALDAV_PASSWORD"),
        }),
    };
    let admin = Admin {
        client: http::client().unwrap(),
        user: env("SIMPLECALENDAR_CALDAV_USER"),
        password: env("SIMPLECALENDAR_CALDAV_PASSWORD"),
    };
    let provider = CaldavProvider::new().unwrap();
    let account = Account::of(&cx).unwrap();
    let (Found::Home(home), _) = provider.discover(&account).await.unwrap() else {
        panic!("the address is a calendar; give the server's address");
    };

    let run = chrono::Utc::now().timestamp();
    let collection = home.join(&format!("{CALENDAR}-{run}/")).unwrap();
    let created = admin
        .send(
            "MKCALENDAR",
            &collection,
            format!(
                r#"<?xml version="1.0"?><c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:set><d:prop><d:displayname>{CALENDAR}</d:displayname></d:prop></d:set></c:mkcalendar>"#
            ),
        )
        .await;
    assert_eq!(created, StatusCode::CREATED);
    admin
        .put(&collection, "single.ics", event("single", ""))
        .await;
    admin
        .put(
            &collection,
            "weekly.ics",
            event("weekly", "RRULE:FREQ=WEEKLY;COUNT=4\r\n")
                .replace("END:VCALENDAR", &format!("{EXCEPTION}END:VCALENDAR")),
        )
        .await;

    let calendars = provider.list_calendars(&cx).await.unwrap();
    let calendar = calendars
        .iter()
        .find(|c| c.name == CALENDAR)
        .expect("the new calendar is listed");
    let reference = CalendarRef {
        remote_id: calendar.remote_id.clone(),
        time_zone: calendar.time_zone.clone(),
    };

    let all = provider.pull(&cx, &reference, None, None).await.unwrap();
    assert!(all.full);
    let mut titles: Vec<_> = all.upserts.iter().map(|e| e.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, ["moved", "single", "weekly"]);
    let Next::Done(cursor) = all.next else {
        panic!("one page")
    };

    // Nothing changed.
    let same = provider
        .pull(&cx, &reference, Some(&cursor), None)
        .await
        .unwrap();
    assert!(same.upserts.is_empty() && same.deletions.is_empty() && !same.full);
    let Next::Done(cursor) = same.next else {
        panic!("one page")
    };

    // One event added, one deleted, and the series lost its exception.
    admin
        .put(&collection, "added.ics", event("added", ""))
        .await;
    admin
        .send(
            "DELETE",
            &collection.join("single.ics").unwrap(),
            String::new(),
        )
        .await;
    admin
        .put(
            &collection,
            "weekly.ics",
            event("weekly", "RRULE:FREQ=WEEKLY;COUNT=4\r\n"),
        )
        .await;

    let changes = provider
        .pull(&cx, &reference, Some(&cursor), None)
        .await
        .unwrap();
    assert!(!changes.full);
    let mut titles: Vec<_> = changes.upserts.iter().map(|e| e.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, ["added", "weekly"]);
    let path = collection.path();
    let mut deletions = changes.deletions.clone();
    deletions.sort();
    let exception = all
        .upserts
        .iter()
        .find(|e| matches!(e.series, SeriesRole::Exception { .. }))
        .unwrap();
    assert_eq!(
        deletions,
        [format!("{path}single.ics"), exception.remote_id.clone()]
    );

    writes(&cx, &admin, &provider, &reference, &collection, run).await;

    admin.send("DELETE", &collection, String::new()).await;
}

fn brussels(d: u32, h: u32) -> i64 {
    Brussels
        .with_ymd_and_hms(2026, 11, d, h, 0, 0)
        .single()
        .unwrap()
        .timestamp()
}

fn retitled(remote_id: &str, base: &RemoteEvent, title: &str) -> EventUpdate {
    EventUpdate {
        remote_id: remote_id.into(),
        desired: EventData {
            title: title.into(),
            ..EventData::from(base)
        },
        changed: [Field::Title].into_iter().collect(),
    }
}

/// Creating, changing and deleting through the provider, as the outbox does (§7.3).
async fn writes(
    cx: &SourceContext,
    admin: &Admin,
    provider: &CaldavProvider,
    calendar: &CalendarRef,
    collection: &Url,
    run: i64,
) {
    let writer = provider.as_writable().expect("CalDAV writes");
    let url = |href: &str| collection.join(href).unwrap();

    // A new event, with a reminder; sent twice, as after a lost answer.
    let id = new_event_id(&calendar.remote_id, &format!("new{run}"));
    let data = EventData {
        title: "created".into(),
        description: Some("two\nlines, and more".into()),
        location: None,
        all_day: false,
        start_utc: brussels(3, 10),
        end_utc: brussels(3, 11),
        start_tz: Some("Europe/Brussels".into()),
        end_tz: Some("Europe/Brussels".into()),
        recurrence: None,
        reminders: Reminders {
            use_default: false,
            overrides: vec![Reminder {
                method: ReminderMethod::Popup,
                minutes_before: 15,
            }],
        },
    };
    let created = writer.create_event(cx, calendar, &id, &data).await.unwrap();
    assert_eq!(EventData::from(&created), data);
    let again = writer.create_event(cx, calendar, &id, &data).await.unwrap();
    assert_eq!(again.remote_id, created.remote_id);
    let read = writer.get_event(cx, calendar, &id).await.unwrap().unwrap();
    assert_eq!(EventData::from(&read), data);

    // A change keeps what this app does not know.
    admin
        .put(
            collection,
            "kept.ics",
            event("kept", "CATEGORIES:Work\r\nX-OTHER-APP:keep me\r\n"),
        )
        .await;
    let kept_id = format!("{}kept.ics", collection.path());
    let kept = writer
        .get_event(cx, calendar, &kept_id)
        .await
        .unwrap()
        .unwrap();
    let base = Etag(kept.etag.clone().unwrap());
    let changed = writer
        .update_event(
            cx,
            calendar,
            &retitled(&kept_id, &kept, "renamed"),
            Some(&base),
        )
        .await
        .unwrap();
    assert_eq!(changed.title, "renamed");
    let text = admin.get(&url("kept.ics")).await;
    assert!(text.contains("X-OTHER-APP:keep me"), "{text}");
    assert!(text.contains("SUMMARY:renamed"), "{text}");

    // An edit based on the ETag this app's own write replaced is no conflict. Once someone
    // else wrote, an edit or delete based on it is.
    let stale = writer
        .update_event(
            cx,
            calendar,
            &retitled(&kept_id, &kept, "again"),
            Some(&base),
        )
        .await;
    assert!(stale.is_ok(), "own writes are no conflict: {stale:?}");
    admin
        .put(
            collection,
            "kept.ics",
            event("kept", "X-CHANGED:elsewhere\r\n"),
        )
        .await;
    let conflict = writer
        .update_event(
            cx,
            calendar,
            &retitled(&kept_id, &kept, "lost"),
            Some(&base),
        )
        .await;
    assert!(
        matches!(conflict, Err(ProviderError::Conflict)),
        "{conflict:?}"
    );
    let conflict = writer
        .delete_event(cx, calendar, &kept_id, Some(&base))
        .await;
    assert!(
        matches!(conflict, Err(ProviderError::Conflict)),
        "{conflict:?}"
    );

    // One occurrence of the series becomes an exception in its resource, and the series can
    // still be changed from the ETag it was read with.
    let series_id = format!("{}weekly.ics", collection.path());
    let series = writer
        .get_event(cx, calendar, &series_id)
        .await
        .unwrap()
        .unwrap();
    let series_etag = Etag(series.etag.clone().unwrap());
    let occurrence = occurrence_id(&series_id, brussels(16, 14));
    let one = EventUpdate {
        remote_id: occurrence.clone(),
        desired: EventData {
            title: "just this one".into(),
            start_utc: brussels(16, 15),
            end_utc: brussels(16, 16),
            recurrence: None,
            ..EventData::from(&series)
        },
        changed: [Field::Title, Field::Times].into_iter().collect(),
    };
    let exception = writer
        .update_event(cx, calendar, &one, Some(&series_etag))
        .await
        .unwrap();
    assert_eq!(exception.remote_id, occurrence);
    assert_eq!(EventData::from(&exception), one.desired);
    writer
        .update_event(
            cx,
            calendar,
            &retitled(&series_id, &series, "weekly, renamed"),
            Some(&series_etag),
        )
        .await
        .unwrap();
    let text = admin.get(&url("weekly.ics")).await;
    assert!(
        text.contains("SUMMARY:weekly, renamed") || text.contains("SUMMARY:weekly\\, renamed"),
        "{text}"
    );
    assert!(text.contains("SUMMARY:just this one"), "{text}");

    // Another occurrence goes; then the exception; then the whole series.
    writer
        .delete_event(
            cx,
            calendar,
            &occurrence_id(&series_id, brussels(23, 14)),
            None,
        )
        .await
        .unwrap();
    writer
        .delete_event(cx, calendar, &occurrence, None)
        .await
        .unwrap();
    let text = admin.get(&url("weekly.ics")).await;
    assert!(text.contains("EXDATE"), "{text}");
    assert!(!text.contains("RECURRENCE-ID"), "{text}");
    let series = writer
        .get_event(cx, calendar, &series_id)
        .await
        .unwrap()
        .unwrap();
    writer
        .delete_event(cx, calendar, &series_id, series.etag.map(Etag).as_ref())
        .await
        .unwrap();
    assert!(
        writer
            .get_event(cx, calendar, &series_id)
            .await
            .unwrap()
            .is_none()
    );
}
