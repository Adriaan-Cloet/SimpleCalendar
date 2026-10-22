//! "Only this event" and "all events" for a series (#17, `docs/architecture.md` §4, §10).
//!
//! **Only this event** writes an exception: a row that names its series and the occurrence it
//! replaces (`original_start_utc`), with the values from the form. Deleting one occurrence
//! makes that row `cancelled`. The rule and the calendar belong to the whole series, so a
//! change to either is refused here.
//!
//! **All events** changes the master. The form shows one occurrence, so what changed is read
//! against that occurrence, the "base":
//!
//! - A field the user changed goes to the master, and to every exception that still had the
//!   series' value; an exception keeps a field it changed itself. The calendar always follows.
//! - The times move by the same wall-clock amount as the base did, so moving the occurrence of
//!   3 November from 09:00 to 10:00 moves every occurrence to 10:00, also after daylight saving
//!   changes. Exceptions move with the series and stay linked.
//! - A new rule, or a switch between all-day and timed or to another zone, starts the series
//!   again: its exceptions are dropped, as Google does.
//! - No rule at all makes the series the single event in the form.
//!
//! On a remote calendar (#18) every change also queues an outbox row: for one occurrence an
//! update or delete of Google's instance, for all events an update of the master. The server
//! then decides what happens to the exceptions, so when all events move or get a new rule the
//! local exceptions are dropped and the calendar is pulled again in full, after the push.
//
// Rust note: the functions here return `rusqlite::Result<Result<_, ServiceError>>`, as in
// `events.rs`: the outer error is the database's, the inner one a refusal to report.

use chrono::NaiveTime;

use super::ServiceError;
use super::events::{
    EVENT_GONE, Fields, Scope, Stored, Valid, load, now, set_recurrence, write_event,
};
use crate::model::TimeKind;
use crate::outbox::{self, Op};
use crate::provider::google;
use crate::recurrence::{from_wall_clock, wall_clock};

const ONLY_ALL_REPEAT: &str = "A change to how an event repeats applies to all events.";
const ONLY_ALL_CALENDAR: &str = "Only all events can move to another calendar.";

const DAY: i64 = 24 * 3600;

/// The occurrence of `series` at `original_start`: the exception that replaced it, if one did,
/// and the values it shows. `None` if the rule produces no such occurrence, or it was deleted.
pub(super) fn find(
    c: &rusqlite::Connection,
    series: &Stored,
    original_start: i64,
) -> rusqlite::Result<Option<(Option<Stored>, Fields)>> {
    let exception = load(
        c,
        "recurring_event_id = ?1 AND original_start_utc = ?2",
        &[series.id, original_start],
    )?
    .pop();
    Ok(match exception {
        Some(exception) if exception.cancelled => None,
        Some(exception) => {
            let fields = exception.fields.clone();
            Some((Some(exception), fields))
        }
        None if series.produces(original_start) => Some((None, series.fields.at(original_start))),
        None => None,
    })
}

/// Changes one or all occurrences of `series` to `valid`. Returns the original start of the
/// occurrence that was changed, which "all events" may have moved, and the source to sync if
/// an outbox row was queued.
pub(super) fn change(
    c: &rusqlite::Connection,
    series: &Stored,
    original_start: i64,
    scope: Scope,
    valid: &Valid,
) -> rusqlite::Result<Result<(i64, Option<String>), ServiceError>> {
    let Some((exception, base)) = find(c, series, original_start)? else {
        return Ok(Err(ServiceError::NotFound(EVENT_GONE)));
    };
    match scope {
        Scope::This => {
            if valid.rule != series.rule() {
                return Ok(Err(ServiceError::InvalidInput(ONLY_ALL_REPEAT.into())));
            }
            if valid.fields.calendar_id != series.fields.calendar_id {
                return Ok(Err(ServiceError::InvalidInput(ONLY_ALL_CALENDAR.into())));
            }
            let id = match exception {
                Some(exception) => exception.id,
                None => insert_exception(c, series.id, original_start, "confirmed")?,
            };
            // A new exception starts as the series' occurrence, which is what the server has.
            let before = outbox::read_data(c, id)?;
            write_event(c, id, &valid.fields)?;
            let push = outbox::queue(c, Op::Update, id, before.as_ref(), now())?;
            Ok(Ok((original_start, push)))
        }
        Scope::All => {
            let edited = exception.map(|e| e.id);
            let before = outbox::read_data(c, series.id)?;
            let moved = change_all(c, series, original_start, &base, edited, valid)?;
            let push = outbox::queue(c, Op::Update, series.id, before.as_ref(), now())?;
            Ok(Ok((moved, push)))
        }
    }
}

/// Deletes the occurrence of `series` at `original_start`. Returns the source to sync if an
/// outbox row was queued.
pub(super) fn cancel(
    c: &rusqlite::Connection,
    series: &Stored,
    original_start: i64,
) -> rusqlite::Result<Result<Option<String>, ServiceError>> {
    let id = match find(c, series, original_start)? {
        None => return Ok(Err(ServiceError::NotFound(EVENT_GONE))),
        Some((Some(exception), _)) => {
            c.execute(
                "UPDATE events SET status = 'cancelled' WHERE id = ?1",
                [exception.id],
            )?;
            exception.id
        }
        Some((None, _)) => insert_exception(c, series.id, original_start, "cancelled")?,
    };
    // The cancelled row keeps the occurrence's values: what to restore if the server refuses.
    let before = outbox::read_data(c, id)?;
    Ok(Ok(outbox::queue(
        c,
        Op::Delete,
        id,
        before.as_ref(),
        now(),
    )?))
}

/// A new exception to `series` for its occurrence at `original_start`, with the series' values
/// at that time. Returns its id.
///
/// On a remote calendar its remote id is Google's id of that occurrence, so the outbox can
/// address it and a later pull updates this row instead of adding a second one. On a local
/// calendar it gets a random one: a name built from the start would collide once "all events"
/// moves the series.
fn insert_exception(
    c: &rusqlite::Connection,
    series: i64,
    original_start: i64,
    status: &str,
) -> rusqlite::Result<i64> {
    let (calendar_id, master, all_day): (i64, String, bool) = c.query_row(
        "SELECT calendar_id, remote_id, all_day FROM events WHERE id = ?1",
        [series],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    // Google is the only remote calendar that can be written so far.
    let remote_id = match outbox::remote_source(c, calendar_id)? {
        Some(_) => google::instance_id(&master, original_start, all_day),
        None => uuid::Uuid::new_v4().simple().to_string(),
    };
    c.execute(
        "INSERT INTO events (calendar_id, remote_id, ical_uid, status, title, description,
                             location, all_day, start_utc, end_utc, start_tz, end_tz,
                             recurring_remote_id, recurring_event_id, original_start_utc,
                             is_organizer, guests_can_modify, use_default_reminders)
         SELECT calendar_id, ?2, ical_uid, ?4, title, description,
                location, all_day, ?3, ?3 + end_utc - start_utc, start_tz, end_tz,
                remote_id, id, ?3,
                is_organizer, guests_can_modify, use_default_reminders
         FROM events WHERE id = ?1",
        rusqlite::params![series, remote_id, original_start, status],
    )?;
    let id = c.last_insert_rowid();
    c.execute(
        "INSERT INTO event_reminders (event_id, method, minutes_before)
         SELECT ?2, method, minutes_before FROM event_reminders WHERE event_id = ?1",
        [series, id],
    )?;
    Ok(id)
}

/// "All events", chosen on the occurrence at `original_start`, which showed `base` and is the
/// exception `edited` if there is one. Returns that occurrence's new original start.
fn change_all(
    c: &rusqlite::Connection,
    series: &Stored,
    original_start: i64,
    base: &Fields,
    edited: Option<i64>,
    valid: &Valid,
) -> rusqlite::Result<i64> {
    let new = &valid.fields;
    let Some(rule) = &valid.rule else {
        drop_exceptions(c, series.id)?;
        write_event(c, series.id, new)?;
        set_recurrence(c, series.id, None)?;
        return Ok(new.start_utc);
    };

    let old = &series.fields;
    let reshaped = (base.all_day, &base.tz) != (new.all_day, &new.tz);
    let (kind_before, kind) = (old.kind(), if reshaped { new.kind() } else { old.kind() });
    // How far the occurrence moved, on the wall clock of its zone.
    let delta = wall_clock(new.start_utc, new.kind()) - wall_clock(base.start_utc, base.kind());
    let shift = |stored: i64, from: TimeKind, to: TimeKind| {
        let moved = wall_clock(stored, from) + delta;
        if to == TimeKind::AllDay {
            from_wall_clock(moved.date().and_time(NaiveTime::MIN), to)
        } else {
            from_wall_clock(moved, to)
        }
    };
    // The series gets longer or shorter by as much as the occurrence did.
    let length = old.duration() + new.duration() - base.duration();
    let length = if kind == TimeKind::AllDay {
        ((length + DAY / 2).div_euclid(DAY)).max(1) * DAY
    } else {
        length.max(0)
    };

    let start = shift(old.start_utc, kind_before, kind);
    let (all_day, tz) = if reshaped {
        (new.all_day, new.tz.clone())
    } else {
        (old.all_day, old.tz.clone())
    };
    write_event(
        c,
        series.id,
        &Fields {
            all_day,
            tz,
            start_utc: start,
            end_utc: start + length,
            ..follow(old, old, base, new)
        },
    )?;
    let recurrence = rule.to_recurrence(kind, series.recurrence.as_deref());
    set_recurrence(c, series.id, recurrence.as_deref())?;

    let moved = shift(original_start, kind_before, kind);
    if reshaped || Some(rule) != series.rule().as_ref() {
        drop_exceptions(c, series.id)?;
        if outbox::remote_source(c, series.fields.calendar_id)?.is_some() {
            pull_again(c, series.fields.calendar_id)?;
        }
        return Ok(moved);
    }
    // The server keeps the exceptions of a remote series under ids built from their original
    // starts, and decides itself where they go when the series moves. Only it knows.
    let retimed = delta != chrono::TimeDelta::zero() || length != old.duration();
    if retimed && outbox::remote_source(c, series.fields.calendar_id)?.is_some() {
        drop_exceptions(c, series.id)?;
        pull_again(c, series.fields.calendar_id)?;
        return Ok(moved);
    }
    for exception in load(c, "recurring_event_id = ?1", &[series.id])? {
        let fields = if Some(exception.id) == edited {
            new.clone()
        } else {
            let own = &exception.fields;
            let start = shift(own.start_utc, own.kind(), own.kind());
            let length = if own.duration() == old.duration() {
                length
            } else {
                own.duration()
            };
            Fields {
                start_utc: start,
                end_utc: start + length,
                ..follow(own, old, base, new)
            }
        };
        write_event(c, exception.id, &fields)?;
        // Rust note: every exception has an original start; the table's `CHECK` makes sure.
        let original = exception.original_start.unwrap_or(original_start);
        c.execute(
            "UPDATE events SET original_start_utc = ?2 WHERE id = ?1",
            [exception.id, shift(original, kind_before, kind)],
        )?;
    }
    Ok(moved)
}

/// `row` (the master or an exception) after a change from `base` to `new` on all events of a
/// series whose master had `series`: every field that changed, where `row` still had the
/// series' value. Its calendar always follows. The times are `row`'s; the caller sets them.
fn follow(row: &Fields, series: &Fields, base: &Fields, new: &Fields) -> Fields {
    // Rust note: a generic function works for every field type that can be compared and
    // copied, which `PartialEq + Clone` says.
    fn pick<T: PartialEq + Clone>(row: &T, series: &T, base: &T, new: &T) -> T {
        if new != base && row == series {
            new.clone()
        } else {
            row.clone()
        }
    }
    let reminders = pick(
        &(row.use_default_reminders, &row.reminders),
        &(series.use_default_reminders, &series.reminders),
        &(base.use_default_reminders, &base.reminders),
        &(new.use_default_reminders, &new.reminders),
    );
    Fields {
        calendar_id: new.calendar_id,
        title: pick(&row.title, &series.title, &base.title, &new.title),
        description: pick(
            &row.description,
            &series.description,
            &base.description,
            &new.description,
        ),
        location: pick(
            &row.location,
            &series.location,
            &base.location,
            &new.location,
        ),
        use_default_reminders: reminders.0,
        reminders: reminders.1.clone(),
        ..row.clone()
    }
}

/// Makes the next pull of `calendar_id` a full one, which brings back what the server has.
/// The engine pushes before it pulls, so that pull already sees this edit (§7.2).
fn pull_again(c: &rusqlite::Connection, calendar_id: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE calendars SET sync_cursor = NULL WHERE id = ?1",
        [calendar_id],
    )?;
    Ok(())
}

/// Deletes the exceptions of `series`, cancelled ones included.
fn drop_exceptions(c: &rusqlite::Connection, series: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM events WHERE recurring_event_id = ?1", [series])?;
    Ok(())
}
