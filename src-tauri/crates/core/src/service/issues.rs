//! The sync issues list (#21, `docs/architecture.md` §7.3, §10): changes the server did not
//! save, because the event had changed there (`conflicted`, §7.4) or because it refused them
//! (`failed`). The outbox keeps each one with its payload until the user dismisses or redoes it.
//!
//! **Redo** makes the kept change again, as a new edit on what the event is now:
//!
//! | Change | Event here                 | Redo                                                |
//! | ------ | -------------------------- | --------------------------------------------------- |
//! | create | gone (a failed create)     | Create it again, under a new id                     |
//! | update | still here, may be edited  | Write the kept values over it, queue an update      |
//! | update | gone (deleted elsewhere)   | Create it again, under a new id                     |
//! | delete | still here, may be edited  | Delete it again                                     |
//! | delete | gone                       | Nothing to do; the issue goes                       |
//!
//! An occurrence of a series that was deleted elsewhere comes back as a single event: the
//! series no longer has it, so there is nothing to attach it to.
//
// Rust note: like `events.rs`, this child module adds methods to `CalendarService` in its own
// `impl` block.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::events::{EVENT_GONE, now};
use super::series::{drop_exceptions, pull_again};
use super::{CalendarService, ServiceError};
use crate::CoreEvent;
use crate::outbox::{self, Issue, Op};
use crate::provider::{EventData, Field};

const ISSUE_GONE: &str = "This change is no longer in the list.";
const UNREADABLE: &str = "This change cannot be read, so it cannot be made again.";

/// Why a change was not saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// The event had changed on the server, which kept its version (§7.4).
    Conflicted,
    /// The server refused the change; the event was rolled back (§7.3).
    Failed,
}

/// What the user changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueChange {
    Create,
    Update,
    Delete,
}

/// A change that was not saved, as the sync issues list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncIssue {
    /// The outbox row, which `sync:conflict` names too.
    pub id: i64,
    pub kind: IssueKind,
    pub change: IssueChange,
    /// The event's title in the user's change; for a delete, the title it had.
    pub title: String,
    /// Why it was not saved.
    pub reason: String,
    pub calendar_id: i64,
    pub calendar_name: String,
    /// When the change was made, in epoch seconds.
    pub created_at: i64,
    /// Whether `redo_sync_issue` can make the change again. When not, only dismissing is left.
    pub can_redo: bool,
}

impl CalendarService {
    /// The changes that were not saved, newest first.
    pub async fn list_sync_issues(&self) -> Result<Vec<SyncIssue>, ServiceError> {
        Ok(self
            .db
            .read(|c| {
                outbox::issues(c, None)?
                    .into_iter()
                    .map(|issue| sync_issue(c, issue))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await?)
    }

    /// Forgets a change that was not saved. The event stays as the server has it.
    pub async fn dismiss_sync_issue(&self, id: i64) -> Result<(), ServiceError> {
        let removed = self
            .db
            .write(move |c| {
                c.execute(
                    "DELETE FROM outbox WHERE id = ?1 AND status IN ('conflicted', 'failed')",
                    [id],
                )
            })
            .await?;
        if removed == 0 {
            return Err(ServiceError::NotFound(ISSUE_GONE));
        }
        Ok(())
    }

    /// Makes a change that was not saved again, as a new edit (see the module comment), and
    /// removes it from the list. Like every write it checks that the event and its calendar
    /// may be changed (§4).
    pub async fn redo_sync_issue(&self, id: i64) -> Result<(), ServiceError> {
        let (calendar_id, push) = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let Some(issue) = outbox::issues(&tx, Some(id))?.pop() else {
                    return Ok(Err(ServiceError::NotFound(ISSUE_GONE)));
                };
                let push = match redo(&tx, &issue)? {
                    Ok(push) => push,
                    Err(refused) => return Ok(Err(refused)),
                };
                outbox::remove(&tx, id)?;
                tx.commit()?;
                Ok(Ok((issue.row.calendar_id, push)))
            })
            .await??;
        if let Some(source) = push {
            self.sync.local_write(source);
        }
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![calendar_id],
        });
        Ok(())
    }
}

/// The event an issue is about, as it is here now.
struct Current {
    id: i64,
    /// Deleted here and waiting for the server, or a deleted occurrence.
    gone: bool,
    /// Its organiser lets this account change it (§4).
    editable: bool,
    exception: bool,
    series: bool,
}

fn current(c: &Connection, issue: &Issue) -> rusqlite::Result<Option<Current>> {
    let Some(id) = issue.row.event_id else {
        return Ok(None);
    };
    c.query_row(
        "SELECT pending_delete = 1 OR status = 'cancelled', is_organizer OR guests_can_modify,
                recurring_remote_id IS NOT NULL, recurrence IS NOT NULL
         FROM events WHERE id = ?1",
        [id],
        |r| {
            Ok(Current {
                id,
                gone: r.get(0)?,
                editable: r.get(1)?,
                exception: r.get(2)?,
                series: r.get(3)?,
            })
        },
    )
    .optional()
}

fn sync_issue(c: &Connection, issue: Issue) -> rusqlite::Result<SyncIssue> {
    let event = current(c, &issue)?;
    let row = &issue.row;
    let readable = row.op == Op::Delete || row.payload.is_some();
    let can_redo = issue.calendar_writable
        && readable
        && match (row.op, &event) {
            (Op::Delete, None) => false,
            (_, Some(event)) => !event.gone && event.editable,
            (_, None) => true,
        };
    let title = row
        .payload
        .as_ref()
        .or(row.base.as_ref())
        .map(|data| data.title.clone())
        .unwrap_or_default();
    Ok(SyncIssue {
        id: row.id,
        kind: if issue.conflicted {
            IssueKind::Conflicted
        } else {
            IssueKind::Failed
        },
        change: match row.op {
            Op::Create => IssueChange::Create,
            Op::Update => IssueChange::Update,
            Op::Delete => IssueChange::Delete,
        },
        title,
        reason: issue.reason.unwrap_or_default(),
        calendar_id: row.calendar_id,
        calendar_name: issue.calendar_name,
        created_at: issue.created_at,
        can_redo,
    })
}

/// Makes the change of `issue` again. Returns the source to sync, if a row was queued.
fn redo(c: &Connection, issue: &Issue) -> rusqlite::Result<Result<Option<String>, ServiceError>> {
    if !issue.calendar_writable {
        return Ok(Err(ServiceError::NotEditable));
    }
    let row = &issue.row;
    let event = current(c, issue)?;
    match &event {
        // Already gone: what the user wanted.
        None if row.op == Op::Delete => return Ok(Ok(None)),
        Some(event) if event.gone => return Ok(Err(ServiceError::NotFound(EVENT_GONE))),
        Some(event) if !event.editable => return Ok(Err(ServiceError::NotEditable)),
        _ => {}
    }

    if row.op == Op::Delete {
        let Some(event) = event else {
            return Ok(Ok(None));
        };
        let base = outbox::read_data(c, event.id)?;
        // A deleted occurrence is a cancelled exception, as `series::cancel` makes it.
        let hide = if event.exception {
            "UPDATE events SET status = 'cancelled' WHERE id = ?1"
        } else {
            "UPDATE events SET pending_delete = 1 WHERE id = ?1"
        };
        c.execute(hide, [event.id])?;
        return Ok(Ok(outbox::queue(
            c,
            Op::Delete,
            event.id,
            base.as_ref(),
            now(),
        )?));
    }

    let Some(payload) = &row.payload else {
        return Ok(Err(ServiceError::InvalidInput(UNREADABLE.into())));
    };
    match event {
        Some(event) => {
            let Some(before) = outbox::read_data(c, event.id)? else {
                return Ok(Err(ServiceError::NotFound(EVENT_GONE)));
            };
            outbox::write_data(c, event.id, payload)?;
            // The server decides where the exceptions of a series go when its times or rule
            // change, as for "all events" (`series.rs`): only a full pull brings them back.
            let reshaped =
                !before.same(payload, Field::Times) || !before.same(payload, Field::Recurrence);
            if event.series && reshaped && outbox::remote_source(c, row.calendar_id)?.is_some() {
                drop_exceptions(c, event.id)?;
                pull_again(c, row.calendar_id)?;
            }
            Ok(Ok(outbox::queue(
                c,
                Op::Update,
                event.id,
                Some(&before),
                now(),
            )?))
        }
        None => Ok(Ok(create(c, row.calendar_id, payload)?)),
    }
}

/// Creates the event `data` in `calendar_id` under a new id, as `create_event` does, and queues
/// it. Returns the source to sync.
fn create(c: &Connection, calendar_id: i64, data: &EventData) -> rusqlite::Result<Option<String>> {
    let remote_id = uuid::Uuid::new_v4().simple().to_string();
    c.execute(
        "INSERT INTO events (calendar_id, remote_id, title, start_utc, end_utc)
         VALUES (?1, ?2, '', 0, 0)",
        params![calendar_id, remote_id],
    )?;
    let id = c.last_insert_rowid();
    outbox::write_data(c, id, data)?;
    outbox::queue(c, Op::Create, id, None, now())
}
