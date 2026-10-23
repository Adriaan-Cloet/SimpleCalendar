use std::collections::HashSet;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use chrono_tz::Europe::Brussels;
use rusqlite::{Connection, params};
use tempfile::TempDir;
use tokio::time::Instant;

use super::plan::{Candidate, GRACE, Key, plan, when};
use super::*;
use crate::db::migrations::test_connection;
use crate::model::{Occurrence, OccurrenceTime};

const MINUTE: i64 = 60;
const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

// ---- helpers -------------------------------------------------------------------------------

fn brussels(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Brussels
        .with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .unwrap()
        .timestamp()
}

fn day(y: i32, mo: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, mo, d).unwrap()
}

fn timed(at: i64) -> OccurrenceTime {
    OccurrenceTime::Timed(
        DateTime::from_timestamp(at, 0)
            .unwrap()
            .with_timezone(&Brussels)
            .fixed_offset(),
    )
}

/// A reminder of event `id` starting at `start`, `minutes` before.
fn candidate(id: i64, start: i64, minutes: u32) -> Candidate {
    Candidate {
        key: Key {
            event_id: id,
            start,
            minutes_before: minutes,
        },
        fire_at: start - i64::from(minutes) * MINUTE,
        occurrence: Occurrence {
            event_id: id,
            original_start: start,
            calendar_id: 1,
            title: format!("event {id}"),
            location: None,
            color: None,
            start: timed(start),
            end: timed(start + HOUR),
            editable: true,
        },
    }
}

fn keys(due: &[Candidate]) -> Vec<(i64, u32)> {
    due.iter()
        .map(|c| (c.key.event_id, c.key.minutes_before))
        .collect()
}

/// A local calendar. Returns its id.
fn add_calendar(c: &Connection, visible: bool, defaults: &str) -> i64 {
    c.execute(
        "INSERT INTO calendars (source_id, remote_id, name, access, visible, default_reminders)
         VALUES ('local', ?1, 'Calendar', 'write', ?2, ?3)",
        params![format!("cal-{visible}-{defaults}"), visible, defaults],
    )
    .unwrap();
    c.last_insert_rowid()
}

/// One row of `events`. Start from `Ev::new` and override what a test needs.
struct Ev<'a> {
    title: &'a str,
    start: i64,
    end: i64,
    tz: Option<&'a str>,
    all_day: bool,
    location: Option<&'a str>,
    recurrence: Option<&'a str>,
    /// `(master id, original start)` for an exception.
    exception_of: Option<(i64, i64)>,
    /// `None` for the calendar's defaults, else the event's own popup minutes.
    reminders: Option<&'a [u32]>,
}

impl<'a> Ev<'a> {
    fn new(title: &'a str, start: i64, reminders: &'a [u32]) -> Ev<'a> {
        Ev {
            title,
            start,
            end: start + HOUR,
            tz: Some("Europe/Brussels"),
            all_day: false,
            location: None,
            recurrence: None,
            exception_of: None,
            reminders: Some(reminders),
        }
    }

    fn insert(&self, c: &Connection, calendar_id: i64) -> i64 {
        c.execute(
            "INSERT INTO events (calendar_id, remote_id, title, location, all_day, start_utc,
                                 end_utc, start_tz, end_tz, recurrence, recurring_remote_id,
                                 recurring_event_id, original_start_utc, use_default_reminders)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                calendar_id,
                format!("{}-{}", self.title, self.start),
                self.title,
                self.location,
                self.all_day,
                self.start,
                self.end,
                self.tz,
                self.recurrence,
                self.exception_of.map(|_| "master"),
                self.exception_of.map(|(id, _)| id),
                self.exception_of.map(|(_, at)| at),
                self.reminders.is_none(),
            ],
        )
        .unwrap();
        let id = c.last_insert_rowid();
        for &m in self.reminders.unwrap_or_default() {
            c.execute(
                "INSERT INTO event_reminders (event_id, method, minutes_before)
                 VALUES (?1, 'popup', ?2)",
                params![id, m],
            )
            .unwrap();
        }
        id
    }
}

/// `(event id, start, minutes, fire time)` of every candidate, sorted.
fn found(c: &Connection, now: i64, zone: Tz) -> Vec<(i64, i64, u32, i64)> {
    let mut found: Vec<_> = store::candidates(c, now, now + HORIZON, zone)
        .unwrap()
        .into_iter()
        .map(|c| (c.key.event_id, c.key.start, c.key.minutes_before, c.fire_at))
        .collect();
    found.sort();
    found
}

// ---- plan ----------------------------------------------------------------------------------

#[test]
fn a_reminder_is_due_from_its_time_until_its_event_starts() {
    let start = 10 * HOUR;
    let early = candidate(1, start, 30);
    let at_start = candidate(2, start, 0);

    let before = plan(
        vec![early.clone(), at_start.clone()],
        &HashSet::new(),
        start - 31 * MINUTE,
    );
    assert!(before.due.is_empty());
    assert_eq!(before.next, Some(start - 30 * MINUTE));

    let at_time = plan(
        vec![early.clone(), at_start.clone()],
        &HashSet::new(),
        start - 30 * MINUTE,
    );
    assert_eq!(keys(&at_time.due), [(1, 30)]);
    assert_eq!(at_time.next, Some(start));

    // Still shown when the app opens 20 minutes later, the event not having started.
    let late = plan(vec![early.clone()], &HashSet::new(), start - 10 * MINUTE);
    assert_eq!(keys(&late.due), [(1, 30)]);
}

#[test]
fn a_missed_reminder_is_skipped_once_its_event_has_started() {
    let start = 10 * HOUR;
    let missed = plan(
        vec![candidate(1, start, 30)],
        &HashSet::new(),
        start + 5 * MINUTE,
    );
    assert!(missed.due.is_empty());
    assert_eq!(missed.next, None);

    // A reminder at the start is due for a moment after it, so a late wake still shows it.
    let at_start = plan(
        vec![candidate(2, start, 0)],
        &HashSet::new(),
        start + GRACE - 1,
    );
    assert_eq!(keys(&at_start.due), [(2, 0)]);
    let too_late = plan(vec![candidate(2, start, 0)], &HashSet::new(), start + GRACE);
    assert!(too_late.due.is_empty());
}

#[test]
fn a_shown_reminder_is_neither_due_nor_next() {
    let start = 10 * HOUR;
    let shown = candidate(1, start, 10);
    let later = candidate(1, start, 5);
    let fired = HashSet::from([shown.key, later.key]);

    let p = plan(vec![shown, later], &fired, start - 10 * MINUTE);
    assert!(p.due.is_empty());
    assert_eq!(p.next, None);
}

#[test]
fn due_reminders_come_earliest_first_and_each_once() {
    let now = 10 * HOUR;
    let p = plan(
        vec![
            candidate(1, now + 5 * MINUTE, 5),
            candidate(2, now + 20 * MINUTE, 30),
            candidate(1, now + 5 * MINUTE, 5),
            candidate(3, now + 10 * MINUTE, 15),
        ],
        &HashSet::new(),
        now,
    );
    assert_eq!(keys(&p.due), [(2, 30), (3, 15), (1, 5)]);
}

#[test]
fn the_text_says_today_tomorrow_or_the_date() {
    let today = day(2026, 10, 23);
    assert_eq!(
        when(timed(brussels(2026, 10, 23, 14, 0)), today),
        "Today at 14:00"
    );
    assert_eq!(
        when(timed(brussels(2026, 10, 24, 9, 5)), today),
        "Tomorrow at 09:05"
    );
    assert_eq!(
        when(timed(brussels(2026, 10, 26, 9, 0)), today),
        "Monday 26 October at 09:00"
    );
    assert_eq!(when(OccurrenceTime::Date(today), today), "Today, all day");
    assert_eq!(
        when(OccurrenceTime::Date(day(2026, 10, 24)), today),
        "Tomorrow, all day"
    );
    assert_eq!(
        when(OccurrenceTime::Date(day(2027, 1, 1)), today),
        "Friday 1 January, all day"
    );
}

// ---- store ---------------------------------------------------------------------------------

#[test]
fn candidates_are_the_popup_reminders_of_the_next_day() {
    let c = test_connection();
    let cal = add_calendar(&c, true, "[]");
    let now = brussels(2026, 10, 23, 8, 0);
    let soon = Ev::new("soon", now + 2 * HOUR, &[10, 60]).insert(&c, cal);
    let no_reminder = Ev::new("none", now + HOUR, &[]).insert(&c, cal);
    // A day and a half away: only its reminder of a day before falls in the next 24 hours.
    let later = Ev::new("later", now + 36 * HOUR, &[30, 1440]).insert(&c, cal);
    // Started an hour ago.
    Ev::new("started", now - HOUR, &[0]).insert(&c, cal);
    c.execute(
        "INSERT INTO event_reminders (event_id, method, minutes_before) VALUES (?1, 'email', 5)",
        [soon],
    )
    .unwrap();

    assert_eq!(
        found(&c, now, Brussels),
        [
            (soon, now + 2 * HOUR, 10, now + 2 * HOUR - 10 * MINUTE),
            (soon, now + 2 * HOUR, 60, now + HOUR),
            (later, now + 36 * HOUR, 1440, now + 12 * HOUR),
        ]
    );
    let _ = no_reminder;
}

#[test]
fn calendar_defaults_apply_when_the_event_uses_them() {
    let c = test_connection();
    let work = add_calendar(&c, true, "[60]");
    let hidden = add_calendar(&c, false, "[10]");
    let now = brussels(2026, 10, 23, 8, 0);
    let defaults = Ev {
        reminders: None,
        ..Ev::new("defaults", now + 2 * HOUR, &[])
    }
    .insert(&c, work);
    let own = Ev::new("own", now + 3 * HOUR, &[5]).insert(&c, work);
    Ev::new("hidden", now + 2 * HOUR, &[5]).insert(&c, hidden);

    assert_eq!(
        found(&c, now, Brussels),
        [
            (defaults, now + 2 * HOUR, 60, now + HOUR),
            (own, now + 3 * HOUR, 5, now + 3 * HOUR - 5 * MINUTE),
        ]
    );
}

#[test]
fn every_occurrence_of_a_series_has_reminders_and_an_exception_its_own() {
    let c = test_connection();
    let cal = add_calendar(&c, true, "[]");
    let now = brussels(2026, 10, 23, 6, 0);
    let first = brussels(2026, 10, 23, 9, 0);
    let master = Ev {
        recurrence: Some("RRULE:FREQ=HOURLY;INTERVAL=4"),
        ..Ev::new("series", first, &[15])
    }
    .insert(&c, cal);
    // The 13:00 occurrence moved to 14:00 with a reminder of its own.
    let moved = first + 4 * HOUR;
    Ev {
        exception_of: Some((master, moved)),
        ..Ev::new("moved", moved + HOUR, &[45])
    }
    .insert(&c, cal);

    let found = found(&c, now, Brussels);
    assert_eq!(
        found[..3],
        [
            (master, first, 15, first - 15 * MINUTE),
            (master, moved + HOUR, 45, moved + HOUR - 45 * MINUTE),
            (master, first + 8 * HOUR, 15, first + 8 * HOUR - 15 * MINUTE),
        ]
    );
    // One every four hours until the horizon, and the moved one.
    assert_eq!(found.len(), 6);
}

#[test]
fn an_all_day_reminder_falls_at_midnight_in_the_display_zone() {
    let c = test_connection();
    let cal = add_calendar(&c, true, "[]");
    // Stored as if in UTC: 24 October.
    let stored = day(2026, 10, 24)
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    let id = Ev {
        all_day: true,
        tz: None,
        end: stored + DAY,
        ..Ev::new("holiday", stored, &[0, 1440])
    }
    .insert(&c, cal);
    let now = brussels(2026, 10, 23, 12, 0);

    let midnight = brussels(2026, 10, 24, 0, 0);
    assert_eq!(
        found(&c, now, Brussels),
        [
            (id, midnight, 0, midnight),
            (id, midnight, 1440, midnight - DAY),
        ]
    );
    let midnight = New_York
        .with_ymd_and_hms(2026, 10, 24, 0, 0, 0)
        .unwrap()
        .timestamp();
    assert_eq!(found(&c, now, New_York)[0], (id, midnight, 0, midnight));
}

#[test]
fn a_reminder_across_a_dst_change_is_the_exact_number_of_minutes_before() {
    let c = test_connection();
    let cal = add_calendar(&c, true, "[]");
    // Clocks go back on 25 October 2026 at 03:00 in Brussels: that day lasts 25 hours.
    let start = brussels(2026, 10, 25, 9, 0);
    let id = Ev::new("after", start, &[1440]).insert(&c, cal);

    let found = found(&c, start - 25 * HOUR, Brussels);
    // 24 hours before is 10:00 the day before on the wall clock, not 09:00.
    assert_eq!(found, [(id, start, 1440, brussels(2026, 10, 24, 10, 0))]);
}

#[test]
fn a_reminder_is_recorded_once_and_pruned_after_a_week() {
    let c = test_connection();
    let cal = add_calendar(&c, true, "[]");
    let now = brussels(2026, 10, 23, 8, 0);
    let id = Ev::new("a", now + HOUR, &[60]).insert(&c, cal);
    let key = Key {
        event_id: id,
        start: now + HOUR,
        minutes_before: 60,
    };

    assert!(store::fire(&c, key, now).unwrap());
    assert!(!store::fire(&c, key, now + 1).unwrap());
    assert_eq!(store::fired(&c).unwrap(), HashSet::from([key]));
    // An event deleted since the read is not shown.
    let gone = Key {
        event_id: id + 100,
        ..key
    };
    assert!(!store::fire(&c, gone, now).unwrap());

    assert_eq!(store::prune(&c, now + 7 * DAY).unwrap(), 0);
    assert_eq!(store::prune(&c, now + 7 * DAY + 1).unwrap(), 1);
    assert!(store::fired(&c).unwrap().is_empty());
}

#[test]
fn the_display_zone_is_remembered() {
    let c = test_connection();
    assert_eq!(store::display_zone(&c).unwrap(), None);
    assert!(store::set_display_zone(&c, Brussels).unwrap());
    assert!(!store::set_display_zone(&c, Brussels).unwrap());
    assert_eq!(store::display_zone(&c).unwrap(), Some(Brussels));
    assert!(store::set_display_zone(&c, New_York).unwrap());
    assert_eq!(store::display_zone(&c).unwrap(), Some(New_York));
}

// ---- scheduler -----------------------------------------------------------------------------

/// A database file and a scheduler on a clock that starts at `base` and moves with Tokio's
/// paused time.
struct Harness {
    db: Db,
    handle: ReminderHandle,
    rx: mpsc::UnboundedReceiver<CoreEvent>,
    tx: mpsc::UnboundedSender<CoreEvent>,
    base: i64,
    started: Instant,
    task: Option<tokio::task::JoinHandle<()>>,
    calendar: i64,
    _dir: TempDir,
}

impl Harness {
    async fn new(base: i64) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.db")).unwrap();
        let calendar = db
            .write(|c| {
                set_display_zone(c, Brussels)?;
                Ok(add_calendar(c, true, "[]"))
            })
            .await
            .unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        Harness {
            db,
            handle: ReminderHandle::default(),
            rx,
            tx,
            base,
            started: Instant::now(),
            task: None,
            calendar,
            _dir: dir,
        }
    }

    fn now(&self) -> i64 {
        self.base + self.started.elapsed().as_secs() as i64
    }

    fn start(&mut self) {
        let (base, started) = (self.base, self.started);
        let clock: Clock = Arc::new(move || base + started.elapsed().as_secs() as i64);
        let scheduler =
            ReminderScheduler::with_clock(self.db.clone(), self.tx.clone(), &self.handle, clock);
        self.task = Some(tokio::spawn(scheduler.run()));
    }

    /// Stops the scheduler, as quitting the app does, and starts a new one.
    async fn restart(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
        self.start();
    }

    async fn add(&self, ev: Ev<'static>) -> i64 {
        let calendar = self.calendar;
        self.db
            .write(move |c| Ok(ev.insert(c, calendar)))
            .await
            .unwrap()
    }

    /// The next reminder shown within `limit`, and when, or `None`.
    async fn next_within(&mut self, limit: Duration) -> Option<(ReminderDue, i64)> {
        let deadline = Instant::now() + limit;
        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(CoreEvent::ReminderDue(r))) => return Some((r, self.now())),
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => return None,
            }
        }
    }

    async fn next(&mut self) -> (ReminderDue, i64) {
        self.next_within(Duration::from_secs(2 * DAY as u64))
            .await
            .expect("a reminder")
    }
}

#[tokio::test(start_paused = true)]
async fn a_reminder_is_shown_at_its_time_and_once() {
    let base = brussels(2026, 10, 23, 8, 0);
    let mut h = Harness::new(base).await;
    let start = base + 2 * HOUR;
    let id = h
        .add(Ev {
            location: Some("Room 4"),
            ..Ev::new("Standup", start, &[10])
        })
        .await;
    h.start();

    let (due, at) = h.next().await;
    assert!(
        (start - 10 * MINUTE..start - 10 * MINUTE + 30).contains(&at),
        "shown {}s before the start",
        start - at
    );
    assert_eq!(
        due,
        ReminderDue {
            event_id: id,
            original_start: start,
            title: "Standup".into(),
            body: "Today at 10:00\nRoom 4".into(),
            date: day(2026, 10, 23),
        }
    );
    assert_eq!(h.next_within(Duration::from_secs(DAY as u64)).await, None);
}

#[tokio::test(start_paused = true)]
async fn a_restart_does_not_show_a_reminder_again() {
    let base = brussels(2026, 10, 23, 8, 0);
    let mut h = Harness::new(base).await;
    h.add(Ev::new("a", base + 20 * MINUTE, &[30])).await;
    h.start();
    assert_eq!(h.next().await.0.title, "a");

    h.restart().await;
    assert_eq!(h.next_within(Duration::from_secs(HOUR as u64)).await, None);
}

#[tokio::test(start_paused = true)]
async fn reminders_missed_while_closed_show_only_for_events_not_started() {
    let base = brussels(2026, 10, 23, 8, 0);
    let mut h = Harness::new(base).await;
    // Both reminders were due an hour ago; only the first event is still to come.
    h.add(Ev::new("coming", base + 30 * MINUTE, &[90])).await;
    h.add(Ev::new("started", base - 10 * MINUTE, &[50])).await;
    h.start();

    let (due, at) = h.next().await;
    assert_eq!(due.title, "coming");
    assert!(at - base < 5, "shown at start");
    assert_eq!(h.next_within(Duration::from_secs(DAY as u64)).await, None);
}

#[tokio::test(start_paused = true)]
async fn a_change_while_sleeping_is_picked_up() {
    let base = brussels(2026, 10, 23, 8, 0);
    let mut h = Harness::new(base).await;
    h.add(Ev::new("late", base + 10 * HOUR, &[0])).await;
    h.start();
    tokio::time::sleep(Duration::from_secs(60)).await;

    // An event added now with a reminder before the one the scheduler sleeps towards.
    let start = h.now() + HOUR;
    h.add(Ev::new("new", start, &[30])).await;
    h.handle.changed();

    let (due, at) = h.next().await;
    assert_eq!(due.title, "new");
    assert!((start - 30 * MINUTE..start - 30 * MINUTE + 30).contains(&at));
}

#[tokio::test(start_paused = true)]
async fn nothing_is_shown_before_a_display_zone_is_known() {
    let base = brussels(2026, 10, 23, 8, 0);
    let mut h = Harness::new(base).await;
    h.db.write(|c| c.execute("DELETE FROM settings WHERE key = 'display_zone'", []))
        .await
        .unwrap();
    h.add(Ev::new("a", base + 3 * HOUR, &[30])).await;
    h.start();
    assert_eq!(h.next_within(Duration::from_secs(HOUR as u64)).await, None);

    h.db.write(|c| set_display_zone(c, Brussels)).await.unwrap();
    h.handle.changed();
    assert_eq!(h.next().await.0.title, "a");
}
