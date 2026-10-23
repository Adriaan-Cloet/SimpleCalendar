//! The scheduler's queries. `display_zone`, `candidates` and `fired` run on the read connection,
//! `set_display_zone`, `fire` and `prune` on the write connection.

use std::collections::{HashMap, HashSet};

use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, params};

use super::plan::{Candidate, GRACE, Key};
use crate::db::occurrences;

/// The `settings` key of the zone the frontend last passed to `list_occurrences` (§10).
const DISPLAY_ZONE: &str = "display_zone";

/// How long `fired_reminders` rows are kept (§9).
const KEEP_FIRED: i64 = 7 * 24 * 3600;

/// The display zone, or `None` before the frontend first reported one.
pub(crate) fn display_zone(c: &Connection) -> rusqlite::Result<Option<Tz>> {
    let name: Option<String> = c
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [DISPLAY_ZONE],
            |r| r.get(0),
        )
        .optional()?;
    Ok(name.and_then(|n| n.parse().ok()))
}

/// Remembers `zone` as the display zone. Whether it differs from the one remembered before.
pub(crate) fn set_display_zone(c: &Connection, zone: Tz) -> rusqlite::Result<bool> {
    let changed = c.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value WHERE value <> excluded.value",
        params![DISPLAY_ZONE, zone.name()],
    )?;
    Ok(changed > 0)
}

/// Every `popup` reminder that comes due before `until` and whose occurrence has not started more
/// than `GRACE` before `now`, on a visible calendar. Times of all-day and floating events are
/// taken in `zone`.
pub(crate) fn candidates(
    c: &Connection,
    now: i64,
    until: i64,
    zone: Tz,
) -> rusqlite::Result<Vec<Candidate>> {
    // Looking only as far ahead as the longest reminder keeps the expansion short, and skips it
    // when no event has a reminder at all.
    let Some(lead) = longest_lead(c)? else {
        return Ok(Vec::new());
    };
    let occurrences = occurrences::list_timed(c, now - GRACE, until + lead * 60, zone)?;

    let mut by_row: HashMap<i64, Vec<u32>> = HashMap::new();
    let mut found = Vec::new();
    for o in occurrences.into_iter().filter(|o| o.start >= now - GRACE) {
        let minutes = match by_row.get(&o.row_id) {
            Some(minutes) => minutes.clone(),
            None => {
                let minutes = popup_minutes(c, o.row_id)?;
                by_row.insert(o.row_id, minutes.clone());
                minutes
            }
        };
        for m in minutes {
            let fire_at = o.start - i64::from(m) * 60;
            if fire_at < until {
                found.push(Candidate {
                    key: Key {
                        event_id: o.occurrence.event_id,
                        start: o.start,
                        minutes_before: m,
                    },
                    fire_at,
                    occurrence: o.occurrence.clone(),
                });
            }
        }
    }
    Ok(found)
}

/// The reminders that were shown already.
pub(crate) fn fired(c: &Connection) -> rusqlite::Result<HashSet<Key>> {
    c.prepare("SELECT event_id, occurrence_start, minutes_before FROM fired_reminders")?
        .query_map([], |r| {
            Ok(Key {
                event_id: r.get(0)?,
                start: r.get(1)?,
                minutes_before: r.get(2)?,
            })
        })?
        .collect()
}

/// Records that `key` is shown at `now`. `false` if it was recorded before, or its event is gone:
/// then it must not be shown.
pub(crate) fn fire(c: &Connection, key: Key, now: i64) -> rusqlite::Result<bool> {
    // `OR IGNORE` covers a row that exists; the `WHERE` an event deleted since the read, which
    // would otherwise break the foreign key.
    let inserted = c.execute(
        "INSERT OR IGNORE INTO fired_reminders (event_id, occurrence_start, minutes_before, fired_at)
         SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM events WHERE id = ?1)",
        params![key.event_id, key.start, key.minutes_before, now],
    )?;
    Ok(inserted > 0)
}

/// Deletes the rows of reminders shown more than a week before `now`. The number deleted.
pub(crate) fn prune(c: &Connection, now: i64) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM fired_reminders WHERE fired_at < ?1",
        [now - KEEP_FIRED],
    )
}

/// The minutes of the longest `popup` reminder of any event or calendar default, if there is one.
fn longest_lead(c: &Connection) -> rusqlite::Result<Option<i64>> {
    let own: Option<i64> = c.query_row(
        "SELECT max(minutes_before) FROM event_reminders WHERE method = 'popup'",
        [],
        |r| r.get(0),
    )?;
    let defaults = c
        .prepare("SELECT default_reminders FROM calendars WHERE default_reminders <> '[]'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .flat_map(|json| parse_defaults(json))
        .map(i64::from)
        .max();
    Ok(own.max(defaults))
}

/// The minutes of the `popup` reminders that apply to event row `id`: the calendar's defaults if
/// the event uses them, else its own.
fn popup_minutes(c: &Connection, id: i64) -> rusqlite::Result<Vec<u32>> {
    let (use_defaults, defaults): (bool, String) = c.query_row(
        "SELECT e.use_default_reminders, c.default_reminders
         FROM events e JOIN calendars c ON c.id = e.calendar_id WHERE e.id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut minutes = if use_defaults {
        parse_defaults(&defaults)
    } else {
        c.prepare(
            "SELECT minutes_before FROM event_reminders WHERE event_id = ?1 AND method = 'popup'",
        )?
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?
    };
    minutes.sort_unstable();
    minutes.dedup();
    Ok(minutes)
}

/// `calendars.default_reminders`, a JSON array of minutes. A value this code did not write is
/// read as no defaults, as `get_event` does.
fn parse_defaults(json: &str) -> Vec<u32> {
    serde_json::from_str(json).unwrap_or_default()
}
