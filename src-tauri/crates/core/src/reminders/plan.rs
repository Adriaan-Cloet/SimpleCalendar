//! Which reminders are due, and the text a notification shows. Pure functions: the scheduler
//! passes in the time, so tests need no clock.

use std::collections::HashSet;

use chrono::{Days, NaiveDate};

use crate::model::{Occurrence, OccurrenceTime};

/// How long after its time a reminder is still shown although its event has started: the
/// scheduler wakes a moment late, and a reminder at the start (0 minutes before) is due exactly
/// when its event starts.
pub(crate) const GRACE: i64 = 60;

/// One reminder of one occurrence: a row of `fired_reminders` once it was shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    /// The occurrence's `event_id`: for a series, the master's.
    pub(crate) event_id: i64,
    /// The occurrence's start as an instant.
    pub(crate) start: i64,
    pub(crate) minutes_before: u32,
}

/// A reminder that may come due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) key: Key,
    /// `key.start` minus the reminder's minutes.
    pub(crate) fire_at: i64,
    pub(crate) occurrence: Occurrence,
}

/// What the scheduler does now and when it looks again.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The reminders to show now, earliest first, each once.
    pub(crate) due: Vec<Candidate>,
    /// When the next reminder that was not shown comes due, if one does.
    pub(crate) next: Option<i64>,
}

/// Splits `candidates` into the ones due at `now` and the time of the next one. A reminder is due
/// once its time has come, as long as its event has not started; a reminder missed while the app
/// was closed is skipped once its event is under way (§9). `fired` are the ones already shown.
pub(crate) fn plan(candidates: Vec<Candidate>, fired: &HashSet<Key>, now: i64) -> Plan {
    let mut plan = Plan::default();
    let mut seen = HashSet::new();
    for c in candidates {
        if fired.contains(&c.key) {
            continue;
        }
        if c.fire_at > now {
            plan.next = Some(plan.next.map_or(c.fire_at, |n| n.min(c.fire_at)));
        } else if (now < c.key.start || now < c.fire_at + GRACE) && seen.insert(c.key) {
            plan.due.push(c);
        }
    }
    plan.due
        .sort_by_key(|c| (c.fire_at, c.key.start, c.key.event_id, c.key.minutes_before));
    plan
}

/// When an occurrence starts, as a notification says it: "Today at 14:00", "Tomorrow at 09:00",
/// "Monday 26 October at 09:00", or "Today, all day". `start` is in the display zone and `today`
/// is the date there.
pub(crate) fn when(start: OccurrenceTime, today: NaiveDate) -> String {
    let (date, time) = match start {
        OccurrenceTime::Timed(t) => (t.date_naive(), Some(t.format("%H:%M").to_string())),
        OccurrenceTime::Date(d) => (d, None),
    };
    let day = if date == today {
        "Today".to_owned()
    } else if Some(date) == today.checked_add_days(Days::new(1)) {
        "Tomorrow".to_owned()
    } else {
        date.format("%A %-d %B").to_string()
    };
    match time {
        Some(time) => format!("{day} at {time}"),
        None => format!("{day}, all day"),
    }
}
