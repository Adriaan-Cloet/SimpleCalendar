//! The sync engine (`docs/architecture.md` §7.1, §7.2): it sends the outbox, then pulls.
//!
//! One long-lived task owns a queue of commands. It runs at most one cycle per source at a
//! time; a request that arrives during a cycle sets a "run again" flag instead of starting a
//! second one. Different sources sync in parallel. The engine is the only component that talks
//! to providers and the only one that applies remote changes to the database.

mod cycle;
mod push;
pub(crate) mod store;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Instant;

use cycle::{Ctx, SyncError};

use crate::CoreEvent;
use crate::db::Db;
use crate::model::SourceKind;
use crate::outbox;
use crate::provider::Providers;
use crate::secrets::Secrets;

/// What a source is doing, as `sync:status` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Syncing,
    /// The last cycle succeeded.
    Ok,
    /// The last cycle failed; `last_error` says why. The timer tries again.
    Error,
    /// The credentials were rejected. Automatic syncs skip the source until the user signs in
    /// again; a manual sync still tries.
    AuthRequired,
}

/// The payload of `sync:status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncStatus {
    pub source_id: String,
    pub state: SyncState,
    pub last_error: Option<String>,
}

/// The payload of `sync:conflict`: a change that met a newer version on the server, which won
/// (§7.4). The outbox row keeps the change for the sync issues list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncConflict {
    pub outbox_id: i64,
    /// The event's title as the user last saw it.
    pub title: String,
}

/// How often the timer syncs a source of this kind, unless `sources.sync_interval_s` overrides
/// it. `None` for kinds that are never synced.
pub fn default_interval(kind: SourceKind) -> Option<Duration> {
    const MINUTE: u64 = 60;
    match kind {
        SourceKind::Ics => Some(Duration::from_secs(60 * MINUTE)),
        SourceKind::Google | SourceKind::Caldav => Some(Duration::from_secs(15 * MINUTE)),
        SourceKind::Local => None,
    }
}

/// A handle to send requests to a running engine. Cloning is cheap.
#[derive(Debug, Clone)]
pub struct SyncHandle {
    tx: mpsc::UnboundedSender<Msg>,
}

impl SyncHandle {
    /// Syncs every remote source, as at app start. Sources that need a new sign-in are skipped.
    pub fn sync_all(&self) {
        self.send(Msg::Sync {
            source_id: None,
            manual: false,
        });
    }

    /// Syncs one source, or every remote source for `None`, on the user's request (`sync_now`).
    /// This also tries sources that need a new sign-in.
    pub fn sync_now(&self, source_id: Option<String>) {
        self.send(Msg::Sync {
            source_id,
            manual: true,
        });
    }

    /// Syncs `source_id` soon because a local edit queued an outbox row for it: the cycle sends
    /// it before pulling (§7.2). Like the timer, this skips a source that needs a new sign-in;
    /// the row waits until the user signs in again.
    pub fn push(&self, source_id: String) {
        self.send(Msg::Sync {
            source_id: Some(source_id),
            manual: false,
        });
    }

    /// Stops the engine. Cycles that are running are cancelled at their next `.await`; a page
    /// being applied is either committed whole or not at all.
    pub fn shutdown(&self) {
        self.send(Msg::Shutdown);
    }

    fn send(&self, msg: Msg) {
        // Only fails when the engine has stopped, and then there is nothing left to sync.
        if self.tx.send(msg).is_err() {
            log::debug!("sync engine has stopped; request ignored");
        }
    }
}

#[cfg(test)]
impl SyncHandle {
    /// A handle that no engine reads, with the queue it writes to. For testing callers of the
    /// engine without running one.
    pub(crate) fn detached() -> (SyncHandle, Requests) {
        let (tx, rx) = mpsc::unbounded_channel();
        (SyncHandle { tx }, Requests(rx))
    }
}

/// What was sent to a detached handle.
#[cfg(test)]
pub(crate) struct Requests(mpsc::UnboundedReceiver<Msg>);

#[cfg(test)]
impl Requests {
    /// The sources of the `sync_now` requests since the last call, in order.
    pub(crate) fn manual(&mut self) -> Vec<Option<String>> {
        let mut requested = Vec::new();
        while let Ok(msg) = self.0.try_recv() {
            if let Msg::Sync {
                source_id,
                manual: true,
            } = msg
            {
                requested.push(source_id);
            }
        }
        requested
    }

    /// The sources of the `push` requests since the last call, in order.
    pub(crate) fn pushes(&mut self) -> Vec<String> {
        let mut requested = Vec::new();
        while let Ok(msg) = self.0.try_recv() {
            if let Msg::Sync {
                source_id: Some(source_id),
                manual: false,
            } = msg
            {
                requested.push(source_id);
            }
        }
        requested
    }
}

/// Requests to the engine task, including the ones it sends itself.
#[derive(Debug)]
enum Msg {
    /// Sync one source, or all remote sources for `None`. `manual` is a user request.
    Sync {
        source_id: Option<String>,
        manual: bool,
    },
    /// A cycle ended. `next` is when the timer should sync the source again; `None` if it is
    /// gone or is never synced.
    Finished {
        source_id: String,
        next: Option<Duration>,
    },
    Shutdown,
}

/// The engine. Create it with `new`, then drive `run` on the async runtime.
//
// Rust note: the engine is driven by one task, so its bookkeeping (`running`, `again`, `due`)
// needs no lock. Cycles run as separate tasks and only talk back through a `Finished` message.
pub struct SyncEngine {
    ctx: Arc<Ctx>,
    rx: mpsc::UnboundedReceiver<Msg>,
    /// For cycles to report `Finished`. Weak, so that the engine stops once every handle is
    /// dropped instead of keeping its own queue open forever.
    tx: mpsc::WeakUnboundedSender<Msg>,
    /// Sources with a cycle in progress.
    running: HashSet<String>,
    /// Sources that were requested again during their cycle. The value is whether any of
    /// those requests was manual.
    again: HashMap<String, bool>,
    /// When the timer syncs each idle source next.
    due: HashMap<String, Instant>,
    /// The running cycles. Dropping the set cancels them.
    tasks: JoinSet<()>,
}

impl SyncEngine {
    /// The engine and a handle to it. Nothing runs until `run` is awaited or spawned.
    ///
    /// `events` receives a `CoreEvent` after every commit that changed calendars, and at the
    /// start and end of every cycle.
    pub fn new(
        db: Db,
        secrets: Secrets,
        providers: Providers,
        events: mpsc::UnboundedSender<CoreEvent>,
    ) -> (SyncEngine, SyncHandle) {
        let (tx, rx) = mpsc::unbounded_channel();
        let engine = SyncEngine {
            ctx: Arc::new(Ctx {
                db,
                secrets,
                providers,
                events,
            }),
            rx,
            tx: tx.downgrade(),
            running: HashSet::new(),
            again: HashMap::new(),
            due: HashMap::new(),
            tasks: JoinSet::new(),
        };
        (engine, SyncHandle { tx })
    }

    /// Runs until `shutdown` is called or every handle is dropped.
    pub async fn run(mut self) {
        // Rows that were being sent when the app last stopped go back in line (§7.3).
        match self.ctx.db.write(|c| outbox::recover(c)).await {
            Ok(0) => {}
            Ok(n) => log::info!("outbox: {n} rows were being sent at the last stop; sent again"),
            Err(e) => log::error!("outbox: cannot put interrupted rows back: {e}"),
        }
        loop {
            let msg = match self.due.values().min().copied() {
                // Waits for a message, but no longer than until the next source is due.
                Some(at) => match tokio::time::timeout_at(at, self.rx.recv()).await {
                    Ok(msg) => msg,
                    Err(_elapsed) => {
                        self.start_due();
                        continue;
                    }
                },
                None => self.rx.recv().await,
            };
            match msg {
                Some(Msg::Sync {
                    source_id: Some(id),
                    manual,
                }) => self.start(id, manual),
                Some(Msg::Sync {
                    source_id: None,
                    manual,
                }) => self.start_all(manual).await,
                Some(Msg::Finished { source_id, next }) => self.finished(source_id, next),
                Some(Msg::Shutdown) | None => break,
            }
        }
    }

    /// Starts a cycle of every remote source.
    async fn start_all(&mut self, manual: bool) {
        match self.ctx.db.read(store::remote_source_ids).await {
            Ok(ids) => {
                for id in ids {
                    self.start(id, manual);
                }
            }
            Err(e) => log::error!("cannot list sources to sync: {e}"),
        }
    }

    /// Starts a cycle of `source_id`, or asks for one more if a cycle is running.
    fn start(&mut self, source_id: String, manual: bool) {
        if self.running.contains(&source_id) {
            *self.again.entry(source_id).or_default() |= manual;
            return;
        }
        let Some(tx) = self.tx.upgrade() else {
            // Every handle is gone and the engine is about to stop.
            return;
        };
        self.due.remove(&source_id);
        self.running.insert(source_id.clone());
        let ctx = Arc::clone(&self.ctx);
        self.tasks.spawn(async move {
            let next = cycle::run(&ctx, &source_id, manual).await;
            let _ = tx.send(Msg::Finished { source_id, next });
        });
    }

    /// Starts the sources whose timer has run out.
    fn start_due(&mut self) {
        let now = Instant::now();
        let ready: Vec<String> = self
            .due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            self.start(id, false);
        }
    }

    fn finished(&mut self, source_id: String, next: Option<Duration>) {
        self.running.remove(&source_id);
        // Collect the results of finished tasks, so the set does not grow.
        while self.tasks.try_join_next().is_some() {}
        if let Some(manual) = self.again.remove(&source_id) {
            self.start(source_id, manual);
        } else if let Some(next) = next {
            self.due.insert(source_id, Instant::now() + next);
        }
    }
}
