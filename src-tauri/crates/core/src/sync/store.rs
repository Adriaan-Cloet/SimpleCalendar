//! The database side of a sync cycle (`docs/architecture.md` §7.2).
//!
//! Plain functions on a connection or transaction. The cycle runs them through `Db::read` and
//! `Db::write`, one transaction per page.

use std::collections::{BTreeSet, HashSet};

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{SyncConflict, SyncState};
use crate::model::SourceKind;
use crate::outbox::{self, Op};
use crate::provider::{
    CalendarRef, EventData, MAX_REMINDER_MINUTES, RemoteCalendar, RemoteEvent, SeriesRole,
    SyncCursor,
};

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

/// Records the outcome of a cycle on the source. `Offline` is stored as `error` with the
/// `offline` flag set (migration 6).
pub(super) fn set_status(
    conn: &Connection,
    source_id: &str,
    state: SyncState,
    last_error: Option<&str>,
) -> rusqlite::Result<()> {
    let (status, offline) = match state {
        // A cycle never ends as `Syncing`.
        SyncState::Ok | SyncState::Syncing => ("ok", false),
        SyncState::Error => ("error", false),
        SyncState::Offline => ("error", true),
        SyncState::AuthRequired => ("auth_required", false),
    };
    conn.execute(
        "UPDATE sources SET status = ?2, offline = ?3, last_error = ?4 WHERE id = ?1",
        params![source_id, status, offline, last_error],
    )?;
    Ok(())
}

/// Makes the source's calendars match the provider's list: adds new ones, updates name,
/// colour, zone, access, primary flag and default reminders, and removes those that are gone,
/// with their events. The user's colour and visibility are never touched.
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
        // Rust note: a `Vec<u32>` serialises to a JSON array such as `[10,30]` and cannot fail.
        let defaults =
            serde_json::to_string(&cal.default_reminders).expect("numbers serialise to JSON");
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
                    "UPDATE calendars SET name = ?2, color = ?3, time_zone = ?4, access = ?5,
                                          is_primary = ?6, default_reminders = ?7
                     WHERE id = ?1
                       AND (name IS NOT ?2 OR color IS NOT ?3 OR time_zone IS NOT ?4
                            OR access IS NOT ?5 OR is_primary IS NOT ?6
                            OR default_reminders IS NOT ?7)",
                    params![
                        id,
                        cal.name,
                        cal.color,
                        cal.time_zone,
                        cal.access.as_str(),
                        cal.is_primary,
                        defaults
                    ],
                )?;
                if updated > 0 {
                    changed.push(id);
                }
                (id, cursor)
            }
            None => {
                tx.execute(
                    "INSERT INTO calendars (source_id, remote_id, name, color, time_zone, access,
                                            is_primary, default_reminders)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        source_id,
                        cal.remote_id,
                        cal.name,
                        cal.color,
                        cal.time_zone,
                        cal.access.as_str(),
                        cal.is_primary,
                        defaults
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

/// What applying a page did.
#[derive(Debug, Default)]
pub(crate) struct Applied {
    /// Whether any event changed.
    pub changed: bool,
    /// Our changes that met a newer version on the server, for `sync:conflict` (§7.4).
    pub conflicts: Vec<SyncConflict>,
}

/// Applies one page of changes and adds the remote ids it upserted to `seen`.
///
/// An event with outbox rows that were not sent yet is a possible conflict (§7.4). If the server
/// still has the version our edit was based on, the event is left alone and the edit goes out
/// with the next push. If the server already has our change, the rows are done. Otherwise the
/// server wins: its version replaces ours and the rows become `conflicted`.
pub(crate) fn apply_page(
    tx: &Transaction,
    calendar_id: i64,
    upserts: &[RemoteEvent],
    deletions: &[String],
    seen: &mut HashSet<String>,
) -> rusqlite::Result<Applied> {
    let mut applied = Applied::default();
    for event in upserts {
        seen.insert(event.remote_id.clone());
        match waiting(tx, calendar_id, &event.remote_id)? {
            None => {
                upsert_event(tx, calendar_id, event)?;
                applied.changed = true;
            }
            Some((id, rows)) => reconcile(tx, calendar_id, id, &rows, Some(event), &mut applied)?,
        }
    }
    for remote_id in deletions {
        match waiting(tx, calendar_id, remote_id)? {
            // Deleting a master also deletes its linked exceptions, through the cascade on
            // `recurring_event_id`.
            None => {
                applied.changed |= tx.execute(
                    "DELETE FROM events WHERE calendar_id = ?1 AND remote_id = ?2",
                    params![calendar_id, remote_id],
                )? > 0;
            }
            Some((id, rows)) => reconcile(tx, calendar_id, id, &rows, None, &mut applied)?,
        }
    }
    Ok(applied)
}

/// The id of event `remote_id` and its outbox rows that were not sent yet, if it has any.
fn waiting(
    tx: &Transaction,
    calendar_id: i64,
    remote_id: &str,
) -> rusqlite::Result<Option<(i64, Vec<outbox::Pending>)>> {
    let id: Option<i64> = tx
        .query_row(
            "SELECT id FROM events WHERE calendar_id = ?1 AND remote_id = ?2",
            params![calendar_id, remote_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    let rows = outbox::waiting(tx, id)?;
    Ok((!rows.is_empty()).then_some((id, rows)))
}

/// Decides between the server's version of event `id` (`remote`, or `None` if it was deleted
/// there) and our `rows` for it, which are not empty (§7.4).
fn reconcile(
    tx: &Transaction,
    calendar_id: i64,
    id: i64,
    rows: &[outbox::Pending],
    remote: Option<&RemoteEvent>,
    applied: &mut Applied,
) -> rusqlite::Result<()> {
    // Rust note: `rows` is never empty, so `first` and `last` are `Some`; the `let else` only
    // satisfies the compiler.
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Ok(());
    };
    let done = match remote {
        Some(remote) => {
            // The ETag tells whether the server's version is still the one the edit was based
            // on. Without one on either side, the values the app owns do.
            let ours: Option<String> =
                tx.query_row("SELECT etag FROM events WHERE id = ?1", [id], |r| r.get(0))?;
            let unchanged = match (ours, &remote.etag) {
                (Some(ours), Some(theirs)) => ours == *theirs,
                _ => first
                    .base
                    .as_ref()
                    .is_some_and(|base| *base == EventData::from(remote)),
            };
            if unchanged {
                log::info!("event {}: kept, an edit waits to be sent", remote.remote_id);
                return Ok(());
            }
            last.op != Op::Delete
                && last
                    .payload
                    .as_ref()
                    .is_some_and(|p| outbox::has_change(p, first.base.as_ref(), remote))
        }
        None => last.op == Op::Delete,
    };
    applied.changed = true;
    if !done {
        let conflict = server_wins(tx, calendar_id, Some(id), rows, remote)?;
        applied.conflicts.push(conflict);
        return Ok(());
    }
    // An earlier attempt got through and only its answer was lost.
    for row in rows {
        outbox::remove(tx, row.id)?;
    }
    match remote {
        Some(remote) => upsert_event(tx, calendar_id, remote),
        None => tx
            .execute("DELETE FROM events WHERE id = ?1", [id])
            .map(drop),
    }
}

/// The server wins (§7.4): `rows` become `conflicted`, keeping their payload for the sync issues
/// list, and event `id` becomes what the server has, `remote`, or goes if the server has nothing.
/// Returns what `sync:conflict` reports.
pub(super) fn server_wins(
    tx: &Transaction,
    calendar_id: i64,
    id: Option<i64>,
    rows: &[outbox::Pending],
    remote: Option<&RemoteEvent>,
) -> rusqlite::Result<SyncConflict> {
    let remote_id = rows.first().map_or("", |r| r.event_remote_id.as_str());
    log::warn!("event {remote_id}: changed elsewhere; the server's version stays");
    for row in rows {
        outbox::set_outcome(
            tx,
            row.id,
            "conflicted",
            "The event was changed elsewhere, so this change was not saved.",
        )?;
    }
    match (remote, id) {
        (Some(event), Some(id)) => {
            tx.execute("UPDATE events SET pending_delete = 0 WHERE id = ?1", [id])?;
            upsert_event(tx, calendar_id, event)?;
        }
        (None, Some(id)) => {
            tx.execute("DELETE FROM events WHERE id = ?1", [id])?;
        }
        (_, None) => {}
    }
    // The title of the user's change, or of the event before it for a delete.
    let title = rows
        .last()
        .and_then(|r| r.payload.as_ref())
        .or_else(|| rows.first().and_then(|r| r.base.as_ref()))
        .map(|data| data.title.clone())
        .or_else(|| remote.map(|r| r.title.clone()))
        .unwrap_or_default();
    Ok(SyncConflict {
        outbox_id: rows.last().map_or(0, |r| r.id),
        title,
    })
}

/// Inserts or updates one event, then links it to its series (§7.2). The engine also stores
/// what the server answers to a write with it.
pub(crate) fn upsert_event(
    tx: &Transaction,
    calendar_id: i64,
    event: &RemoteEvent,
) -> rusqlite::Result<()> {
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
                             remote_updated_at, etag, is_organizer, guests_can_modify,
                             use_default_reminders)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20, ?21)
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
             remote_updated_at = excluded.remote_updated_at,
             etag = excluded.etag,
             is_organizer = excluded.is_organizer,
             guests_can_modify = excluded.guests_can_modify,
             use_default_reminders = excluded.use_default_reminders
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
            event.etag,
            event.is_organizer,
            event.guests_can_modify,
            event.reminders.use_default,
        ],
        |r| r.get(0),
    )?;

    // The event's own reminders, replaced as a whole like Google's `overrides` array. A value
    // outside the column's range would fail the whole page, so it is dropped instead.
    tx.execute("DELETE FROM event_reminders WHERE event_id = ?1", [id])?;
    for reminder in &event.reminders.overrides {
        if reminder.minutes_before > MAX_REMINDER_MINUTES {
            continue;
        }
        tx.execute(
            "INSERT OR IGNORE INTO event_reminders (event_id, method, minutes_before)
             VALUES (?1, ?2, ?3)",
            params![id, reminder.method.as_str(), reminder.minutes_before],
        )?;
    }

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

/// After the last page of a full pull: deletes the events that were not seen, except those
/// with an outbox row still to send (a create the server has not heard of yet, or an
/// occurrence changed here that the server only lists once it is sent), then the
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
        if outbox::has_pending(tx, id)? {
            continue;
        }
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

/// Stores the warnings of a finished pull (§7.7). A full pull's warnings replace the earlier
/// ones; an incremental pull's are added to them. Called in the transaction of the last page.
pub(super) fn store_warnings(
    tx: &Transaction,
    calendar_id: i64,
    warnings: BTreeSet<String>,
    full: bool,
) -> rusqlite::Result<()> {
    let mut all = warnings;
    if !full {
        if all.is_empty() {
            return Ok(());
        }
        let stored: String = tx.query_row(
            "SELECT warnings FROM calendars WHERE id = ?1",
            [calendar_id],
            |r| r.get(0),
        )?;
        // A value this code did not write is dropped rather than failing the sync.
        all.extend(serde_json::from_str::<Vec<String>>(&stored).unwrap_or_default());
    }
    let json = serde_json::to_string(&all).expect("strings serialise");
    tx.execute(
        "UPDATE calendars SET warnings = ?2 WHERE id = ?1",
        params![calendar_id, json],
    )?;
    Ok(())
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
