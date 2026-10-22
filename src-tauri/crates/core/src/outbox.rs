//! The outbox: local edits to remote calendars, waiting to be sent (`docs/architecture.md`
//! §7.3).
//!
//! A write to a remote calendar changes `events` and queues a row here in the same transaction,
//! so the UI shows the edit at once and the sync engine sends it before its next pull. A row
//! holds the event as the user wants it (`payload`) and as it was before (`base`); the engine
//! sends the difference. Writes to local calendars queue nothing.
//!
//! v0.2 sends rows in order and stops at the first that cannot be sent yet; retries with
//! backoff, coalescing and crash recovery are the offline outbox of v0.3.

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::SourceKind;
use crate::provider::{EventData, Reminder, ReminderMethod, Reminders};

/// What a row asks the server to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Create,
    Update,
    Delete,
}

impl Op {
    /// The value in `outbox.op`.
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Create => "create",
            Op::Update => "update",
            Op::Delete => "delete",
        }
    }

    pub fn parse(value: &str) -> Option<Op> {
        [Op::Create, Op::Update, Op::Delete]
            .into_iter()
            .find(|op| op.as_str() == value)
    }
}

/// The values of event `id` that the app owns, or `None` if there is no such event.
pub fn read_data(c: &Connection, id: i64) -> rusqlite::Result<Option<EventData>> {
    let Some(mut data) = c
        .query_row(
            "SELECT title, description, location, all_day, start_utc, end_utc, start_tz, end_tz,
                    recurrence, use_default_reminders
             FROM events WHERE id = ?1",
            [id],
            |r| {
                Ok(EventData {
                    title: r.get(0)?,
                    description: r.get(1)?,
                    location: r.get(2)?,
                    all_day: r.get(3)?,
                    start_utc: r.get(4)?,
                    end_utc: r.get(5)?,
                    start_tz: r.get(6)?,
                    end_tz: r.get(7)?,
                    recurrence: r.get(8)?,
                    reminders: Reminders {
                        use_default: r.get(9)?,
                        overrides: Vec::new(),
                    },
                })
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut stmt =
        c.prepare("SELECT method, minutes_before FROM event_reminders WHERE event_id = ?1")?;
    data.reminders.overrides = stmt
        .query_map([id], |r| {
            let method: String = r.get(0)?;
            Ok(Reminder {
                // The column's `CHECK` allows only these two.
                method: ReminderMethod::parse(&method).unwrap_or(ReminderMethod::Popup),
                minutes_before: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    data.reminders.overrides.sort();
    Ok(Some(data))
}

/// Writes `data` back to event `id`: what a failed edit is rolled back to (§7.3).
pub fn write_data(c: &Connection, id: i64, data: &EventData) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE events
         SET title = ?2, description = ?3, location = ?4, all_day = ?5, start_utc = ?6,
             end_utc = ?7, start_tz = ?8, end_tz = ?9, recurrence = ?10,
             use_default_reminders = ?11
         WHERE id = ?1",
        params![
            id,
            data.title,
            data.description,
            data.location,
            data.all_day,
            data.start_utc,
            data.end_utc,
            data.start_tz,
            data.end_tz,
            data.recurrence,
            data.reminders.use_default,
        ],
    )?;
    c.execute("DELETE FROM event_reminders WHERE event_id = ?1", [id])?;
    for reminder in &data.reminders.overrides {
        c.execute(
            "INSERT OR IGNORE INTO event_reminders (event_id, method, minutes_before)
             VALUES (?1, ?2, ?3)",
            params![id, reminder.method.as_str(), reminder.minutes_before],
        )?;
    }
    Ok(())
}

/// The source of `calendar_id` if it is remote, so its edits go through the outbox.
pub fn remote_source(c: &Connection, calendar_id: i64) -> rusqlite::Result<Option<String>> {
    let found: Option<(String, String)> = c
        .query_row(
            "SELECT s.id, s.kind FROM calendars c JOIN sources s ON s.id = c.source_id
             WHERE c.id = ?1",
            [calendar_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(found
        .filter(|(_, kind)| kind != SourceKind::Local.as_str())
        .map(|(id, _)| id))
}

/// Queues `op` for event `id` as it is now, if its calendar is remote. `base` is the event
/// before the edit (`None` for a create); the payload is read from the row (`None` for a
/// delete). Call it in the transaction of the edit, after the edit. Returns the source to sync,
/// or `None` if nothing was queued: a local calendar, or an update that changed nothing.
pub fn queue(
    c: &Connection,
    op: Op,
    id: i64,
    base: Option<&EventData>,
    now: i64,
) -> rusqlite::Result<Option<String>> {
    let (calendar_id, remote_id, etag): (i64, String, Option<String>) = c.query_row(
        "SELECT calendar_id, remote_id, etag FROM events WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let Some(source) = remote_source(c, calendar_id)? else {
        return Ok(None);
    };
    let payload = match op {
        Op::Delete => None,
        Op::Create | Op::Update => read_data(c, id)?,
    };
    if op == Op::Update && payload.as_ref() == base {
        return Ok(None);
    }
    // Rust note: serialising these plain structs cannot fail, so `expect` never fires.
    let json = |data: &EventData| serde_json::to_string(data).expect("event data serialises");
    c.execute(
        "INSERT INTO outbox (calendar_id, event_id, event_remote_id, op, payload, base,
                             base_etag, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            calendar_id,
            id,
            remote_id,
            op.as_str(),
            payload.as_ref().map(json),
            base.map(json),
            etag,
            now,
        ],
    )?;
    Ok(Some(source))
}

/// One row the engine sends.
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: i64,
    pub calendar_id: i64,
    pub event_id: Option<i64>,
    pub event_remote_id: String,
    pub op: Op,
    pub payload: Option<EventData>,
    pub base: Option<EventData>,
    /// The ETag to send as `If-Match`: the event's own, which our earlier rows for it have
    /// kept current, else the one the edit was based on.
    pub if_match: Option<String>,
}

/// The oldest pending row of `source_id`'s calendars, in `id` order (§7.3).
pub fn next_pending(c: &Connection, source_id: &str) -> rusqlite::Result<Option<Pending>> {
    c.query_row(
        "SELECT o.id, o.calendar_id, o.event_id, o.event_remote_id, o.op, o.payload, o.base,
                coalesce(e.etag, o.base_etag)
         FROM outbox o
         JOIN calendars c ON c.id = o.calendar_id
         LEFT JOIN events e ON e.id = o.event_id
         WHERE c.source_id = ?1 AND o.status = 'pending'
         ORDER BY o.id
         LIMIT 1",
        [source_id],
        |r| {
            let op: String = r.get(4)?;
            let data = |i: usize| -> rusqlite::Result<Option<EventData>> {
                let text: Option<String> = r.get(i)?;
                // A value this code did not write reads as missing; the engine then fails the
                // row rather than sending something it cannot read.
                Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
            };
            Ok(Pending {
                id: r.get(0)?,
                calendar_id: r.get(1)?,
                event_id: r.get(2)?,
                event_remote_id: r.get(3)?,
                // The column's `CHECK` allows only these three.
                op: Op::parse(&op).unwrap_or(Op::Update),
                payload: data(5)?,
                base: data(6)?,
                if_match: r.get(7)?,
            })
        },
    )
    .optional()
}

/// Marks row `id` `conflicted` or `failed`, keeping its payload so the change is not lost (§7.3).
pub fn set_outcome(
    c: &Connection,
    id: i64,
    status: &str,
    last_error: &str,
) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE outbox SET status = ?2, last_error = ?3, attempts = attempts + 1 WHERE id = ?1",
        params![id, status, last_error],
    )?;
    Ok(())
}

/// Removes a row that was sent.
pub fn remove(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
    Ok(())
}

/// Whether event `id` has a row still waiting to be sent. A pull leaves such an event alone,
/// so the server's copy never overwrites an edit that is about to be sent (§7.4).
pub fn has_pending(c: &Connection, id: i64) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS (SELECT 1 FROM outbox
                        WHERE event_id = ?1 AND status IN ('pending', 'in_flight'))",
        [id],
        |r| r.get(0),
    )
}
