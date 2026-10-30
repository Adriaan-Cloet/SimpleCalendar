//! Writes to remote calendars go through the outbox (#18, `docs/architecture.md` §7.3).
//!
//! The calendar of these tests stands for a Google calendar the account may write: a source of
//! kind `google` (id `google`) with `access = 'write'`.

use chrono_tz::Europe::Brussels;
use rusqlite::OptionalExtension;

use super::events::{assert_invalid, draft, local, meeting, single};
use super::*;
use crate::provider::EventData;

// ---- helpers -------------------------------------------------------------------------------

/// One outbox row: `op`, `event_remote_id`, `payload`, `base`, `base_etag`.
type Row = (
    String,
    String,
    Option<EventData>,
    Option<EventData>,
    Option<String>,
);

impl Setup {
    pub(super) async fn google_calendar(&self) -> i64 {
        self.remote_calendar("google", "write").await
    }

    /// A writable CalDAV calendar at `https://dav.example.com/alice/work/`.
    pub(super) async fn caldav_calendar(&self) -> i64 {
        let id = self.remote_calendar("caldav", "write").await;
        self.db
            .write(move |c| {
                c.execute(
                    "UPDATE calendars SET remote_id = 'https://dav.example.com/alice/work/'
                     WHERE id = ?1",
                    [id],
                )
            })
            .await
            .unwrap();
        id
    }

    pub(super) async fn outbox(&self) -> Vec<Row> {
        self.db
            .read(|c| {
                c.prepare(
                    "SELECT op, event_remote_id, payload, base, base_etag FROM outbox ORDER BY id",
                )?
                .query_map([], |r| {
                    let data = |i: usize| -> rusqlite::Result<Option<EventData>> {
                        let text: Option<String> = r.get(i)?;
                        Ok(text.map(|t| serde_json::from_str(&t).unwrap()))
                    };
                    Ok((r.get(0)?, r.get(1)?, data(2)?, data(3)?, r.get(4)?))
                })?
                .collect()
            })
            .await
            .unwrap()
    }

    /// The event's `remote_id`, and whether it waits to be deleted.
    pub(super) async fn remote_row(&self, id: i64) -> Option<(String, bool)> {
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT remote_id, pending_delete FROM events WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
            })
            .await
            .unwrap()
    }

    /// A meeting the server already has in `calendar`.
    pub(super) async fn synced_meeting(&mut self, calendar: i64) -> i64 {
        self.synced(meeting(calendar)).await
    }

    /// The event of `d` as if the server already had it: no outbox row, ETag `"e1"`.
    pub(super) async fn synced(&mut self, d: EventDraft) -> i64 {
        let id = self.service.create_event(d, "UTC").await.unwrap().event_id;
        self.db
            .write(move |c| {
                c.execute("DELETE FROM outbox", [])?;
                c.execute("UPDATE events SET etag = '\"e1\"' WHERE id = ?1", [id])
            })
            .await
            .unwrap();
        self.requests.pushes();
        while self.events.try_recv().is_ok() {}
        id
    }
}

fn one_hour_later(mut d: EventDraft) -> EventDraft {
    d.start = "2026-10-20T10:00".into();
    d.end = "2026-10-20T11:00".into();
    d
}

// ---- single events -------------------------------------------------------------------------

#[tokio::test]
async fn a_new_google_event_is_queued_as_a_create_under_its_own_id() {
    let mut s = setup();
    let cal = s.google_calendar().await;

    let created = s.service.create_event(meeting(cal), "UTC").await.unwrap();

    let (remote_id, _) = s.remote_row(created.event_id).await.unwrap();
    let rows = s.outbox().await;
    assert_eq!(rows.len(), 1);
    let (op, row_remote_id, payload, base, etag) = &rows[0];
    assert_eq!((op.as_str(), row_remote_id), ("create", &remote_id));
    assert_eq!(payload.as_ref().unwrap().title, "Meeting");
    assert_eq!((base, etag), (&None, &None));
    assert_eq!(s.requests.pushes(), vec!["google".to_owned()]);
}

#[tokio::test]
async fn an_edit_is_queued_with_the_state_before_and_after_and_its_etag() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;

    s.service
        .update_event(single(id), Scope::All, one_hour_later(meeting(cal)), "UTC")
        .await
        .unwrap();

    let rows = s.outbox().await;
    assert_eq!(rows.len(), 1);
    let (op, _, payload, base, etag) = &rows[0];
    assert_eq!(op, "update");
    let (payload, base) = (payload.as_ref().unwrap(), base.as_ref().unwrap());
    assert_eq!(payload.start_utc, instant(Brussels, 2026, 10, 20, 10, 0));
    assert_eq!(base.start_utc, instant(Brussels, 2026, 10, 20, 9, 0));
    assert_eq!(etag.as_deref(), Some("\"e1\""));
    assert_eq!(s.requests.pushes(), vec!["google".to_owned()]);
}

#[tokio::test]
async fn an_edit_that_changes_nothing_queues_nothing() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;

    s.service
        .update_event(single(id), Scope::All, meeting(cal), "UTC")
        .await
        .unwrap();

    assert!(s.outbox().await.is_empty());
    assert!(s.requests.pushes().is_empty());
}

#[tokio::test]
async fn a_deleted_google_event_is_hidden_until_the_server_confirms() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;

    s.service
        .delete_event(single(id), Scope::All)
        .await
        .unwrap();

    assert_eq!(s.remote_row(id).await.map(|r| r.1), Some(true));
    let rows = s.outbox().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "delete");
    assert_eq!(rows[0].3.as_ref().unwrap().title, "Meeting");
    assert!(
        s.service
            .list_occurrences("2026-10-19", "2026-10-26", "UTC")
            .await
            .unwrap()
            .is_empty()
    );
    let gone = s.service.get_event(single(id)).await;
    assert!(matches!(gone, Err(ServiceError::NotFound(_))), "{gone:?}");
    let again = s.service.delete_event(single(id), Scope::All).await;
    assert!(matches!(again, Err(ServiceError::NotFound(_))), "{again:?}");
    assert_eq!(s.requests.pushes(), vec!["google".to_owned()]);
}

#[tokio::test]
async fn writes_to_local_calendars_queue_nothing() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let id = s
        .service
        .create_event(meeting(cal), "UTC")
        .await
        .unwrap()
        .event_id;
    s.service
        .update_event(single(id), Scope::All, one_hour_later(meeting(cal)), "UTC")
        .await
        .unwrap();
    s.service
        .delete_event(single(id), Scope::All)
        .await
        .unwrap();

    assert!(s.outbox().await.is_empty());
    assert!(s.requests.pushes().is_empty());
    assert_eq!(s.count("events").await, 0);
}

#[tokio::test]
async fn events_move_only_between_calendars_on_this_computer() {
    let mut s = setup();
    let google = s.google_calendar().await;
    let home = local(&mut s, "Home").await;
    let on_google = s.synced_meeting(google).await;
    let at_home = s
        .service
        .create_event(meeting(home), "UTC")
        .await
        .unwrap()
        .event_id;

    assert_invalid(
        s.service
            .update_event(single(on_google), Scope::All, meeting(home), "UTC")
            .await,
    );
    assert_invalid(
        s.service
            .update_event(single(at_home), Scope::All, meeting(google), "UTC")
            .await,
    );
    assert!(s.outbox().await.is_empty());
}

// ---- series --------------------------------------------------------------------------------

/// A weekly standup in `calendar` from Tuesday 20 October 2026, 09:00 to 09:30 in Brussels,
/// as the server has it.
pub(super) async fn google_standup(s: &mut Setup, calendar: i64) -> i64 {
    s.synced(standup_on(calendar, "2026-10-20", "Standup"))
        .await
}

fn standup_on(calendar: i64, date: &str, title: &str) -> EventDraft {
    draft(serde_json::json!({
        "calendar_id": calendar, "title": title, "all_day": false,
        "start": format!("{date}T09:00"), "end": format!("{date}T09:30"),
        "time_zone": "Europe/Brussels",
        "repeat": {"frequency": "weekly"},
    }))
}

#[tokio::test]
async fn one_occurrence_of_a_google_series_is_addressed_by_googles_instance_id() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = google_standup(&mut s, cal).await;
    let (master, _) = s.remote_row(id).await.unwrap();
    let nov3 = instant(Brussels, 2026, 11, 3, 9, 0);
    let nov10 = instant(Brussels, 2026, 11, 10, 9, 0);

    s.service
        .update_event(
            OccurrenceRef {
                event_id: id,
                original_start: nov3,
            },
            Scope::This,
            standup_on(cal, "2026-11-03", "Retro"),
            "UTC",
        )
        .await
        .unwrap();
    s.service
        .delete_event(
            OccurrenceRef {
                event_id: id,
                original_start: nov10,
            },
            Scope::This,
        )
        .await
        .unwrap();

    // 09:00 in Brussels is 08:00 UTC in winter.
    let rows = s.outbox().await;
    let sent: Vec<_> = rows.iter().map(|r| (r.0.as_str(), r.1.as_str())).collect();
    let (on_nov3, on_nov10) = (
        format!("{master}_20261103T080000Z"),
        format!("{master}_20261110T080000Z"),
    );
    assert_eq!(
        sent,
        vec![("update", on_nov3.as_str()), ("delete", on_nov10.as_str())]
    );
    // The change is against the occurrence as the series had it.
    let (payload, base) = (rows[0].2.as_ref().unwrap(), rows[0].3.as_ref().unwrap());
    assert_eq!(
        (base.title.as_str(), payload.title.as_str()),
        ("Standup", "Retro")
    );
    assert_eq!(
        (base.recurrence.as_ref(), payload.recurrence.as_ref()),
        (None, None)
    );
}

#[tokio::test]
async fn a_new_caldav_event_is_named_by_its_path_in_the_calendar() {
    let s = setup();
    let cal = s.caldav_calendar().await;
    let id = s
        .service
        .create_event(meeting(cal), "UTC")
        .await
        .unwrap()
        .event_id;
    let (remote_id, _) = s.remote_row(id).await.unwrap();
    let name = remote_id
        .strip_prefix("/alice/work/")
        .and_then(|r| r.strip_suffix(".ics"))
        .unwrap_or_else(|| panic!("{remote_id}"));
    assert_eq!(name.len(), 32);
    assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
    let rows = s.outbox().await;
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1.as_str()),
        ("create", remote_id.as_str())
    );
}

#[tokio::test]
async fn one_occurrence_of_a_caldav_series_is_addressed_in_its_series_resource() {
    let mut s = setup();
    let cal = s.caldav_calendar().await;
    let id = google_standup(&mut s, cal).await;
    let (master, _) = s.remote_row(id).await.unwrap();
    let nov3 = instant(Brussels, 2026, 11, 3, 9, 0);
    let nov10 = instant(Brussels, 2026, 11, 10, 9, 0);

    s.service
        .update_event(
            OccurrenceRef {
                event_id: id,
                original_start: nov3,
            },
            Scope::This,
            standup_on(cal, "2026-11-03", "Retro"),
            "UTC",
        )
        .await
        .unwrap();
    s.service
        .delete_event(
            OccurrenceRef {
                event_id: id,
                original_start: nov10,
            },
            Scope::This,
        )
        .await
        .unwrap();

    let rows = s.outbox().await;
    let sent: Vec<_> = rows.iter().map(|r| (r.0.as_str(), r.1.as_str())).collect();
    let (on_nov3, on_nov10) = (format!("{master}#{nov3}"), format!("{master}#{nov10}"));
    assert_eq!(
        sent,
        vec![("update", on_nov3.as_str()), ("delete", on_nov10.as_str())]
    );
    // The occurrence lives in the series' resource: the edit is based on its ETag.
    assert_eq!(rows[0].4.as_deref(), Some("\"e1\""));
}

#[tokio::test]
async fn retiming_all_events_of_a_google_series_pulls_it_again() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = google_standup(&mut s, cal).await;
    let nov3 = instant(Brussels, 2026, 11, 3, 9, 0);
    s.service
        .update_event(
            OccurrenceRef {
                event_id: id,
                original_start: nov3,
            },
            Scope::This,
            standup_on(cal, "2026-11-03", "Retro"),
            "UTC",
        )
        .await
        .unwrap();
    s.db.write(move |c| {
        c.execute("DELETE FROM outbox", [])?;
        c.execute(
            "UPDATE calendars SET sync_cursor = 'c1' WHERE id = ?1",
            [cal],
        )
    })
    .await
    .unwrap();

    let mut later = standup_on(cal, "2026-10-20", "Standup");
    later.start = "2026-10-20T10:00".into();
    later.end = "2026-10-20T10:30".into();
    s.service
        .update_event(
            OccurrenceRef {
                event_id: id,
                original_start: instant(Brussels, 2026, 10, 20, 9, 0),
            },
            Scope::All,
            later,
            "UTC",
        )
        .await
        .unwrap();

    // Google decides where its exceptions go: the local ones are dropped, and the next pull
    // is a full one that brings back what it kept.
    let exceptions: i64 =
        s.db.read(move |c| {
            c.query_row(
                "SELECT count(*) FROM events WHERE recurring_event_id = ?1",
                [id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(exceptions, 0);
    let cursor: Option<String> =
        s.db.read(move |c| {
            c.query_row(
                "SELECT sync_cursor FROM calendars WHERE id = ?1",
                [cal],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(cursor, None);
    let rows = s.outbox().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "update");
    assert_eq!(
        rows[0].2.as_ref().unwrap().start_utc,
        instant(Brussels, 2026, 10, 20, 10, 0)
    );
}

#[tokio::test]
async fn a_new_google_series_deleted_before_it_is_sent_leaves_nothing_to_send() {
    let s = setup();
    let cal = s.google_calendar().await;
    let created = s
        .service
        .create_event(standup_on(cal, "2026-10-20", "Standup"), "UTC")
        .await
        .unwrap();
    let id = created.event_id;
    let nov3 = instant(Brussels, 2026, 11, 3, 9, 0);
    let occurrence = |original_start| OccurrenceRef {
        event_id: id,
        original_start,
    };
    s.service
        .update_event(
            occurrence(nov3),
            Scope::This,
            standup_on(cal, "2026-11-03", "Retro"),
            "UTC",
        )
        .await
        .unwrap();
    assert_eq!(s.outbox().await.len(), 2);

    s.service
        .delete_event(occurrence(created.original_start), Scope::All)
        .await
        .unwrap();

    // The create and the change of its occurrence go, and the series with its exception.
    assert!(s.outbox().await.is_empty());
    let events: i64 =
        s.db.read(|c| c.query_row("SELECT count(*) FROM events", [], |r| r.get(0)))
            .await
            .unwrap();
    assert_eq!(events, 0);
}
