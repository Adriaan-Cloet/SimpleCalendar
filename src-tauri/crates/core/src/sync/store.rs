//! The database side of a sync cycle (`docs/architecture.md` §7.2).
//!
//! Plain functions on a connection or transaction. The cycle runs them through `Db::read` and
//! `Db::write`, one transaction per page.

use std::collections::HashSet;

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::model::SourceKind;
use crate::provider::{CalendarRef, RemoteCalendar, RemoteEvent, SeriesRole, SyncCursor};

/// A source, as far as a cycle needs it.
pub(super) struct SourceRow {
    pub kind: SourceKind,
    pub display_name: String,
    pub auth_required: bool,
    /// `sources.sync_interval_s`; `None` means the provider default.
    pub interval_s: Option<i64>,
}

/// A calendar of the source, after the calendar list was applied.
pub(super) struct LocalCalendar {
    pub id: i64,
    pub reference: CalendarRef,
    pub cursor: Option<SyncCursor>,
}

/// The ids of every source that is not local, oldest first.
pub(super) fn remote_source_ids(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT id FROM sources WHERE kind <> 'local' ORDER BY created_at, id")?;
    stmt.query_map([], |r| r.get(0))?.collect()
}

/// The source `id`, or `None` if it was removed.
pub(super) fn load_source(conn: &Connection, id: &str) -> rusqlite::Result<Option<SourceRow>> {
    conn.query_row(
        "SELECT kind, status, sync_interval_s, display_name FROM sources WHERE id = ?1",
        [id],
        |r| {
            let kind: String = r.get(0)?;
            let status: String = r.get(1)?;
            Ok(SourceRow {
                kind: SourceKind::parse(&kind).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        Type::Text,
                        format!("unknown source kind {kind:?}").into(),
                    )
                })?,
                auth_required: status == "auth_required",
                interval_s: r.get(2)?,
                display_name: r.get(3)?,
            })
        },
    )
    .optional()
}

/// Records the outcome of a cycle on the source.
pub(super) fn set_status(
    conn: &Connection,
    source_id: &str,
    status: &str,
    last_error: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sources SET status = ?2, last_error = ?3 WHERE id = ?1",
        params![source_id, status, last_error],
    )?;
    Ok(())
}

/// Makes the source's calendars match the provider's list: adds new ones, updates name,
/// colour, zone and access, and removes those that are gone, with their events.
///
/// Returns the calendars in the provider's order, and the ids of those that were added,
/// changed or removed.
pub(super) fn apply_calendars(
    tx: &Transaction,
    source_id: &str,
    remote: &[RemoteCalendar],
) -> rusqlite::Result<(Vec<LocalCalendar>, Vec<i64>)> {
    let mut calendars = Vec::with_capacity(remote.len());
    let mut changed = Vec::new();

    for cal in remote {
        let existing: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT id, sync_cursor FROM calendars WHERE source_id = ?1 AND remote_id = ?2",
                params![source_id, cal.remote_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (id, cursor) = match existing {
            Some((id, cursor)) => {
                // `IS NOT` instead of `<>`: it compares NULLs as values, so a colour that
                // goes from NULL to '#336699' counts as a change.
                let updated = tx.execute(
                    "UPDATE calendars SET name = ?2, color = ?3, time_zone = ?4, access = ?5
                     WHERE id = ?1
                       AND (name IS NOT ?2 OR color IS NOT ?3 OR time_zone IS NOT ?4
                            OR access IS NOT ?5)",
                    params![id, cal.name, cal.color, cal.time_zone, cal.access.as_str()],
                )?;
                if updated > 0 {
                    changed.push(id);
                }
                (id, cursor)
            }
            None => {
                tx.execute(
                    "INSERT INTO calendars (source_id, remote_id, name, color, time_zone, access)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        source_id,
                        cal.remote_id,
                        cal.name,
                        cal.color,
                        cal.time_zone,
                        cal.access.as_str()
                    ],
                )?;
                let id = tx.last_insert_rowid();
                changed.push(id);
                (id, None)
            }
        };
        calendars.push(LocalCalendar {
            id,
            reference: CalendarRef {
                remote_id: cal.remote_id.clone(),
                time_zone: cal.time_zone.clone(),
            },
            cursor: cursor.map(SyncCursor),
        });
    }

    let kept: HashSet<i64> = calendars.iter().map(|c| c.id).collect();
    let mut stmt = tx.prepare("SELECT id FROM calendars WHERE source_id = ?1")?;
    let gone: Vec<i64> = stmt
        .query_map([source_id], |r| r.get(0))?
        .filter(|id| !matches!(id, Ok(id) if kept.contains(id)))
        .collect::<rusqlite::Result<_>>()?;
    for id in gone {
        // The cascade removes the calendar's events.
        tx.execute("DELETE FROM calendars WHERE id = ?1", [id])?;
        changed.push(id);
    }

    Ok((calendars, changed))
}

/// Applies one page of changes and adds the remote ids it upserted to `seen`. Returns whether
/// anything changed.
pub(super) fn apply_page(
    tx: &Transaction,
    calendar_id: i64,
    upserts: &[RemoteEvent],
    deletions: &[String],
    seen: &mut HashSet<String>,
) -> rusqlite::Result<bool> {
    for event in upserts {
        upsert_event(tx, calendar_id, event)?;
        seen.insert(event.remote_id.clone());
    }
    let mut deleted = 0;
    for remote_id in deletions {
        // Deleting a master also deletes its linked exceptions, through the cascade on
        // `recurring_event_id`.
        deleted += tx.execute(
            "DELETE FROM events WHERE calendar_id = ?1 AND remote_id = ?2",
            params![calendar_id, remote_id],
        )?;
    }
    Ok(!upserts.is_empty() || deleted > 0)
}

/// Inserts or updates one event, then links it to its series (§7.2).
fn upsert_event(tx: &Transaction, calendar_id: i64, event: &RemoteEvent) -> rusqlite::Result<()> {
    let (recurrence, master_remote_id, original_start) = match &event.series {
        SeriesRole::Single => (None, None, None),
        SeriesRole::Master { recurrence } => (Some(recurrence.as_str()), None, None),
        SeriesRole::Exception {
            master_remote_id,
            original_start_utc,
        } => (
            None,
            Some(master_remote_id.as_str()),
            Some(*original_start_utc),
        ),
    };

    // `ON CONFLICT DO UPDATE`, never `INSERT OR REPLACE`: the latter deletes the row and
    // inserts a new one, which gives it a new id and lets the cascade delete the exceptions of
    // a master every time it is synced (§6). The link is cleared here and set again below, so
    // an event that stops being an exception does not keep a stale one.
    let id: i64 = tx.query_row(
        "INSERT INTO events (calendar_id, remote_id, ical_uid, status, title, description,
                             location, color, all_day, start_utc, end_utc, start_tz, end_tz,
                             recurrence, recurring_remote_id, original_start_utc,
                             remote_updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT (calendar_id, remote_id) DO UPDATE SET
             ical_uid = excluded.ical_uid,
             status = excluded.status,
             title = excluded.title,
             description = excluded.description,
             location = excluded.location,
             color = excluded.color,
             all_day = excluded.all_day,
             start_utc = excluded.start_utc,
             end_utc = excluded.end_utc,
             start_tz = excluded.start_tz,
             end_tz = excluded.end_tz,
             recurrence = excluded.recurrence,
             recurring_remote_id = excluded.recurring_remote_id,
             recurring_event_id = NULL,
             original_start_utc = excluded.original_start_utc,
             remote_updated_at = excluded.remote_updated_at
         RETURNING id",
        params![
            calendar_id,
            event.remote_id,
            event.ical_uid,
            event.status.as_str(),
            event.title,
            event.description,
            event.location,
            event.color,
            event.all_day,
            event.start_utc,
            event.end_utc,
            event.start_tz,
            event.end_tz,
            recurrence,
            master_remote_id,
            original_start,
            event.remote_updated_at,
        ],
        |r| r.get(0),
    )?;

    match &event.series {
        SeriesRole::Single => {}
        // Link the exception if its master is already here.
        SeriesRole::Exception { .. } => {
            tx.execute(
                "UPDATE events
                 SET recurring_event_id = (
                     SELECT m.id FROM events m
                     WHERE m.calendar_id = events.calendar_id
                       AND m.remote_id = events.recurring_remote_id)
                 WHERE id = ?1",
                [id],
            )?;
        }
        // Adopt the exceptions that arrived before the master.
        SeriesRole::Master { .. } => {
            tx.execute(
                "UPDATE events SET recurring_event_id = ?1
                 WHERE calendar_id = ?2
                   AND recurring_remote_id = ?3
                   AND recurring_event_id IS NULL",
                params![id, calendar_id, event.remote_id],
            )?;
        }
    }
    Ok(())
}

/// After the last page of a full pull: deletes the events that were not seen, then the
/// exceptions whose series never arrived. Returns whether anything was deleted.
pub(super) fn finish_full_pull(
    tx: &Transaction,
    calendar_id: i64,
    seen: &HashSet<String>,
) -> rusqlite::Result<bool> {
    let mut stmt = tx.prepare("SELECT id, remote_id FROM events WHERE calendar_id = ?1")?;
    let unseen: Vec<i64> = stmt
        .query_map([calendar_id], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?
        .filter(|row| !matches!(row, Ok((_, remote_id)) if seen.contains(remote_id)))
        .map(|row| row.map(|(id, _)| id))
        .collect::<rusqlite::Result<_>>()?;
    let mut deleted = 0;
    for id in unseen {
        // A master deleted earlier in this loop may already have taken this row with it.
        deleted += tx.execute("DELETE FROM events WHERE id = ?1", [id])?;
    }
    deleted += tx.execute(
        "DELETE FROM events
         WHERE calendar_id = ?1
           AND recurring_remote_id IS NOT NULL
           AND recurring_event_id IS NULL",
        [calendar_id],
    )?;
    Ok(deleted > 0)
}

/// How many exceptions of the calendar wait for a master that is not here.
pub(super) fn unlinked_exceptions(tx: &Transaction, calendar_id: i64) -> rusqlite::Result<i64> {
    tx.query_row(
        "SELECT count(*) FROM events
         WHERE calendar_id = ?1
           AND recurring_remote_id IS NOT NULL
           AND recurring_event_id IS NULL",
        [calendar_id],
        |r| r.get(0),
    )
}

/// Stores the cursor of a finished pull. Called in the transaction of the last page.
pub(super) fn store_cursor(
    tx: &Transaction,
    calendar_id: i64,
    cursor: &SyncCursor,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE calendars SET sync_cursor = ?2, last_synced_at = ?3 WHERE id = ?1",
        params![calendar_id, cursor.0, now],
    )?;
    Ok(())
}
