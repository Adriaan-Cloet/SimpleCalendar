//! The outbox: local edits to remote calendars, waiting to be sent (`docs/architecture.md`
//! §7.3).
//!
//! A write to a remote calendar changes `events` and queues a row here in the same transaction,
//! so the UI shows the edit at once and the sync engine sends it before its next pull. A row
//! holds the event as the user wants it (`payload`) and as it was before (`base`); the engine
//! sends the difference. Writes to local calendars queue nothing.
//!
//! A row is `pending` until the engine takes it (`claim`), then `in_flight` while it is sent.
//! A second edit to an event with a pending row merges into it. A row that could not be sent
//! goes back to `pending` with a later `next_attempt_at`; one the server refused, or that met
//! a newer version there, stays as `failed` or `conflicted`.

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
/// or `None` if nothing is left to send: a local calendar, an update that changed nothing, or
/// one that undid the edit still waiting, or the delete of an event the server never had.
///
/// An edit to an event that already has a `pending` row is merged into it (§7.3), so the
/// server only hears the end result. A row that is `in_flight` is being sent and is left
/// alone: the edit gets a row of its own, sent after it.
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

    let waiting: Option<(i64, String, Option<String>)> = c
        .query_row(
            "SELECT id, op, base FROM outbox WHERE event_id = ?1 AND status = 'pending'
             ORDER BY id DESC LIMIT 1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((row, earlier, earlier_base)) = waiting {
        // The column's `CHECK` allows only these three.
        let earlier = Op::parse(&earlier).unwrap_or(Op::Update);
        let earlier_base: Option<EventData> =
            earlier_base.and_then(|t| serde_json::from_str(&t).ok());
        match (earlier, op) {
            // The server never heard of it, and now never will.
            (Op::Create, Op::Delete) => {
                c.execute(
                    "DELETE FROM outbox WHERE status = 'pending' AND (id = ?1 OR event_id IN
                         (SELECT id FROM events WHERE recurring_event_id = ?2))",
                    params![row, id],
                )?;
                c.execute("DELETE FROM events WHERE id = ?1", [id])?;
                return Ok(None);
            }
            // Still a create, now of the event as it is.
            (Op::Create, _) => {
                c.execute(
                    "UPDATE outbox SET payload = ?2 WHERE id = ?1",
                    params![row, payload.as_ref().map(json)],
                )?;
                return Ok(Some(source));
            }
            // Against the original `base`, which is what the server has.
            (Op::Update, Op::Update) if payload == earlier_base => {
                c.execute("DELETE FROM outbox WHERE id = ?1", [row])?;
                return Ok(None);
            }
            (Op::Update, Op::Update | Op::Delete) => {
                c.execute(
                    "UPDATE outbox SET op = ?2, payload = ?3 WHERE id = ?1",
                    params![row, op.as_str(), payload.as_ref().map(json)],
                )?;
                return Ok(Some(source));
            }
            // A deleted event is hidden, so it cannot be edited again; a create of an id that
            // already has a row cannot happen. Queue it as it is.
            (Op::Delete, _) | (Op::Update, Op::Create) => {}
        }
    }

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
    /// The attempts that failed so far.
    pub attempts: i64,
    next_attempt_at: i64,
}

/// Takes the oldest pending row of `source_id`'s calendars, in `id` order (§7.3), and marks it
/// `in_flight`. `None` if there is none, or if it is still backing off from a failed attempt
/// and `ignore_backoff` (a sync the user asked for) is not set: the rows behind it wait too,
/// so the order holds.
///
/// Call it on the write connection: an edit then cannot merge into the row between reading it
/// and marking it, which would lose that edit when the row is removed after sending.
pub fn claim(
    c: &Connection,
    source_id: &str,
    now: i64,
    ignore_backoff: bool,
) -> rusqlite::Result<Option<Pending>> {
    let Some(row) = next_pending(c, source_id)? else {
        return Ok(None);
    };
    if row.next_attempt_at > now && !ignore_backoff {
        return Ok(None);
    }
    c.execute(
        "UPDATE outbox SET status = 'in_flight' WHERE id = ?1",
        [row.id],
    )?;
    Ok(Some(row))
}

/// The oldest pending row of `source_id`'s calendars, without taking it.
pub fn next_pending(c: &Connection, source_id: &str) -> rusqlite::Result<Option<Pending>> {
    c.query_row(
        "SELECT o.id, o.calendar_id, o.event_id, o.event_remote_id, o.op, o.payload, o.base,
                coalesce(e.etag, o.base_etag), o.attempts, o.next_attempt_at
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
                attempts: r.get(8)?,
                next_attempt_at: r.get(9)?,
            })
        },
    )
    .optional()
}

/// Puts a row that could not be sent back in line, to be tried again from `next_attempt_at`
/// (epoch seconds) on. `error` is why, or `None` when the attempt does not count, as when the
/// sign-in was rejected.
pub fn retry_later(
    c: &Connection,
    id: i64,
    next_attempt_at: i64,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE outbox
         SET status = 'pending', next_attempt_at = ?2,
             attempts = attempts + (?3 IS NOT NULL), last_error = coalesce(?3, last_error)
         WHERE id = ?1",
        params![id, next_attempt_at, error],
    )?;
    Ok(())
}

/// When the earliest row of `source_id` that is backing off may be tried again, if one is and
/// that is after `now`: the engine syncs the source again then.
pub fn next_retry(c: &Connection, source_id: &str, now: i64) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT min(o.next_attempt_at) FROM outbox o
         JOIN calendars c ON c.id = o.calendar_id
         WHERE c.source_id = ?1 AND o.status = 'pending' AND o.next_attempt_at > ?2",
        params![source_id, now],
        |r| r.get(0),
    )
}

/// Puts the rows that were being sent when the app stopped back in line (§7.3). Whether the
/// server got them is unknown; sending again is safe, because a create carries its own id and
/// an update or delete its `If-Match`. Returns how many there were.
pub fn recover(c: &Connection) -> rusqlite::Result<usize> {
    c.execute(
        "UPDATE outbox SET status = 'pending' WHERE status = 'in_flight'",
        [],
    )
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
