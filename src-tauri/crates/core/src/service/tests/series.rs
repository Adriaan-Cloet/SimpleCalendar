//! Repeating events: "only this event" and "all events" (#17, `docs/architecture.md` §4, §10).
//!
//! Most tests use a weekly standup on Tuesdays at 09:00 in Brussels, from 20 October 2026.
//! Summer time ends on Sunday 25 October, so the first occurrence is at 07:00 UTC and the later
//! ones at 08:00 UTC: the same wall-clock time.

use chrono_tz::Europe::Brussels;
use chrono_tz::UTC;

use super::events::{assert_invalid, draft, is_not_editable, local, meeting};
use super::*;

// ---- helpers -------------------------------------------------------------------------------

/// A stored instant of a Brussels wall-clock time in 2026.
fn at(mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    instant(Brussels, 2026, mo, d, h, mi)
}

fn repeat(frequency: RepeatFrequency, until: Option<&str>) -> Option<Repeat> {
    Some(Repeat {
        frequency,
        until: until.map(str::to_owned),
    })
}

/// The weekly standup, without an end.
fn standup(calendar: i64) -> EventDraft {
    let mut d = draft(serde_json::json!({
        "calendar_id": calendar,
        "title": "Standup",
        "all_day": false,
        "start": "2026-10-20T09:00",
        "end": "2026-10-20T09:30",
        "time_zone": "Europe/Brussels",
    }));
    d.repeat = repeat(RepeatFrequency::Weekly, None);
    d
}

/// The standup's draft for the occurrence on `date`, unchanged.
fn standup_on(calendar: i64, date: &str) -> EventDraft {
    moved(
        standup(calendar),
        &format!("{date}T09:00"),
        &format!("{date}T09:30"),
    )
}

/// `draft` with other wall-clock times, in the same zone.
fn moved(mut draft: EventDraft, start: &str, end: &str) -> EventDraft {
    draft.start = start.into();
    draft.end = end.into();
    draft
}

fn titled(mut draft: EventDraft, title: &str) -> EventDraft {
    draft.title = title.into();
    draft
}

fn occurrence(event_id: i64, original_start: i64) -> OccurrenceRef {
    OccurrenceRef {
        event_id,
        original_start,
    }
}

/// A wall-clock start as the views show it, in Brussels.
fn wall(time: &OccurrenceTime) -> String {
    match time {
        OccurrenceTime::Timed(t) => t
            .with_timezone(&Brussels)
            .format("%Y-%m-%dT%H:%M")
            .to_string(),
        OccurrenceTime::Date(d) => d.to_string(),
    }
}

/// One exception row: `original_start_utc`, `status`, `title`, `calendar_id`.
type Exception = (i64, String, String, i64);

impl Setup {
    /// What a viewer in Brussels sees from 19 October to 30 November 2026, as "title start".
    async fn shown(&self) -> Vec<String> {
        self.shown_between("2026-10-19", "2026-11-30").await
    }

    async fn shown_between(&self, from: &str, to: &str) -> Vec<String> {
        self.service
            .list_occurrences(from, to, "Europe/Brussels")
            .await
            .unwrap()
            .iter()
            .map(|o| format!("{} {}", o.title, wall(&o.start)))
            .collect()
    }

    /// The exceptions of series `master`, by original start.
    async fn exceptions(&self, master: i64) -> Vec<Exception> {
        self.db
            .read(move |c| {
                c.prepare(
                    "SELECT original_start_utc, status, title, calendar_id FROM events
                     WHERE recurring_event_id = ?1 ORDER BY original_start_utc",
                )?
                .query_map([master], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect()
            })
            .await
            .unwrap()
    }

    async fn recurrence(&self, id: i64) -> Option<String> {
        self.db
            .read(move |c| {
                c.query_row("SELECT recurrence FROM events WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
            })
            .await
            .unwrap()
    }

    /// Creates the weekly standup in a new local calendar. Returns the calendar and the series.
    async fn standup(&mut self) -> (i64, i64) {
        let cal = local(self, "Work").await;
        let id = self
            .service
            .create_event(standup(cal), "Europe/Brussels")
            .await
            .unwrap()
            .event_id;
        (cal, id)
    }

    /// Changes one occurrence of `series` and returns what `update_event` returned.
    async fn change(
        &self,
        series: i64,
        original_start: i64,
        scope: Scope,
        draft: EventDraft,
    ) -> Option<Occurrence> {
        self.service
            .update_event(
                occurrence(series, original_start),
                scope,
                draft,
                "Europe/Brussels",
            )
            .await
            .unwrap()
    }
}

// ---- creating and reading ------------------------------------------------------------------

#[tokio::test]
async fn a_repeating_event_keeps_its_time_across_daylight_saving() {
    let mut s = setup();
    let cal = local(&mut s, "Work").await;
    let mut d = standup(cal);
    d.repeat = repeat(RepeatFrequency::Weekly, Some("2026-11-03"));

    let created = s.service.create_event(d, "Europe/Brussels").await.unwrap();

    assert_eq!(created.original_start, at(10, 20, 9, 0));
    assert_eq!(
        s.recurrence(created.event_id).await.as_deref(),
        Some("RRULE:FREQ=WEEKLY;UNTIL=20261103T225959Z")
    );
    assert_eq!(
        s.shown().await,
        vec![
            "Standup 2026-10-20T09:00",
            "Standup 2026-10-27T09:00",
            "Standup 2026-11-03T09:00",
        ]
    );
}

#[tokio::test]
async fn an_occurrence_reads_with_its_own_date_and_the_series_rule() {
    let mut s = setup();
    let cal = local(&mut s, "Work").await;
    let mut d = standup(cal);
    d.repeat = repeat(RepeatFrequency::Weekly, Some("2026-12-31"));
    let id = s.service.create_event(d, "UTC").await.unwrap().event_id;

    let event = s
        .service
        .get_event(occurrence(id, at(11, 3, 9, 0)))
        .await
        .unwrap();

    assert_eq!(event.id, id);
    assert_eq!(event.start, "2026-11-03T09:00");
    assert_eq!(event.end, "2026-11-03T09:30");
    assert_eq!(
        event.repeat,
        repeat(RepeatFrequency::Weekly, Some("2026-12-31"))
    );
}

#[tokio::test]
async fn an_all_day_series_repeats_by_date() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = draft(serde_json::json!({
        "calendar_id": cal, "title": "Christmas", "all_day": true,
        "start": "2026-12-25", "end": "2026-12-26",
    }));
    d.repeat = repeat(RepeatFrequency::Yearly, Some("2028-12-25"));
    let id = s
        .service
        .create_event(d, "America/New_York")
        .await
        .unwrap()
        .event_id;

    assert_eq!(
        s.recurrence(id).await.as_deref(),
        Some("RRULE:FREQ=YEARLY;UNTIL=20281225")
    );
    assert_eq!(
        s.shown_between("2026-01-01", "2027-01-31").await,
        vec!["Christmas 2026-12-25"]
    );
    let next = s
        .service
        .get_event(occurrence(id, instant(UTC, 2027, 12, 25, 0, 0)))
        .await
        .unwrap();
    assert_eq!(
        (next.start.as_str(), next.end.as_str()),
        ("2027-12-25", "2027-12-26")
    );
}

#[tokio::test]
async fn a_rule_the_editor_cannot_show_reads_as_custom() {
    let mut s = setup();
    let (_, id) = s.standup().await;
    s.db.write(move |c| {
        c.execute(
            "UPDATE events SET recurrence = 'RRULE:FREQ=WEEKLY;BYDAY=TU,TH' WHERE id = ?1",
            [id],
        )
    })
    .await
    .unwrap();

    let event = s
        .service
        .get_event(occurrence(id, at(10, 22, 9, 0)))
        .await
        .unwrap();
    assert_eq!(event.repeat, repeat(RepeatFrequency::Custom, None));

    // Saved with "custom", the rule stays as it was.
    let mut d = titled(
        moved(
            standup(event.calendar_id),
            "2026-10-22T09:00",
            "2026-10-22T09:30",
        ),
        "Sync",
    );
    d.repeat = repeat(RepeatFrequency::Custom, None);
    s.change(id, at(10, 22, 9, 0), Scope::All, d).await;
    assert_eq!(
        s.recurrence(id).await.as_deref(),
        Some("RRULE:FREQ=WEEKLY;BYDAY=TU,TH")
    );
    assert_eq!(
        &s.shown().await[..2],
        ["Sync 2026-10-20T09:00", "Sync 2026-10-22T09:00"]
    );
}

#[tokio::test]
async fn a_repeat_must_end_after_it_starts_and_be_one_the_editor_offers() {
    let mut s = setup();
    let cal = local(&mut s, "Work").await;

    let mut early = standup(cal);
    early.repeat = repeat(RepeatFrequency::Weekly, Some("2026-10-19"));
    assert_invalid(s.service.create_event(early, "UTC").await);

    let mut unreadable = standup(cal);
    unreadable.repeat = repeat(RepeatFrequency::Weekly, Some("next year"));
    assert_invalid(s.service.create_event(unreadable, "UTC").await);

    // "Custom" keeps an existing rule; a new event has none.
    let mut custom = standup(cal);
    custom.repeat = repeat(RepeatFrequency::Custom, None);
    assert_invalid(s.service.create_event(custom, "UTC").await);

    assert_eq!(s.count("events").await, 0);
}

#[tokio::test]
async fn a_time_that_is_not_an_occurrence_is_not_found() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    for wrong in [at(10, 21, 9, 0), at(10, 27, 9, 30), 0] {
        let target = occurrence(id, wrong);
        let not_found = |r: &Result<_, ServiceError>| matches!(r, Err(ServiceError::NotFound(_)));
        assert!(not_found(&s.service.get_event(target).await.map(|_| ())));
        assert!(not_found(
            &s.service
                .update_event(target, Scope::This, standup(cal), "UTC")
                .await
                .map(|_| ())
        ));
        assert!(not_found(
            &s.service.delete_event(target, Scope::This).await
        ));
    }
    assert!(s.exceptions(id).await.is_empty());
}

// ---- only this event -----------------------------------------------------------------------

#[tokio::test]
async fn changing_one_occurrence_makes_an_exception() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    while s.events.try_recv().is_ok() {}

    let returned = s
        .change(
            id,
            at(11, 3, 9, 0),
            Scope::This,
            titled(
                moved(standup(cal), "2026-11-04T11:00", "2026-11-04T12:00"),
                "Retro",
            ),
        )
        .await
        .unwrap();

    assert_eq!(returned.event_id, id);
    assert_eq!(returned.original_start, at(11, 3, 9, 0));
    assert_eq!(wall(&returned.start), "2026-11-04T11:00");
    assert_eq!(
        s.shown().await,
        vec![
            "Standup 2026-10-20T09:00",
            "Standup 2026-10-27T09:00",
            "Retro 2026-11-04T11:00",
            "Standup 2026-11-10T09:00",
            "Standup 2026-11-17T09:00",
            "Standup 2026-11-24T09:00",
        ]
    );
    assert_eq!(
        s.exceptions(id).await,
        vec![(at(11, 3, 9, 0), "confirmed".into(), "Retro".into(), cal)]
    );
    let event = s
        .service
        .get_event(occurrence(id, at(11, 3, 9, 0)))
        .await
        .unwrap();
    assert_eq!(event.id, id);
    assert_eq!(event.title, "Retro");
    assert_eq!(event.start, "2026-11-04T11:00");
    assert_eq!(event.repeat, repeat(RepeatFrequency::Weekly, None));
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![cal]
        }
    );
}

#[tokio::test]
async fn changing_an_exception_again_updates_it() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    let nov3 = at(11, 3, 9, 0);
    s.change(
        id,
        nov3,
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;
    s.change(
        id,
        nov3,
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Planning"),
    )
    .await;

    let exceptions = s.exceptions(id).await;
    assert_eq!(exceptions.len(), 1);
    assert_eq!(exceptions[0].2, "Planning");
}

#[tokio::test]
async fn an_exception_keeps_its_own_reminders() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    let mut d = standup_on(cal, "2026-11-03");
    d.reminders = vec![EventReminder::popup(30)];
    s.change(id, at(11, 3, 9, 0), Scope::This, d).await;

    let one = s.service.get_event(occurrence(id, at(11, 3, 9, 0))).await;
    let other = s.service.get_event(occurrence(id, at(11, 10, 9, 0))).await;
    assert_eq!(one.unwrap().reminders, vec![EventReminder::popup(30)]);
    assert!(other.unwrap().reminders.is_empty());
}

#[tokio::test]
async fn deleting_one_occurrence_cancels_it() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    while s.events.try_recv().is_ok() {}

    s.service
        .delete_event(occurrence(id, at(10, 27, 9, 0)), Scope::This)
        .await
        .unwrap();
    // The first occurrence is the master's own start, and can go as well.
    s.service
        .delete_event(occurrence(id, at(10, 20, 9, 0)), Scope::This)
        .await
        .unwrap();

    assert_eq!(
        &s.shown().await[..2],
        ["Standup 2026-11-03T09:00", "Standup 2026-11-10T09:00"]
    );
    let statuses: Vec<_> = s
        .exceptions(id)
        .await
        .into_iter()
        .map(|e| (e.0, e.1))
        .collect();
    assert_eq!(
        statuses,
        vec![
            (at(10, 20, 9, 0), "cancelled".to_owned()),
            (at(10, 27, 9, 0), "cancelled".to_owned()),
        ]
    );
    let gone = s.service.get_event(occurrence(id, at(10, 27, 9, 0))).await;
    assert!(matches!(gone, Err(ServiceError::NotFound(_))), "{gone:?}");
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![cal]
        }
    );
}

#[tokio::test]
async fn deleting_a_changed_occurrence_cancels_its_exception() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    let nov3 = at(11, 3, 9, 0);
    s.change(
        id,
        nov3,
        Scope::This,
        titled(
            moved(standup(cal), "2026-11-04T11:00", "2026-11-04T12:00"),
            "Retro",
        ),
    )
    .await;

    s.service
        .delete_event(occurrence(id, nov3), Scope::This)
        .await
        .unwrap();

    let shown = s.shown().await;
    assert!(!shown.iter().any(|o| o.starts_with("Retro")), "{shown:?}");
    assert!(!shown.contains(&"Standup 2026-11-03T09:00".to_owned()));
    assert_eq!(
        s.exceptions(id).await,
        vec![(nov3, "cancelled".into(), "Retro".into(), cal)]
    );
}

#[tokio::test]
async fn only_all_events_can_change_the_repeat_or_the_calendar() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    let other = local(&mut s, "Home").await;

    let mut daily = standup(cal);
    daily.repeat = repeat(RepeatFrequency::Daily, None);
    let mut elsewhere = standup(other);
    elsewhere.title = "Elsewhere".into();
    let mut none = standup(cal);
    none.repeat = None;
    for d in [daily, elsewhere, none] {
        assert_invalid(
            s.service
                .update_event(occurrence(id, at(11, 3, 9, 0)), Scope::This, d, "UTC")
                .await,
        );
    }
    assert!(s.exceptions(id).await.is_empty());
}

#[tokio::test]
async fn one_day_of_an_all_day_series_can_change() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = draft(serde_json::json!({
        "calendar_id": cal, "title": "Holiday", "all_day": true,
        "start": "2026-10-19", "end": "2026-10-20",
    }));
    d.repeat = repeat(RepeatFrequency::Daily, Some("2026-10-21"));
    let id = s
        .service
        .create_event(d.clone(), "UTC")
        .await
        .unwrap()
        .event_id;

    s.change(
        id,
        instant(UTC, 2026, 10, 20, 0, 0),
        Scope::This,
        titled(moved(d, "2026-10-20", "2026-10-21"), "Day off"),
    )
    .await;

    assert_eq!(
        s.shown().await,
        vec![
            "Holiday 2026-10-19",
            "Day off 2026-10-20",
            "Holiday 2026-10-21"
        ]
    );
}

#[tokio::test]
async fn one_occurrence_on_the_night_summer_time_starts() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let mut d = draft(serde_json::json!({
        "calendar_id": cal, "title": "Backup", "all_day": false,
        "start": "2026-03-28T02:30", "end": "2026-03-28T02:45",
        "time_zone": "Europe/Brussels",
    }));
    d.repeat = repeat(RepeatFrequency::Daily, Some("2026-03-30"));
    let id = s.service.create_event(d, "UTC").await.unwrap().event_id;
    // 02:30 does not exist on 29 March and becomes 03:30 (RFC 5545 §3.3.5).
    let gap_day = instant(Brussels, 2026, 3, 29, 3, 30);
    assert_eq!(
        s.shown_between("2026-03-28", "2026-03-31").await,
        vec![
            "Backup 2026-03-28T02:30",
            "Backup 2026-03-29T03:30",
            "Backup 2026-03-30T02:30"
        ]
    );

    let event = s.service.get_event(occurrence(id, gap_day)).await.unwrap();
    assert_eq!(event.start, "2026-03-29T03:30");
    s.service
        .delete_event(occurrence(id, gap_day), Scope::This)
        .await
        .unwrap();

    assert_eq!(
        s.shown_between("2026-03-28", "2026-03-31").await,
        vec!["Backup 2026-03-28T02:30", "Backup 2026-03-30T02:30"]
    );
}

// ---- all events ----------------------------------------------------------------------------

#[tokio::test]
async fn changing_all_events_keeps_exceptions_linked() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;
    s.service
        .delete_event(occurrence(id, at(10, 27, 9, 0)), Scope::This)
        .await
        .unwrap();

    let returned = s
        .change(
            id,
            at(11, 10, 9, 0),
            Scope::All,
            titled(
                moved(standup(cal), "2026-11-10T09:00", "2026-11-10T09:30"),
                "Sync",
            ),
        )
        .await
        .unwrap();

    assert_eq!(returned.original_start, at(11, 10, 9, 0));
    assert_eq!(
        s.shown().await,
        vec![
            "Sync 2026-10-20T09:00",
            "Retro 2026-11-03T09:00",
            "Sync 2026-11-10T09:00",
            "Sync 2026-11-17T09:00",
            "Sync 2026-11-24T09:00",
        ]
    );
    assert_eq!(s.exceptions(id).await.len(), 2);
}

#[tokio::test]
async fn an_exception_follows_the_fields_it_did_not_change() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    // Only the time of 3 November changes; its title and length are still the series'.
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        moved(standup(cal), "2026-11-03T11:00", "2026-11-03T11:30"),
    )
    .await;

    let mut d = titled(
        moved(standup(cal), "2026-10-20T09:00", "2026-10-20T10:00"),
        "Sync",
    );
    d.location = Some("Room 1".into());
    s.change(id, at(10, 20, 9, 0), Scope::All, d).await;

    let event = s
        .service
        .get_event(occurrence(id, at(11, 3, 9, 0)))
        .await
        .unwrap();
    assert_eq!(event.title, "Sync");
    assert_eq!(event.location.as_deref(), Some("Room 1"));
    assert_eq!(
        (event.start.as_str(), event.end.as_str()),
        ("2026-11-03T11:00", "2026-11-03T12:00")
    );
}

#[tokio::test]
async fn moving_all_events_moves_the_series_and_its_exceptions() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        moved(standup(cal), "2026-11-03T11:00", "2026-11-03T11:30"),
    )
    .await;
    s.service
        .delete_event(occurrence(id, at(10, 27, 9, 0)), Scope::This)
        .await
        .unwrap();

    // One hour later, chosen on the first occurrence, before summer time ends.
    let returned = s
        .change(
            id,
            at(10, 20, 9, 0),
            Scope::All,
            moved(standup(cal), "2026-10-20T10:00", "2026-10-20T10:30"),
        )
        .await
        .unwrap();

    assert_eq!(returned.original_start, at(10, 20, 10, 0));
    assert_eq!(
        s.shown().await,
        vec![
            "Standup 2026-10-20T10:00",
            "Standup 2026-11-03T12:00",
            "Standup 2026-11-10T10:00",
            "Standup 2026-11-17T10:00",
            "Standup 2026-11-24T10:00",
        ]
    );
    let originals: Vec<_> = s.exceptions(id).await.into_iter().map(|e| e.0).collect();
    assert_eq!(originals, vec![at(10, 27, 10, 0), at(11, 3, 10, 0)]);
}

#[tokio::test]
async fn moving_all_events_to_another_day() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;

    // Chosen on an occurrence after summer time ended: Tuesday becomes Wednesday.
    s.change(
        id,
        at(11, 10, 9, 0),
        Scope::All,
        moved(standup(cal), "2026-11-11T09:00", "2026-11-11T09:30"),
    )
    .await;

    assert_eq!(
        &s.shown().await[..3],
        [
            "Standup 2026-10-21T09:00",
            "Standup 2026-10-28T09:00",
            "Retro 2026-11-04T09:00"
        ]
    );
}

#[tokio::test]
async fn all_events_chosen_on_a_moved_occurrence_move_by_the_same_amount() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    let nov3 = at(11, 3, 9, 0);
    s.change(
        id,
        nov3,
        Scope::This,
        moved(standup(cal), "2026-11-03T11:00", "2026-11-03T11:30"),
    )
    .await;

    // Half an hour later than where 3 November was moved to.
    s.change(
        id,
        nov3,
        Scope::All,
        moved(standup(cal), "2026-11-03T11:30", "2026-11-03T12:00"),
    )
    .await;

    assert_eq!(
        &s.shown().await[..3],
        [
            "Standup 2026-10-20T09:30",
            "Standup 2026-10-27T09:30",
            "Standup 2026-11-03T11:30"
        ]
    );
}

#[tokio::test]
async fn changing_how_all_events_repeat_drops_the_exceptions() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;
    s.service
        .delete_event(occurrence(id, at(10, 27, 9, 0)), Scope::This)
        .await
        .unwrap();

    let mut d = standup(cal);
    d.repeat = repeat(RepeatFrequency::Daily, Some("2026-10-22"));
    s.change(id, at(10, 20, 9, 0), Scope::All, d).await;

    assert!(s.exceptions(id).await.is_empty());
    assert_eq!(
        s.shown().await,
        vec![
            "Standup 2026-10-20T09:00",
            "Standup 2026-10-21T09:00",
            "Standup 2026-10-22T09:00"
        ]
    );
}

#[tokio::test]
async fn an_edit_that_leaves_nothing_at_that_time_returns_none() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    let mut d = moved(standup(cal), "2026-11-10T09:00", "2026-11-10T09:30");
    d.repeat = repeat(RepeatFrequency::Monthly, None);

    // Monthly from 20 October: 10 November is no longer an occurrence.
    assert_eq!(s.change(id, at(11, 10, 9, 0), Scope::All, d).await, None);
    assert_eq!(
        s.shown().await,
        vec!["Standup 2026-10-20T09:00", "Standup 2026-11-20T09:00"]
    );
}

#[tokio::test]
async fn making_all_events_all_day_drops_the_exceptions() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;

    let mut d = standup(cal);
    d.all_day = true;
    d.start = "2026-11-03".into();
    d.end = "2026-11-04".into();
    d.time_zone = None;
    let returned = s.change(id, at(11, 3, 9, 0), Scope::All, d).await.unwrap();

    assert!(s.exceptions(id).await.is_empty());
    assert_eq!(
        returned.start,
        OccurrenceTime::Date("2026-11-03".parse().unwrap())
    );
    assert_eq!(
        &s.shown().await[..2],
        ["Standup 2026-10-20", "Standup 2026-10-27"]
    );
    assert_eq!(s.recurrence(id).await.as_deref(), Some("RRULE:FREQ=WEEKLY"));
}

#[tokio::test]
async fn ending_the_repeat_of_all_events_leaves_the_event_in_the_form() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;

    let mut d = moved(standup(cal), "2026-11-10T14:00", "2026-11-10T15:00");
    d.repeat = None;
    let returned = s.change(id, at(11, 10, 9, 0), Scope::All, d).await.unwrap();

    assert_eq!(returned.event_id, id);
    assert_eq!(returned.original_start, at(11, 10, 14, 0));
    assert_eq!(s.recurrence(id).await, None);
    assert!(s.exceptions(id).await.is_empty());
    assert_eq!(s.shown().await, vec!["Standup 2026-11-10T14:00"]);
}

#[tokio::test]
async fn a_single_event_can_start_repeating() {
    let mut s = setup();
    let cal = local(&mut s, "Home").await;
    let id = s
        .service
        .create_event(meeting(cal), "UTC")
        .await
        .unwrap()
        .event_id;

    let mut d = meeting(cal);
    d.repeat = repeat(RepeatFrequency::Weekly, Some("2026-10-27"));
    // A single event ignores the scope and its original start.
    s.service
        .update_event(occurrence(id, 0), Scope::This, d, "UTC")
        .await
        .unwrap();

    assert_eq!(
        s.shown().await,
        vec!["Meeting 2026-10-20T09:00", "Meeting 2026-10-27T09:00"]
    );
}

#[tokio::test]
async fn all_events_move_to_another_calendar_together() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    let other = local(&mut s, "Home").await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;
    while s.events.try_recv().is_ok() {}

    s.change(id, at(10, 20, 9, 0), Scope::All, standup(other))
        .await;

    assert_eq!(s.exceptions(id).await[0].3, other);
    let calendars: Vec<i64> = s
        .service
        .list_occurrences("2026-10-19", "2026-11-30", "UTC")
        .await
        .unwrap()
        .iter()
        .map(|o| o.calendar_id)
        .collect();
    assert!(calendars.iter().all(|&c| c == other), "{calendars:?}");
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![cal, other]
        }
    );
}

#[tokio::test]
async fn deleting_all_events_removes_the_series_and_its_exceptions() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.change(
        id,
        at(11, 3, 9, 0),
        Scope::This,
        titled(standup_on(cal, "2026-11-03"), "Retro"),
    )
    .await;
    s.service
        .delete_event(occurrence(id, at(10, 27, 9, 0)), Scope::This)
        .await
        .unwrap();

    s.service
        .delete_event(occurrence(id, at(11, 10, 9, 0)), Scope::All)
        .await
        .unwrap();

    assert_eq!(s.count("events").await, 0);
    assert!(s.shown().await.is_empty());
}

// ---- editability (§4) ----------------------------------------------------------------------

#[tokio::test]
async fn a_series_that_is_not_editable_refuses_every_scope() {
    let mut s = setup();
    let (cal, id) = s.standup().await;
    s.db.write(move |c| c.execute("UPDATE events SET is_organizer = 0 WHERE id = ?1", [id]))
        .await
        .unwrap();

    let target = occurrence(id, at(11, 3, 9, 0));
    for scope in [Scope::This, Scope::All] {
        assert!(is_not_editable(
            s.service
                .update_event(target, scope, standup(cal), "UTC")
                .await
        ));
        assert!(is_not_editable(s.service.delete_event(target, scope).await));
    }
    assert!(s.exceptions(id).await.is_empty());
    assert!(!s.service.get_event(target).await.unwrap().editable);
}
