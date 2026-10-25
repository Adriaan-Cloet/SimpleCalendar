use super::*;
use crate::model::EventStatus;
use crate::provider::Reminders;

fn event(uid: &str, series: SeriesRole) -> RemoteEvent {
    RemoteEvent {
        remote_id: uid.into(),
        ical_uid: Some(uid.into()),
        status: EventStatus::Confirmed,
        title: uid.into(),
        description: None,
        location: None,
        color: None,
        all_day: false,
        start_utc: 0,
        end_utc: 3600,
        start_tz: None,
        end_tz: None,
        series,
        remote_updated_at: None,
        etag: None,
        is_organizer: true,
        guests_can_modify: false,
        reminders: Reminders::default(),
    }
}

fn exception(uid: &str, original: i64) -> RemoteEvent {
    event(
        uid,
        SeriesRole::Exception {
            master_remote_id: uid.into(),
            original_start_utc: original,
        },
    )
}

fn master(uid: &str) -> RemoteEvent {
    event(
        uid,
        SeriesRole::Master {
            recurrence: "RRULE:FREQ=DAILY".into(),
        },
    )
}

fn item(etag: &str, exceptions: &[i64]) -> Item {
    Item {
        etag: etag.into(),
        exceptions: exceptions.to_vec(),
    }
}

// ---- comparing a listing -------------------------------------------------------------------

#[test]
fn a_listing_names_what_to_fetch_and_what_is_gone() {
    let seen = BTreeMap::from([
        ("/c/same.ics".to_owned(), item("\"1\"", &[])),
        ("/c/changed.ics".to_owned(), item("\"1\"", &[])),
        ("/c/gone.ics".to_owned(), item("\"1\"", &[100])),
    ]);
    let listed = BTreeMap::from([
        ("/c/same.ics".to_owned(), Some("\"1\"".to_owned())),
        ("/c/changed.ics".to_owned(), Some("\"2\"".to_owned())),
        ("/c/new.ics".to_owned(), Some("\"1\"".to_owned())),
        // Without an ETag nothing can be compared, so it is always fetched.
        ("/c/untagged.ics".to_owned(), None),
    ]);
    let diff = compare(&seen, &listed);
    assert_eq!(
        diff.fetch,
        ["/c/changed.ics", "/c/new.ics", "/c/untagged.ics"]
    );
    assert_eq!(diff.gone, ["/c/gone.ics"]);
}

#[test]
fn a_gone_resource_deletes_its_event_and_exceptions() {
    let ids = item("\"1\"", &[100, 200]).remote_ids("/c/a.ics");
    assert_eq!(ids, ["/c/a.ics", "/c/a.ics#100", "/c/a.ics#200"]);
}

// ---- reading a resource --------------------------------------------------------------------

#[test]
fn events_are_named_after_their_resource() {
    let read = resource(
        "/c/w.ics",
        "\"7\"",
        vec![master("weekly"), exception("weekly", 100)],
        None,
    );
    let ids: Vec<_> = read.events.iter().map(|e| e.remote_id.as_str()).collect();
    assert_eq!(ids, ["/c/w.ics", "/c/w.ics#100"]);
    assert_eq!(
        read.events[1].series,
        SeriesRole::Exception {
            master_remote_id: "/c/w.ics".into(),
            original_start_utc: 100,
        }
    );
    // The UID stays as it was: writing an event back needs it (#27).
    assert_eq!(read.events[0].ical_uid.as_deref(), Some("weekly"));
    assert!(
        read.events
            .iter()
            .all(|e| e.etag.as_deref() == Some("\"7\""))
    );
    assert_eq!(read.item, item("\"7\"", &[100]));
    assert!(read.removed.is_empty());
}

#[test]
fn an_exception_that_left_the_resource_is_removed() {
    let before = item("\"1\"", &[100, 200]);
    let read = resource(
        "/c/w.ics",
        "\"2\"",
        vec![master("weekly"), exception("weekly", 200)],
        Some(&before),
    );
    assert_eq!(read.removed, ["/c/w.ics#100"]);
}

#[test]
fn a_resource_that_lost_its_master_removes_it() {
    // Only an exception is left, as for an invitation to one occurrence.
    let before = item("\"1\"", &[100]);
    let read = resource(
        "/c/w.ics",
        "\"2\"",
        vec![exception("weekly", 100)],
        Some(&before),
    );
    assert_eq!(read.removed, ["/c/w.ics"]);
    assert_eq!(read.item.exceptions, [100]);
}

#[test]
fn a_second_master_in_one_resource_is_left_out() {
    // RFC 4791 §4.1 allows one UID per resource; a second one would share the remote id.
    let read = resource(
        "/c/x.ics",
        "\"1\"",
        vec![
            event("a", SeriesRole::Single),
            event("b", SeriesRole::Single),
        ],
        None,
    );
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].ical_uid.as_deref(), Some("a"));
}

#[test]
fn an_empty_resource_removes_everything_it_had() {
    let before = item("\"1\"", &[100]);
    let read = resource("/c/x.ics", "\"2\"", Vec::new(), Some(&before));
    assert_eq!(read.removed, ["/c/x.ics", "/c/x.ics#100"]);
}

// ---- the stored form -----------------------------------------------------------------------

#[test]
fn the_cursor_round_trips_and_stays_small() {
    let cursor = Cursor {
        token: Some("http://sabre.io/ns/sync/4".into()),
        ctag: None,
        items: BTreeMap::from([
            ("/c/a.ics".to_owned(), item("\"1\"", &[])),
            ("/c/b.ics".to_owned(), item("\"2\"", &[100])),
        ]),
    };
    let stored = cursor.store();
    assert_eq!(
        stored.0,
        r#"{"token":"http://sabre.io/ns/sync/4","items":{"/c/a.ics":{"etag":"\"1\""},"/c/b.ics":{"etag":"\"2\"","exceptions":[100]}}}"#
    );
    assert_eq!(Cursor::load(&stored), Some(cursor));
}

#[test]
fn an_unreadable_cursor_is_none() {
    // An ICS or Google cursor, or one from a future version.
    assert_eq!(Cursor::load(&SyncCursor("abc".into())), None);
}
