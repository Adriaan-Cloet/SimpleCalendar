//! The sync engine for read-only sources (`docs/architecture.md` §7.1, §7.2).
//!
//! One long-lived task owns a queue of commands. It runs at most one cycle per source at a
//! time; a request that arrives during a cycle sets a "run again" flag instead of starting a
//! second one. Different sources sync in parallel. The engine is the only component that talks
//! to providers and the only one that applies remote changes to the database.

use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::CoreEvent;
use crate::db::Db;
use crate::model::SourceKind;
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

/// How often the timer syncs a source of this kind, unless `sources.sync_interval_s` overrides
/// it. `None` for kinds that are never synced.
pub fn default_interval(kind: SourceKind) -> Option<Duration> {
    let _ = kind;
    todo!()
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

/// Requests to the engine task, including the ones it sends itself.
#[derive(Debug)]
#[expect(
    dead_code,
    reason = "read by the engine loop, which is not written yet"
)]
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
pub struct SyncEngine {
    _db: Db,
    _secrets: Secrets,
    _providers: Providers,
    _events: mpsc::UnboundedSender<CoreEvent>,
    _rx: mpsc::UnboundedReceiver<Msg>,
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
            _db: db,
            _secrets: secrets,
            _providers: providers,
            _events: events,
            _rx: rx,
        };
        (engine, SyncHandle { tx })
    }

    /// Runs until `shutdown` is called or every handle is dropped.
    pub async fn run(self) {
        todo!()
    }
}
