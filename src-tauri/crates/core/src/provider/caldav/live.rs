//! A test against a real CalDAV server, opt-in:
//!
//! ```sh
//! SIMPLECALENDAR_CALDAV_URL=https://dav.example.com SIMPLECALENDAR_CALDAV_USER=alice \
//! SIMPLECALENDAR_CALDAV_PASSWORD=... cargo test -p simplecalendar-core caldav_live -- --ignored
//! ```
//!
//! It creates a calendar named `simplecalendar-test` in the account's home, reads it, changes
//! it on the server and reads the changes, then deletes it again. Nothing else is touched. Its
//! address differs per run: Nextcloud keeps a deleted calendar in its trash bin and refuses to
//! create one at the same address.

use reqwest::Method;

use super::*;
use crate::provider::SeriesRole;

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
    let Found::Home(home) = provider.discover(&account).await.unwrap() else {
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

    admin.send("DELETE", &collection, String::new()).await;
}
