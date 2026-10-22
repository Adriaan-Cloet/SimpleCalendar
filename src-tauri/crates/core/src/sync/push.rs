//! Step 2 of a cycle: sending the outbox (`docs/architecture.md` §7.2, §7.3).
//!
//! Rows go out one at a time in `id` order, so edits to one event never overtake each other.
//! What happens after each attempt:
//!
//! | Answer                         | Then                                                      |
//! | ------------------------------ | --------------------------------------------------------- |
//! | Success                        | Store what the server sent back (its ETag), drop the row  |
//! | Nothing changed                | Drop the row without a request                            |
//! | Network, 5xx, 429, or sign-in  | Stop: the row waits, and the cycle skips its pull         |
//! | 412, or the event is gone      | Fetch it: if it already has our change, a success; else   |
//! |                                | the server's copy stays, the row becomes `conflicted`     |
//! | Refused for good               | Roll the event back, the row becomes `failed`             |
//!
//! Stopping at the first row that cannot be sent keeps the order; retrying it later with
//! backoff is the offline outbox of v0.3. The server's version is never overwritten: every
//! update and delete carries `If-Match` when there is an ETag (§7.4).

use rusqlite::OptionalExtension;

use crate::outbox::{self, Op, Pending};
use crate::provider::{
    CalendarProvider, CalendarRef, Etag, EventData, EventUpdate, ProviderError, RemoteEvent,
    SourceContext, WritableProvider,
};

use super::SyncError;
use super::cycle::Ctx;
use super::store::upsert_event;
use crate::CoreEvent;

/// What sending one row did.
//
// Rust note: the event is boxed (kept on the heap) so the other variants stay small, as in
// `provider::google::json::Change`.
enum Sent {
    /// The server has the event as it sent back.
    Stored(Box<RemoteEvent>),
    Deleted,
    /// The row changed nothing; no request was made.
    Unchanged,
}

/// What becomes of a row.
enum Outcome {
    Done(Sent),
    /// The server's copy differs and stays. `None` if the event is gone there.
    Conflicted(Option<Box<RemoteEvent>>),
    Failed(String),
}

/// Sends the source's pending rows in order. Returns an error, and leaves the rest waiting,
/// when one cannot be sent now: the cycle then reports it and does not pull, so a pull never
/// meets an edit that is still on its way.
pub(super) async fn push(
    ctx: &Ctx,
    cx: &SourceContext,
    provider: &dyn CalendarProvider,
) -> Result<(), SyncError> {
    loop {
        let source = cx.source_id.clone();
        let next = ctx
            .db
            .read(move |c| {
                let Some(row) = outbox::next_pending(c, &source)? else {
                    return Ok(None);
                };
                let calendar = c.query_row(
                    "SELECT remote_id, time_zone FROM calendars WHERE id = ?1",
                    [row.calendar_id],
                    |r| {
                        Ok(CalendarRef {
                            remote_id: r.get(0)?,
                            time_zone: r.get(1)?,
                        })
                    },
                )?;
                Ok(Some((row, calendar)))
            })
            .await?;
        let Some((row, calendar)) = next else {
            return Ok(());
        };
        let Some(writer) = provider.as_writable() else {
            // Only a calendar whose provider writes is writable (§4), so this needs a provider
            // that stopped writing. The rows wait rather than being lost.
            log::warn!(
                "source {}: rows wait, but its provider does not write",
                cx.source_id
            );
            return Ok(());
        };

        let outcome = match send(writer, cx, &calendar, &row).await {
            Ok(sent) => Outcome::Done(sent),
            // Rust note: `e @ pattern` binds the whole value that matched, so the error can be
            // passed on as it is.
            Err(e @ (ProviderError::Transient { .. } | ProviderError::Auth)) => {
                return Err(SyncError::Provider(e));
            }
            Err(ProviderError::Conflict) => check(writer, cx, &calendar, &row).await?,
            Err(ProviderError::NotFound) if row.op == Op::Update => {
                check(writer, cx, &calendar, &row).await?
            }
            Err(other) => Outcome::Failed(other.to_string()),
        };
        apply(ctx, row, outcome).await?;
    }
}

/// Sends one row.
async fn send(
    writer: &dyn WritableProvider,
    cx: &SourceContext,
    calendar: &CalendarRef,
    row: &Pending,
) -> Result<Sent, ProviderError> {
    let if_match = row.if_match.clone().map(Etag);
    match row.op {
        Op::Create => {
            let payload = readable(&row.payload)?;
            let event = writer
                .create_event(cx, calendar, &row.event_remote_id, payload)
                .await?;
            Ok(Sent::Stored(Box::new(event)))
        }
        Op::Update => {
            let (payload, base) = (readable(&row.payload)?, readable(&row.base)?);
            let changed = payload.changed_from(base);
            if changed.is_empty() {
                return Ok(Sent::Unchanged);
            }
            let update = EventUpdate {
                remote_id: row.event_remote_id.clone(),
                desired: payload.clone(),
                changed,
            };
            let event = writer
                .update_event(cx, calendar, &update, if_match.as_ref())
                .await?;
            Ok(Sent::Stored(Box::new(event)))
        }
        Op::Delete => {
            writer
                .delete_event(cx, calendar, &row.event_remote_id, if_match.as_ref())
                .await?;
            Ok(Sent::Deleted)
        }
    }
}

/// A row's payload or base, or why it cannot be sent.
fn readable(data: &Option<EventData>) -> Result<&EventData, ProviderError> {
    data.as_ref()
        .ok_or_else(|| ProviderError::Permanent("the change cannot be read".into()))
}

/// After a 412, or an update of an event the server no longer has: fetches it once (§7.4). If
/// it already has our value in every changed field, an earlier attempt got through and only
/// its answer was lost, so this is a success. Otherwise the server wins.
async fn check(
    writer: &dyn WritableProvider,
    cx: &SourceContext,
    calendar: &CalendarRef,
    row: &Pending,
) -> Result<Outcome, SyncError> {
    let remote = writer.get_event(cx, calendar, &row.event_remote_id).await?;
    Ok(match (row.op, remote) {
        (Op::Delete, None) => Outcome::Done(Sent::Deleted),
        (Op::Update, Some(remote)) if has_our_change(row, &remote) => {
            Outcome::Done(Sent::Stored(Box::new(remote)))
        }
        (_, remote) => Outcome::Conflicted(remote.map(Box::new)),
    })
}

/// Whether `remote` has the payload's value in every field the row changes.
fn has_our_change(row: &Pending, remote: &RemoteEvent) -> bool {
    let (Some(payload), Some(base)) = (&row.payload, &row.base) else {
        return false;
    };
    let theirs = EventData::from(remote);
    payload
        .changed_from(base)
        .into_iter()
        .all(|field| payload.same(&theirs, field))
}

/// Stores what became of `row`, in one transaction, and tells the views.
async fn apply(ctx: &Ctx, row: Pending, outcome: Outcome) -> Result<(), SyncError> {
    let calendar_id = row.calendar_id;
    let changed = ctx
        .db
        .write(move |c| {
            let tx = c.transaction()?;
            let changed = match outcome {
                Outcome::Done(Sent::Unchanged) => {
                    outbox::remove(&tx, row.id)?;
                    false
                }
                Outcome::Done(Sent::Stored(event)) => {
                    outbox::remove(&tx, row.id)?;
                    store_answer(&tx, &row, &event)?;
                    true
                }
                Outcome::Done(Sent::Deleted) => {
                    outbox::remove(&tx, row.id)?;
                    // A deleted occurrence keeps its cancelled exception row, which hides it;
                    // a deleted event or series goes.
                    if let Some(id) = row.event_id {
                        tx.execute(
                            "DELETE FROM events WHERE id = ?1 AND pending_delete = 1",
                            [id],
                        )?;
                    }
                    true
                }
                Outcome::Conflicted(remote) => {
                    log::warn!(
                        "event {}: changed elsewhere; the server's version stays",
                        row.event_remote_id
                    );
                    outbox::set_outcome(
                        &tx,
                        row.id,
                        "conflicted",
                        "The event was changed elsewhere, so this change was not saved.",
                    )?;
                    match (remote, row.event_id) {
                        (Some(event), Some(id)) => {
                            tx.execute("UPDATE events SET pending_delete = 0 WHERE id = ?1", [id])?;
                            upsert_event(&tx, calendar_id, &event)?;
                        }
                        (None, Some(id)) => {
                            tx.execute("DELETE FROM events WHERE id = ?1", [id])?;
                        }
                        (_, None) => {}
                    }
                    true
                }
                Outcome::Failed(why) => {
                    log::warn!("event {}: not saved: {why}", row.event_remote_id);
                    outbox::set_outcome(&tx, row.id, "failed", &why)?;
                    roll_back(&tx, &row)?;
                    true
                }
            };
            tx.commit()?;
            Ok(changed)
        })
        .await?;
    if changed {
        ctx.emit(CoreEvent::CalendarChanged {
            calendar_ids: vec![calendar_id],
        });
    }
    Ok(())
}

/// Stores the event the server sent back. If more of our edits to it wait, only its ETag: the
/// row keeps showing the latest edit, and the next row is checked against this version.
fn store_answer(
    tx: &rusqlite::Transaction,
    row: &Pending,
    event: &RemoteEvent,
) -> rusqlite::Result<()> {
    let waiting = match row.event_id {
        Some(id) => outbox::has_pending(tx, id)?,
        None => false,
    };
    if waiting {
        tx.execute(
            "UPDATE events SET etag = ?2 WHERE id = ?1",
            rusqlite::params![row.event_id, event.etag],
        )?;
        return Ok(());
    }
    upsert_event(tx, row.calendar_id, event)
}

/// Undoes the local edit of a row the server refused for good (§7.3).
fn roll_back(tx: &rusqlite::Transaction, row: &Pending) -> rusqlite::Result<()> {
    let Some(id) = row.event_id else {
        return Ok(());
    };
    match (row.op, &row.base) {
        // The server never had it.
        (Op::Create, _) => {
            tx.execute("DELETE FROM events WHERE id = ?1", [id])?;
        }
        (Op::Update, Some(base)) => outbox::write_data(tx, id, base)?,
        // A deleted event shows again; a deleted occurrence too, as it was.
        (Op::Delete, base) => {
            tx.execute("UPDATE events SET pending_delete = 0 WHERE id = ?1", [id])?;
            if let Some(base) = base {
                let exception: Option<bool> = tx
                    .query_row(
                        "SELECT recurring_remote_id IS NOT NULL FROM events WHERE id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if exception == Some(true) {
                    tx.execute("UPDATE events SET status = 'confirmed' WHERE id = ?1", [id])?;
                    outbox::write_data(tx, id, base)?;
                }
            }
        }
        (Op::Update, None) => {}
    }
    Ok(())
}
