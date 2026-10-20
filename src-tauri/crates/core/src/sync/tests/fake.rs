//! A scripted provider for the engine tests.
//!
//! Each `pull` returns the next scripted result for its source. With the script empty it
//! reports "nothing changed". Every call is recorded and sent to the test, which can wait for
//! it. A gated source holds its pulls until the test releases them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use tokio::sync::{Semaphore, mpsc};
use tokio::time::Instant;

use crate::db::Db;
use crate::model::{Access, SourceKind};
use crate::provider::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, Next, PageToken, ProviderError,
    RemoteCalendar, SourceContext, SyncCursor,
};
use crate::secrets::Secret;

/// One recorded `pull`.
#[derive(Debug, Clone)]
pub struct Pull {
    pub source_id: String,
    pub calendar: String,
    pub cursor: Option<String>,
    pub page: Option<String>,
    pub display_name: String,
    pub secret: Option<Secret>,
    pub at: Instant,
}

pub struct Fake {
    calendars: Mutex<Vec<RemoteCalendar>>,
    list_error: Mutex<Option<ProviderError>>,
    script: Mutex<HashMap<String, VecDeque<Result<ChangeSet, ProviderError>>>>,
    calls: mpsc::UnboundedSender<Pull>,
    gated: Mutex<HashSet<String>>,
    release: Semaphore,
    /// When set, every pull first checks the database invariants and records violations.
    watch: Mutex<Option<Db>>,
    violations: Mutex<Vec<String>>,
}

impl Fake {
    /// The fake with one read-only calendar, `cal-1`, and the receiver of its recorded pulls.
    pub fn new() -> (Fake, mpsc::UnboundedReceiver<Pull>) {
        let (calls, rx) = mpsc::unbounded_channel();
        let fake = Fake {
            calendars: Mutex::new(vec![calendar("cal-1", "Calendar")]),
            list_error: Mutex::new(None),
            script: Mutex::new(HashMap::new()),
            calls,
            gated: Mutex::new(HashSet::new()),
            release: Semaphore::new(0),
            watch: Mutex::new(None),
            violations: Mutex::new(Vec::new()),
        };
        (fake, rx)
    }

    pub fn set_calendars(&self, calendars: Vec<RemoteCalendar>) {
        *lock(&self.calendars) = calendars;
    }

    pub fn fail_list(&self, error: Option<ProviderError>) {
        *lock(&self.list_error) = error;
    }

    /// Queues the results of the next pulls of `source_id`, in order.
    pub fn script(&self, source_id: &str, results: Vec<Result<ChangeSet, ProviderError>>) {
        lock(&self.script)
            .entry(source_id.to_owned())
            .or_default()
            .extend(results);
    }

    /// From now on, pulls of `source_id` wait for `release`.
    pub fn gate(&self, source_id: &str) {
        lock(&self.gated).insert(source_id.to_owned());
    }

    /// Lets `n` held pulls continue.
    pub fn release(&self, n: usize) {
        self.release.add_permits(n);
    }

    /// Checks the invariants before every pull, so after every committed page.
    pub fn watch(&self, db: Db) {
        *lock(&self.watch) = Some(db);
    }

    pub fn violations(&self) -> Vec<String> {
        lock(&self.violations).clone()
    }
}

#[async_trait]
impl CalendarProvider for Fake {
    fn kind(&self) -> SourceKind {
        SourceKind::Ics
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: false,
            incremental_sync: true,
            reminders: false,
        }
    }

    async fn list_calendars(
        &self,
        _cx: &SourceContext,
    ) -> Result<Vec<RemoteCalendar>, ProviderError> {
        if let Some(error) = lock(&self.list_error).clone() {
            return Err(error);
        }
        Ok(lock(&self.calendars).clone())
    }

    async fn pull(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        cursor: Option<&SyncCursor>,
        page: Option<&PageToken>,
    ) -> Result<ChangeSet, ProviderError> {
        let watched = lock(&self.watch).clone();
        if let Some(db) = watched {
            let found = super::invariant_violations(&db).await;
            lock(&self.violations).extend(found);
        }
        let _ = self.calls.send(Pull {
            source_id: cx.source_id.clone(),
            calendar: calendar.remote_id.clone(),
            cursor: cursor.map(|c| c.0.clone()),
            page: page.map(|p| p.0.clone()),
            display_name: cx.display_name.clone(),
            secret: cx.secret.clone(),
            at: Instant::now(),
        });
        let gated = lock(&self.gated).contains(&cx.source_id);
        if gated {
            self.release
                .acquire()
                .await
                .expect("semaphore open")
                .forget();
        }
        let next = lock(&self.script)
            .get_mut(&cx.source_id)
            .and_then(VecDeque::pop_front);
        next.unwrap_or_else(|| {
            Ok(ChangeSet {
                upserts: Vec::new(),
                deletions: Vec::new(),
                full: false,
                warnings: Vec::new(),
                next: Next::Done(SyncCursor(
                    cursor.map_or("unchanged".to_owned(), |c| c.0.clone()),
                )),
            })
        })
    }
}

pub fn calendar(remote_id: &str, name: &str) -> RemoteCalendar {
    RemoteCalendar {
        remote_id: remote_id.to_owned(),
        name: name.to_owned(),
        color: Some("#336699".to_owned()),
        time_zone: Some("Europe/Brussels".to_owned()),
        access: Access::Read,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
