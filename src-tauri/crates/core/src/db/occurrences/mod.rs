//! Reading a date range: the occurrences the views show (`docs/architecture.md` §6 "Reading a
//! date range").
//!
//! Two queries on the read connection, then expansion in Rust. Nothing here writes: occurrences
//! are computed per query and never stored (§4).

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use crate::model::{Occurrence, TimeKind};
use crate::recurrence::{self, Series};

/// Floating and all-day values are stored as if in UTC, so in the viewer's zone they can fall
/// up to 14 hours (the largest UTC offset) on either side. Both queries widen the range by this
/// much; the exact filter happens in Rust.
const WIDEN: i64 = 14 * 3600;

/// The columns both queries select, in this order. `color` is already resolved: the event's own,
/// else the user's calendar colour, else the provider's.
const COLUMNS: &str = "e.id, e.calendar_id, e.title, e.location,
    coalesce(e.color, c.color_override, c.color),
    c.access = 'write' AND (e.is_organizer OR e.guests_can_modify),
    e.all_day, e.start_utc, e.end_utc, e.start_tz, e.recurrence,
    e.recurring_event_id, e.original_start_utc";

/// Single events and linked exceptions that overlap the range. An exception whose master has
/// not arrived yet has no `recurring_event_id` and stays invisible (§7.2).
const SINGLES: &str = "FROM events e JOIN calendars c ON c.id = e.calendar_id
    WHERE c.visible = 1
      AND e.recurrence IS NULL
      AND e.status <> 'cancelled'
      AND e.pending_delete = 0
      AND (e.recurring_remote_id IS NULL OR e.recurring_event_id IS NOT NULL)
      AND e.start_utc < ?2
      AND e.end_utc > ?1";

/// Series that can still produce an occurrence in the range. A cancelled master means the
/// whole series was deleted, and `pending_delete` that it is being deleted (§7.3).
const SERIES: &str = "FROM events e JOIN calendars c ON c.id = e.calendar_id
    WHERE c.visible = 1
      AND e.recurrence IS NOT NULL
      AND e.status <> 'cancelled'
      AND e.pending_delete = 0
      AND e.start_utc < ?2
      AND (e.recurrence_end_utc IS NULL OR e.recurrence_end_utc > ?1)";

/// Every occurrence that overlaps `[from, to)` on a visible calendar, sorted by start.
///
/// `from` and `to` are UTC instants (epoch seconds). Times come back in the `viewer` zone.
/// Run it inside `Db::read`. A series whose recurrence cannot be expanded is logged and shows
/// only its first occurrence, so one broken event never hides the rest.
pub fn list_occurrences(
    conn: &Connection,
    from: i64,
    to: i64,
    viewer: Tz,
) -> rusqlite::Result<Vec<Occurrence>> {
    let (wide_from, wide_to) = (from - WIDEN, to + WIDEN);
    let mut found = Vec::new();

    let mut singles = conn.prepare(&format!("SELECT {COLUMNS} {SINGLES}"))?;
    for row in singles.query_map(params![wide_from, wide_to], EventRow::read)? {
        let row = row?;
        found.push(match row.recurring_event_id {
            Some(_) => Found::exception(row),
            None => Found::single(row),
        });
    }

    let mut series = conn.prepare(&format!("SELECT {COLUMNS} {SERIES}"))?;
    let mut replaced =
        conn.prepare("SELECT original_start_utc FROM events WHERE recurring_event_id = ?1")?;
    let masters = series
        .query_map(params![wide_from, wide_to], EventRow::read)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in masters {
        // Every exception replaces one occurrence: a moved one shows through the first query at
        // its new time, a cancelled one not at all.
        let replaced: HashSet<i64> = replaced
            .query_map([row.id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let duration = row.end - row.start;
        let starts = expand_or_first(&row, wide_from, wide_to);
        for start in starts.into_iter().filter(|s| !replaced.contains(s)) {
            found.push(Found {
                event_id: row.id,
                original_start: start,
                start,
                end: start + duration,
                row: row.clone(),
            });
        }
    }

    // The exact filter, in the viewer's zone, then the order the views draw in.
    //
    // Rust note: `filter_map` keeps the `Some` results and drops the `None`s, so filtering and
    // computing the instants happen in one pass. `then` turns a `bool` into `Some(..)` or `None`.
    let mut shown: Vec<_> = found
        .into_iter()
        .filter_map(|f| {
            let start = recurrence::to_instant(f.start, f.row.kind, viewer);
            let end = recurrence::to_instant(f.end, f.row.kind, viewer);
            recurrence::overlaps(start, end - start, from, to).then_some((start, end, f))
        })
        .collect();
    shown.sort_by_key(|(start, end, f)| (*start, *end, f.event_id));

    Ok(shown
        .into_iter()
        .map(|(_, _, f)| f.into_occurrence(viewer))
        .collect())
}

/// One occurrence in the `viewer` zone, also when its calendar is hidden: the single event
/// `event_id` (whatever `original_start` says), or the occurrence of series `event_id` that the
/// rule produced at `original_start`, as an exception replaced it if one did. `None` if there is
/// no such event or occurrence, or it was deleted.
pub fn occurrence(
    conn: &Connection,
    event_id: i64,
    original_start: i64,
    viewer: Tz,
) -> rusqlite::Result<Option<Occurrence>> {
    let select = |condition: &str, values: &[i64]| {
        conn.query_row(
            &format!(
                "SELECT {COLUMNS} FROM events e JOIN calendars c ON c.id = e.calendar_id
                 WHERE e.status <> 'cancelled' AND e.pending_delete = 0 AND {condition}"
            ),
            params_from_iter(values),
            EventRow::read,
        )
        .optional()
    };
    let Some(row) = select("e.id = ?1 AND e.recurring_remote_id IS NULL", &[event_id])? else {
        return Ok(None);
    };
    if row.recurrence.is_none() {
        return Ok(Some(Found::single(row).into_occurrence(viewer)));
    }

    let replaced: Option<bool> = conn
        .query_row(
            "SELECT status = 'cancelled' FROM events
             WHERE recurring_event_id = ?1 AND original_start_utc = ?2",
            [event_id, original_start],
            |r| r.get(0),
        )
        .optional()?;
    let found = match replaced {
        Some(true) => None,
        Some(false) => select(
            "e.recurring_event_id = ?1 AND e.original_start_utc = ?2",
            &[event_id, original_start],
        )?
        .map(Found::exception),
        None => {
            let series = Series {
                start: row.start,
                duration: row.end - row.start,
                kind: row.kind,
                recurrence: row.recurrence.as_deref().unwrap_or_default(),
            };
            recurrence::produces(&series, original_start).then(|| Found {
                event_id,
                original_start,
                start: original_start,
                end: original_start + series.duration,
                row: row.clone(),
            })
        }
    };
    Ok(found.map(|f| f.into_occurrence(viewer)))
}

/// The stored starts of a series in the range, or only its first one if the recurrence cannot
/// be expanded.
fn expand_or_first(row: &EventRow, from: i64, to: i64) -> Vec<i64> {
    let series = Series {
        start: row.start,
        duration: row.end - row.start,
        kind: row.kind,
        recurrence: row.recurrence.as_deref().unwrap_or_default(),
    };
    recurrence::expand(&series, from, to).unwrap_or_else(|e| {
        log::warn!("event {}: cannot expand recurrence: {e}", row.id);
        vec![row.start]
    })
}

/// One row of either query.
#[derive(Clone)]
struct EventRow {
    id: i64,
    calendar_id: i64,
    title: String,
    location: Option<String>,
    color: Option<String>,
    /// Writable calendar, and the organizer rule of §4.
    writable: bool,
    kind: TimeKind,
    start: i64,
    end: i64,
    recurrence: Option<String>,
    recurring_event_id: Option<i64>,
    original_start: Option<i64>,
}

impl EventRow {
    /// Reads the columns in the order of `COLUMNS`.
    fn read(r: &Row<'_>) -> rusqlite::Result<EventRow> {
        let start_tz: Option<String> = r.get(9)?;
        Ok(EventRow {
            id: r.get(0)?,
            calendar_id: r.get(1)?,
            title: r.get(2)?,
            location: r.get(3)?,
            color: r.get(4)?,
            writable: r.get(5)?,
            kind: TimeKind::from_columns(start_tz.as_deref(), r.get(6)?),
            start: r.get(7)?,
            end: r.get(8)?,
            recurrence: r.get(10)?,
            recurring_event_id: r.get(11)?,
            original_start: r.get(12)?,
        })
    }
}

/// An occurrence before the exact filter, still in stored values.
struct Found {
    event_id: i64,
    original_start: i64,
    start: i64,
    end: i64,
    row: EventRow,
}

impl Found {
    fn single(row: EventRow) -> Found {
        Found {
            event_id: row.id,
            original_start: row.start,
            start: row.start,
            end: row.end,
            row,
        }
    }

    /// A linked exception: the occurrence of its series that it replaces.
    fn exception(row: EventRow) -> Found {
        Found {
            event_id: row.recurring_event_id.unwrap_or(row.id),
            original_start: row.original_start.unwrap_or(row.start),
            start: row.start,
            end: row.end,
            row,
        }
    }

    fn into_occurrence(self, viewer: Tz) -> Occurrence {
        let kind = self.row.kind;
        Occurrence {
            event_id: self.event_id,
            original_start: self.original_start,
            calendar_id: self.row.calendar_id,
            title: self.row.title,
            location: self.row.location,
            color: self.row.color,
            start: recurrence::to_viewer(self.start, kind, viewer),
            end: recurrence::to_viewer(self.end, kind, viewer),
            editable: self.row.writable,
        }
    }
}
