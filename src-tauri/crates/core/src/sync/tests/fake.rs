//! A scripted provider for the engine tests.
//!
//! Each `pull` returns the next scripted result for its source. With the script empty it
//! reports "nothing changed". Every call is recorded and sent to the test, which can wait for
//! it. A gated source holds its pulls until the test releases them.
//!
//! Made writable, it records every write and answers with the next scripted answer; with none
//! scripted, a create or update succeeds and echoes the event with a new ETag (`"w1"`, `"w2"`,
//! ...), a delete succeeds and a get finds nothing. `order` lists pulls and writes as they
//! happened.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use tokio::sync::{Semaphore, mpsc};
use tokio::time::Instant;

use crate::db::Db;
use crate::model::EventStatus;
use crate::model::{Access, SourceKind};
use crate::provider::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, Etag, EventData, EventUpdate, Next,
    PageToken, ProviderError, RemoteCalendar, RemoteEvent, SeriesRole, SourceContext, SyncCursor,
    WritableProvider,
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

/// One recorded write, or the get that checks a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Create {
        remote_id: String,
        event: EventData,
    },
    Update {
        update: EventUpdate,
        if_match: Option<String>,
    },
    Delete {
        remote_id: String,
        if_match: Option<String>,
    },
    Get {
        remote_id: String,
    },
}

/// What a write answers: `Ok(None)` for a delete, or a get that finds nothing.
pub type Answer = Result<Option<RemoteEvent>, ProviderError>;

pub struct Fake {
    writable: Mutex<bool>,
    answers: Mutex<VecDeque<Answer>>,
    writes: Mutex<Vec<Write>>,
    order: Mutex<Vec<String>>,
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
            writable: Mutex::new(false),
            answers: Mutex::new(VecDeque::new()),
            writes: Mutex::new(Vec::new()),
            order: Mutex::new(Vec::new()),
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

    pub fn set_writable(&self, writable: bool) {
        *lock(&self.writable) = writable;
    }

    /// Queues the answers of the next writes and gets, in order.
    pub fn answer(&self, answers: Vec<Answer>) {
        lock(&self.answers).extend(answers);
    }

    pub fn writes(&self) -> Vec<Write> {
        lock(&self.writes).clone()
    }

    /// `pull <calendar>` and `create|update|delete|get <id>`, as they happened.
    pub fn order(&self) -> Vec<String> {
        lock(&self.order).clone()
    }

    /// Records `write` and returns the next scripted answer, if any.
    fn record(&self, write: Write) -> Option<Answer> {
        let (name, id) = match &write {
            Write::Create { remote_id, .. } => ("create", remote_id),
            Write::Update { update, .. } => ("update", &update.remote_id),
            Write::Delete { remote_id, .. } => ("delete", remote_id),
            Write::Get { remote_id } => ("get", remote_id),
        };
        lock(&self.order).push(format!("{name} {id}"));
        lock(&self.writes).push(write);
        lock(&self.answers).pop_front()
    }

    /// The event the server keeps after a successful write of `event`.
    fn echo(&self, remote_id: &str, event: &EventData) -> RemoteEvent {
        let n = lock(&self.writes).len();
        remote(remote_id, event, &format!("\"w{n}\""))
    }
}

/// A server copy of `event` under `remote_id` with `etag`.
pub fn remote(remote_id: &str, event: &EventData, etag: &str) -> RemoteEvent {
    RemoteEvent {
        remote_id: remote_id.to_owned(),
        ical_uid: None,
        status: EventStatus::Confirmed,
        title: event.title.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        color: None,
        all_day: event.all_day,
        start_utc: event.start_utc,
        end_utc: event.end_utc,
        start_tz: event.start_tz.clone(),
        end_tz: event.end_tz.clone(),
        series: match &event.recurrence {
            Some(recurrence) => SeriesRole::Master {
                recurrence: recurrence.clone(),
            },
            None => SeriesRole::Single,
        },
        remote_updated_at: None,
        etag: Some(etag.to_owned()),
        is_organizer: true,
        guests_can_modify: false,
        reminders: event.reminders.clone(),
    }
}

#[async_trait]
impl CalendarProvider for Fake {
    fn kind(&self) -> SourceKind {
        SourceKind::Ics
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: *lock(&self.writable),
            incremental_sync: true,
            reminders: false,
        }
    }

    fn as_writable(&self) -> Option<&dyn WritableProvider> {
        if *lock(&self.writable) {
            Some(self)
        } else {
            None
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
        lock(&self.order).push(format!("pull {}", calendar.remote_id));
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

#[async_trait]
impl WritableProvider for Fake {
    async fn get_event(
        &self,
        _cx: &SourceContext,
        _calendar: &CalendarRef,
        remote_id: &str,
    ) -> Result<Option<RemoteEvent>, ProviderError> {
        let answer = self.record(Write::Get {
            remote_id: remote_id.to_owned(),
        });
        answer.unwrap_or(Ok(None))
    }

    async fn create_event(
        &self,
        _cx: &SourceContext,
        _calendar: &CalendarRef,
        remote_id: &str,
        event: &EventData,
    ) -> Result<RemoteEvent, ProviderError> {
        let answer = self.record(Write::Create {
            remote_id: remote_id.to_owned(),
            event: event.clone(),
        });
        match answer {
            Some(answer) => answer.map(|e| e.expect("a create answers with an event")),
            None => Ok(self.echo(remote_id, event)),
        }
    }

    async fn update_event(
        &self,
        _cx: &SourceContext,
        _calendar: &CalendarRef,
        update: &EventUpdate,
        if_match: Option<&Etag>,
    ) -> Result<RemoteEvent, ProviderError> {
        let answer = self.record(Write::Update {
            update: update.clone(),
            if_match: if_match.map(|e| e.0.clone()),
        });
        match answer {
            Some(answer) => answer.map(|e| e.expect("an update answers with an event")),
            None => Ok(self.echo(&update.remote_id, &update.desired)),
        }
    }

    async fn delete_event(
        &self,
        _cx: &SourceContext,
        _calendar: &CalendarRef,
        remote_id: &str,
        if_match: Option<&Etag>,
    ) -> Result<(), ProviderError> {
        let answer = self.record(Write::Delete {
            remote_id: remote_id.to_owned(),
            if_match: if_match.map(|e| e.0.clone()),
        });
        answer.unwrap_or(Ok(None)).map(|_| ())
    }
}

pub fn calendar(remote_id: &str, name: &str) -> RemoteCalendar {
    RemoteCalendar {
        remote_id: remote_id.to_owned(),
        name: name.to_owned(),
        color: Some("#336699".to_owned()),
        time_zone: Some("Europe/Brussels".to_owned()),
        access: Access::Read,
        is_primary: false,
        default_reminders: vec![10],
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
