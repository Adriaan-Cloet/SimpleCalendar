//! Creating, reading, changing and deleting events (#16, `docs/architecture.md` §4, §10).

use chrono_tz::Europe::Brussels;
use chrono_tz::UTC;

use super::*;
use crate::provider::ReminderMethod;

// ---- helpers -------------------------------------------------------------------------------

/// A local calendar named `name`, with the event of its creation already drained.
async fn local(s: &mut Setup, name: &str) -> i64 {
    let id = s.service.create_local_calendar(name).await.unwrap().id;
    while s.events.try_recv().is_ok() {}
    id
}

fn draft(json: serde_json::Value) -> EventDraft {
    serde_json::from_value(json).unwrap()
}

/// A one-hour meeting in Brussels on 20 October 2026 in `calendar`.
fn meeting(calendar: i64) -> EventDraft {
    draft(serde_json::json!({
        "calendar_id": calendar,
        "title": "Meeting",
        "all_day": false,
        "start": "2026-10-20T09:00",
        "end": "2026-10-20T10:00",
        "time_zone": "Europe/Brussels",
    }))
}

fn single(event_id: i64) -> OccurrenceRef {
    OccurrenceRef {
        event_id,
        original_start: 0,
    }
}

/// The stored times of an event: `start_utc`, `end_utc`, `start_tz`, `end_tz`, `all_day`.
type Times = (i64, i64, Option<String>, Option<String>, bool);

impl Setup {
    async fn times(&self, id: i64) -> Times {
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT start_utc, end_utc, start_tz, end_tz, all_day FROM events WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
            })
            .await
            .unwrap()
    }

    /// Inserts a single event with these flags straight into `calendar`. Returns its id.
    async fn raw_event(&self, calendar: i64, organizer: bool, guests_can_modify: bool) -> i64 {
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT INTO events (calendar_id, remote_id, title, start_utc, end_utc,
                                         start_tz, is_organizer, guests_can_modify)
                     VALUES (?1, 'raw', 'Raw', 0, 3600, 'UTC', ?2, ?3)",
                    params![calendar, organizer, guests_can_modify],
                )?;
                Ok(c.last_insert_rowid())
            })
            .await
            .unwrap()
    }

    async fn set_access(&self, calendar: i64, access: &'static str) {
        self.db
            .write(move |c| {
                c.execute(
                    "UPDATE calendars SET access = ?2 WHERE id = ?1",
                    params![calendar, access],
                )
            })
            .await
            .unwrap();
    }

    /// A calendar of a new source of `kind` with `access`. Returns its id.
    async fn remote_calendar(&self, kind: &'static str, access: &'static str) -> i64 {
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT INTO sources (id, kind, display_name, created_at)
                     VALUES (?1, ?1, 'Remote', 1)",
                    [kind],
                )?;
                c.execute(
                    "INSERT INTO calendars (source_id, remote_id, name, access)
                     VALUES (?1, 'cal', 'Remote', ?2)",
                    params![kind, access],
                )?;
                Ok(c.last_insert_rowid())
            })
            .await
            .unwrap()
    }
}

fn is_not_editable(result: Result<impl std::fmt::Debug, ServiceError>) -> bool {
    matches!(result, Err(ServiceError::NotEditable))
}

fn assert_invalid<T: std::fmt::Debug>(result: Result<T, ServiceError>) {
    assert!(
        matches!(result, Err(ServiceError::InvalidInput(_))),
        "{result:?}"
    );
}

// ---- creating ------------------------------------------------------------------------------

#[tokio::test]
async fn a_timed_event_is_stored_in_its_zone_and_returned_in_the_viewers() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;

    let created = s.service.create_event(meeting(cal), "UTC").await.unwrap();

    let start = instant(Brussels, 2026, 10, 20, 9, 0);
    assert_eq!(
        s.times(created.event_id).await,
        (
            start,
            start + 3600,
            Some("Europe/Brussels".into()),
            Some("Europe/Brussels".into()),
            false
        )
    );
    assert_eq!(created.calendar_id, cal);
    assert_eq!(created.title, "Meeting");
    assert_eq!(created.original_start, start);
    assert!(created.editable);
    // 09:00 in Brussels is 07:00 UTC in summer time.
    assert_eq!(
        serde_json::to_value(created.start).unwrap(),
        "2026-10-20T07:00:00+00:00"
    );
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![cal]
        }
    );
    assert!(
        s.requests.manual().is_empty(),
        "a local event starts no sync"
    );
}

#[tokio::test]
async fn a_new_event_shows_in_its_week() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    s.service.create_event(meeting(cal), "UTC").await.unwrap();

    let week = s
        .service
        .list_occurrences("2026-10-19", "2026-10-26", "Europe/Brussels")
        .await
        .unwrap();
    assert_eq!(titles(&week), vec!["Meeting"]);
    assert!(week[0].editable);
}

#[tokio::test]
async fn a_time_in_the_spring_forward_gap_moves_past_it() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    // 02:30 does not exist in Brussels on 29 March 2026: the clocks jump from 02:00 to 03:00.
    let created = s
        .service
        .create_event(
            draft(serde_json::json!({
                "calendar_id": cal,
                "title": "Early",
                "all_day": false,
                "start": "2026-03-29T02:30",
                "end": "2026-03-29T04:00",
                "time_zone": "Europe/Brussels",
            })),
            "Europe/Brussels",
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(created.start).unwrap(),
        "2026-03-29T03:30:00+02:00"
    );
}

#[tokio::test]
async fn an_all_day_event_is_stored_as_dates() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let created = s
        .service
        .create_event(
            draft(serde_json::json!({
                "calendar_id": cal,
                "title": "Christmas",
                "all_day": true,
                "start": "2026-12-25",
                "end": "2026-12-26",
                // Ignored for an all-day event.
                "time_zone": "Europe/Brussels",
            })),
            "America/New_York",
        )
        .await
        .unwrap();

    let midnight = instant(UTC, 2026, 12, 25, 0, 0);
    assert_eq!(
        s.times(created.event_id).await,
        (midnight, midnight + 86_400, None, None, true)
    );
    assert_eq!(
        created.start,
        OccurrenceTime::Date("2026-12-25".parse().unwrap())
    );
    assert_eq!(
        created.end,
        OccurrenceTime::Date("2026-12-26".parse().unwrap())
    );
}

#[tokio::test]
async fn text_fields_are_trimmed_and_a_blank_title_gets_a_name() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = meeting(cal);
    d.title = "   ".into();
    d.location = Some("  Ghent ".into());
    d.description = Some(" \n ".into());

    let id = s.service.create_event(d, "UTC").await.unwrap().event_id;

    let event = s.service.get_event(id).await.unwrap();
    assert_eq!(event.title, "New event");
    assert_eq!(event.location.as_deref(), Some("Ghent"));
    assert_eq!(event.description, None);
}

#[tokio::test]
async fn a_new_event_gets_a_hex_remote_id_and_is_the_organisers() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let a = s.service.create_event(meeting(cal), "UTC").await.unwrap();
    let b = s.service.create_event(meeting(cal), "UTC").await.unwrap();

    let rows: Vec<(String, bool, bool)> =
        s.db.read(|c| {
            c.prepare("SELECT remote_id, is_organizer, guests_can_modify FROM events ORDER BY id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect()
        })
        .await
        .unwrap();
    assert_ne!(a.event_id, b.event_id);
    assert_ne!(rows[0].0, rows[1].0);
    for (remote_id, organizer, guests) in rows {
        // Google accepts 5 to 1024 characters from a-v and 0-9 (§7.3).
        assert_eq!(remote_id.len(), 32, "{remote_id}");
        assert!(
            remote_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{remote_id}"
        );
        assert!(organizer, "a local event is always the organiser's (§4)");
        assert!(!guests);
    }
}

#[tokio::test]
async fn bad_times_are_invalid_input_and_store_nothing() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let timed = |start: &str, end: &str, zone: Option<&str>| {
        draft(serde_json::json!({
            "calendar_id": cal, "title": "x", "all_day": false,
            "start": start, "end": end, "time_zone": zone,
        }))
    };
    let all_day = |start: &str, end: &str| {
        draft(serde_json::json!({
            "calendar_id": cal, "title": "x", "all_day": true, "start": start, "end": end,
        }))
    };
    let zone = Some("Europe/Brussels");
    for d in [
        // Ends before it starts.
        timed("2026-10-20T10:00", "2026-10-20T09:00", zone),
        // No zone, or an unknown one.
        timed("2026-10-20T09:00", "2026-10-20T10:00", None),
        timed("2026-10-20T09:00", "2026-10-20T10:00", Some("Mars/Olympus")),
        // Not a date and time.
        timed("2026-10-20", "2026-10-20T10:00", zone),
        timed("20/10/2026 09:00", "2026-10-20T10:00", zone),
        timed("2026-10-20T25:00", "2026-10-20T26:00", zone),
        // An all-day end is exclusive, so it must lie after the start.
        all_day("2026-10-20", "2026-10-20"),
        all_day("2026-10-20", "2026-10-19"),
        all_day("2026-10-20T09:00", "2026-10-21"),
    ] {
        assert_invalid(s.service.create_event(d, "UTC").await);
    }
    assert_eq!(s.count("events").await, 0);
    assert!(s.events.try_recv().is_err());
}

#[tokio::test]
async fn an_event_can_be_zero_minutes_long() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = meeting(cal);
    d.end = d.start.clone();
    let created = s.service.create_event(d, "UTC").await.unwrap();
    assert_eq!(created.start, created.end);
}

#[tokio::test]
async fn an_unknown_viewer_zone_is_invalid_input() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    assert_invalid(s.service.create_event(meeting(cal), "Nowhere").await);
    assert_eq!(s.count("events").await, 0);
}

#[tokio::test]
async fn creating_in_a_calendar_that_does_not_exist_is_not_found() {
    let s = setup();
    let err = s
        .service
        .create_event(meeting(42), "UTC")
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "{err:?}");
}

// ---- reminders -----------------------------------------------------------------------------

#[tokio::test]
async fn reminders_are_stored_sorted_and_read_back() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = meeting(cal);
    d.reminders = vec![
        EventReminder::popup(60),
        EventReminder::popup(10),
        EventReminder {
            method: ReminderMethod::Email,
            minutes_before: 1440,
        },
        // Twice the same: stored once.
        EventReminder::popup(10),
    ];
    let id = s.service.create_event(d, "UTC").await.unwrap().event_id;

    let event = s.service.get_event(id).await.unwrap();
    assert!(!event.use_default_reminders);
    assert_eq!(
        event.reminders,
        vec![
            EventReminder::popup(10),
            EventReminder::popup(60),
            EventReminder {
                method: ReminderMethod::Email,
                minutes_before: 1440
            },
        ]
    );
}

#[tokio::test]
async fn a_reminder_longer_than_four_weeks_is_invalid_input() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = meeting(cal);
    d.reminders = vec![EventReminder::popup(40_321)];
    assert_invalid(s.service.create_event(d, "UTC").await);
    assert_eq!(s.count("events").await, 0);
}

#[tokio::test]
async fn default_reminders_read_as_the_calendars() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    s.db.write(move |c| {
        c.execute(
            "UPDATE calendars SET default_reminders = '[30, 5]' WHERE id = ?1",
            [cal],
        )
    })
    .await
    .unwrap();
    let mut d = meeting(cal);
    d.use_default_reminders = true;
    // Ignored while the defaults apply.
    d.reminders = vec![EventReminder::popup(60)];
    let id = s.service.create_event(d, "UTC").await.unwrap().event_id;

    let event = s.service.get_event(id).await.unwrap();
    assert!(event.use_default_reminders);
    assert_eq!(
        event.reminders,
        vec![EventReminder::popup(5), EventReminder::popup(30)]
    );
    assert_eq!(s.count("event_reminders").await, 0);
}

// ---- reading -------------------------------------------------------------------------------

#[tokio::test]
async fn an_event_reads_back_in_its_own_zone() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = meeting(cal);
    d.description = Some("Agenda\nfirst <b>this</b>".into());
    let id = s
        .service
        .create_event(d, "America/New_York")
        .await
        .unwrap()
        .event_id;

    let event = s.service.get_event(id).await.unwrap();
    assert_eq!(event.id, id);
    assert_eq!(event.calendar_id, cal);
    assert_eq!(event.start, "2026-10-20T09:00");
    assert_eq!(event.end, "2026-10-20T10:00");
    assert_eq!(event.time_zone.as_deref(), Some("Europe/Brussels"));
    assert!(!event.all_day);
    assert!(event.editable);
    assert!(!event.recurring);
    // A local description is plain text as typed, also when it looks like HTML.
    assert_eq!(
        event.description.as_deref(),
        Some("Agenda\nfirst <b>this</b>")
    );
}

#[tokio::test]
async fn an_all_day_event_reads_back_as_dates() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let id = s
        .service
        .create_event(
            draft(serde_json::json!({
                "calendar_id": cal, "title": "Trip", "all_day": true,
                "start": "2026-10-20", "end": "2026-10-23",
            })),
            "UTC",
        )
        .await
        .unwrap()
        .event_id;
    let event = s.service.get_event(id).await.unwrap();
    assert!(event.all_day);
    assert_eq!(
        (event.start.as_str(), event.end.as_str()),
        ("2026-10-20", "2026-10-23")
    );
    assert_eq!(event.time_zone, None);
}

#[tokio::test]
async fn a_google_description_reads_as_plain_text() {
    let s = setup();
    let cal = s.remote_calendar("google", "read").await;
    let id =
        s.db.write(move |c| {
            c.execute(
                "INSERT INTO events (calendar_id, remote_id, start_utc, end_utc, start_tz,
                                     description)
                 VALUES (?1, 'g', 0, 0, 'UTC',
                         'Bring <b>snacks</b><br>and a <a href=\"https://x.test/\">map</a>')",
                [cal],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
        .unwrap();

    let event = s.service.get_event(id).await.unwrap();
    assert_eq!(
        event.description.as_deref(),
        Some("Bring snacks\nand a map (https://x.test/)")
    );
    assert!(!event.editable);
}

#[tokio::test]
async fn a_series_reads_as_recurring() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let id = s.raw_event(cal, true, false).await;
    s.db.write(move |c| {
        c.execute(
            "UPDATE events SET recurrence = 'RRULE:FREQ=DAILY' WHERE id = ?1",
            [id],
        )
    })
    .await
    .unwrap();
    assert!(s.service.get_event(id).await.unwrap().recurring);
}

#[tokio::test]
async fn reading_an_event_that_does_not_exist_is_not_found() {
    let s = setup();
    let err = s.service.get_event(42).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "{err:?}");
}

// ---- changing ------------------------------------------------------------------------------

#[tokio::test]
async fn an_event_is_updated_as_a_whole() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = meeting(cal);
    d.location = Some("Ghent".into());
    d.reminders = vec![EventReminder::popup(10)];
    let id = s.service.create_event(d, "UTC").await.unwrap().event_id;
    while s.events.try_recv().is_ok() {}

    let changed = s
        .service
        .update_event(
            single(id),
            Scope::All,
            draft(serde_json::json!({
                "calendar_id": cal,
                "title": "Lunch",
                "all_day": false,
                "start": "2026-10-21T12:00",
                "end": "2026-10-21T13:00",
                "time_zone": "Asia/Tokyo",
            })),
            "UTC",
        )
        .await
        .unwrap();

    assert_eq!(changed.event_id, id);
    assert_eq!(changed.title, "Lunch");
    assert_eq!(changed.location, None);
    let event = s.service.get_event(id).await.unwrap();
    assert_eq!(event.start, "2026-10-21T12:00");
    assert_eq!(event.time_zone.as_deref(), Some("Asia/Tokyo"));
    assert!(event.reminders.is_empty());
    assert_eq!(s.count("event_reminders").await, 0);
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![cal]
        }
    );
}

#[tokio::test]
async fn a_timed_event_can_become_all_day_and_back() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let id = s
        .service
        .create_event(meeting(cal), "UTC")
        .await
        .unwrap()
        .event_id;
    let to_all_day = draft(serde_json::json!({
        "calendar_id": cal, "title": "Meeting", "all_day": true,
        "start": "2026-10-20", "end": "2026-10-21",
    }));
    s.service
        .update_event(single(id), Scope::All, to_all_day, "UTC")
        .await
        .unwrap();
    assert!(s.times(id).await.4);

    s.service
        .update_event(single(id), Scope::All, meeting(cal), "UTC")
        .await
        .unwrap();
    assert_eq!(s.times(id).await.2.as_deref(), Some("Europe/Brussels"));
}

#[tokio::test]
async fn an_event_can_move_to_another_local_calendar() {
    let mut s = setup();
    let home = local(&mut s, "Home").await;
    let work = local(&mut s, "Work").await;
    let id = s
        .service
        .create_event(meeting(home), "UTC")
        .await
        .unwrap()
        .event_id;
    while s.events.try_recv().is_ok() {}

    let moved = s
        .service
        .update_event(single(id), Scope::All, meeting(work), "UTC")
        .await
        .unwrap();

    assert_eq!(moved.calendar_id, work);
    assert_eq!(s.service.get_event(id).await.unwrap().calendar_id, work);
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![home, work]
        }
    );
}

#[tokio::test]
async fn an_event_cannot_move_to_a_calendar_it_may_not_write() {
    let mut s = setup();
    let home = local(&mut s, "Home").await;
    let feed = s.remote_calendar("ics", "read").await;
    let id = s
        .service
        .create_event(meeting(home), "UTC")
        .await
        .unwrap()
        .event_id;

    assert!(is_not_editable(
        s.service
            .update_event(single(id), Scope::All, meeting(feed), "UTC")
            .await
    ));
    assert_eq!(s.service.get_event(id).await.unwrap().calendar_id, home);
}

#[tokio::test]
async fn invalid_changes_leave_the_event_as_it_was() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let id = s
        .service
        .create_event(meeting(cal), "UTC")
        .await
        .unwrap()
        .event_id;
    let before = s.times(id).await;
    while s.events.try_recv().is_ok() {}

    let mut d = meeting(cal);
    d.title = "Changed".into();
    d.end = "2026-10-20T08:00".into();
    assert_invalid(
        s.service
            .update_event(single(id), Scope::All, d, "UTC")
            .await,
    );

    assert_eq!(s.times(id).await, before);
    assert_eq!(s.service.get_event(id).await.unwrap().title, "Meeting");
    assert!(s.events.try_recv().is_err());
}

#[tokio::test]
async fn changing_or_deleting_an_event_that_does_not_exist_is_not_found() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let err = s
        .service
        .update_event(single(42), Scope::All, meeting(cal), "UTC")
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "{err:?}");
    let err = s
        .service
        .delete_event(single(42), Scope::All)
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn a_series_cannot_be_changed_or_deleted_yet() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let id = s.raw_event(cal, true, false).await;
    s.db.write(move |c| {
        c.execute(
            "UPDATE events SET recurrence = 'RRULE:FREQ=DAILY' WHERE id = ?1",
            [id],
        )
    })
    .await
    .unwrap();

    for scope in [Scope::This, Scope::All] {
        assert_invalid(
            s.service
                .update_event(single(id), scope, meeting(cal), "UTC")
                .await,
        );
        assert_invalid(s.service.delete_event(single(id), scope).await);
    }
    assert_eq!(s.count("events").await, 1);
}

// ---- deleting ------------------------------------------------------------------------------

#[tokio::test]
async fn deleting_an_event_removes_it_and_its_reminders() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = meeting(cal);
    d.reminders = vec![EventReminder::popup(10)];
    let keep = s
        .service
        .create_event(meeting(cal), "UTC")
        .await
        .unwrap()
        .event_id;
    let gone = s.service.create_event(d, "UTC").await.unwrap().event_id;
    while s.events.try_recv().is_ok() {}

    s.service
        .delete_event(single(gone), Scope::All)
        .await
        .unwrap();

    let left: Vec<i64> =
        s.db.read(|c| {
            c.prepare("SELECT id FROM events")?
                .query_map([], |r| r.get(0))?
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(left, vec![keep]);
    assert_eq!(s.count("event_reminders").await, 0);
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![cal]
        }
    );
}

// ---- editability (§4, §12) -----------------------------------------------------------------

/// Every combination of calendar access, `is_organizer` and `guests_can_modify`: a write is
/// refused with `NotEditable` exactly when the rule of §4 says the event is not editable.
#[tokio::test]
async fn every_write_follows_the_editability_rule() {
    for access in ["read", "write"] {
        for organizer in [false, true] {
            for guests in [false, true] {
                let mut s = setup();
                let cal = local(&mut s, "Home").await;
                let other = local(&mut s, "Other").await;
                s.set_access(cal, access).await;
                let id = s.raw_event(cal, organizer, guests).await;
                let editable = access == "write" && (organizer || guests);
                let case = format!("access {access}, organizer {organizer}, guests {guests}");

                assert_eq!(
                    s.service.get_event(id).await.unwrap().editable,
                    editable,
                    "{case}"
                );
                let week = s
                    .service
                    .list_occurrences("1970-01-01", "1970-01-02", "UTC")
                    .await
                    .unwrap();
                assert_eq!(week[0].editable, editable, "{case}");

                for scope in [Scope::This, Scope::All] {
                    let update = s
                        .service
                        .update_event(single(id), scope, meeting(cal), "UTC")
                        .await;
                    assert_eq!(is_not_editable(update), !editable, "update, {case}");
                }
                // Moving it elsewhere is a change too.
                let moved = s
                    .service
                    .update_event(single(id), Scope::All, meeting(other), "UTC")
                    .await;
                assert_eq!(is_not_editable(moved), !editable, "move, {case}");
                let delete = s.service.delete_event(single(id), Scope::All).await;
                assert_eq!(is_not_editable(delete), !editable, "delete, {case}");

                let left = s.count("events").await;
                assert_eq!(left, if editable { 0 } else { 1 }, "{case}");
            }
        }
    }
}

#[tokio::test]
async fn nothing_can_be_created_in_a_read_only_calendar() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    s.set_access(cal, "read").await;
    let feed = s.remote_calendar("ics", "read").await;
    for calendar in [cal, feed] {
        assert!(is_not_editable(
            s.service.create_event(meeting(calendar), "UTC").await
        ));
    }
    assert_eq!(s.count("events").await, 0);
    assert!(s.events.try_recv().is_err());
}

#[tokio::test]
async fn subscribed_and_google_events_cannot_be_changed() {
    let s = setup();
    for kind in ["ics", "google"] {
        let cal = s.remote_calendar(kind, "read").await;
        let id = s.raw_event(cal, true, true).await;
        assert!(is_not_editable(
            s.service
                .update_event(single(id), Scope::All, meeting(cal), "UTC")
                .await
        ));
        assert!(is_not_editable(
            s.service.delete_event(single(id), Scope::All).await
        ));
    }
}

/// Until the outbox (#18) exists, a write to a remote calendar would only change the local
/// copy, and the next sync would undo it. Google calendars are stored read-only until then
/// (§7.5), so this only guards against that changing without the outbox.
#[tokio::test]
async fn a_writable_remote_calendar_is_refused_until_the_outbox_exists() {
    let s = setup();
    let cal = s.remote_calendar("google", "write").await;
    let id = s.raw_event(cal, true, false).await;
    assert_invalid(s.service.create_event(meeting(cal), "UTC").await);
    assert_invalid(
        s.service
            .update_event(single(id), Scope::All, meeting(cal), "UTC")
            .await,
    );
    assert_invalid(s.service.delete_event(single(id), Scope::All).await);
    assert_eq!(s.count("events").await, 1);
}

#[test]
fn not_editable_serialises_with_its_kind() {
    let json = serde_json::to_value(ServiceError::NotEditable).unwrap();
    assert_eq!(json["kind"], "not_editable");
    assert!(json["message"].as_str().unwrap().len() > 10);
}

#[test]
fn drafts_and_details_use_the_frontend_shapes() {
    let d = draft(serde_json::json!({
        "calendar_id": 1, "title": "x", "all_day": false,
        "start": "2026-10-20T09:00", "end": "2026-10-20T10:00", "time_zone": "UTC",
        "reminders": [{"method": "popup", "minutes_before": 10}],
    }));
    assert_eq!(d.reminders, vec![EventReminder::popup(10)]);
    assert!(!d.use_default_reminders);
    let target: OccurrenceRef =
        serde_json::from_value(serde_json::json!({"event_id": 3, "original_start": 7})).unwrap();
    assert_eq!((target.event_id, target.original_start), (3, 7));
    let scope: Scope = serde_json::from_value(serde_json::json!("this")).unwrap();
    assert_eq!(scope, Scope::This);
    assert_eq!(
        serde_json::to_value(EventReminder {
            method: ReminderMethod::Email,
            minutes_before: 5
        })
        .unwrap(),
        serde_json::json!({"method": "email", "minutes_before": 5})
    );
}
