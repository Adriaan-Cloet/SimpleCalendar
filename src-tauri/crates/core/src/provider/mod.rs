//! Calendar providers: the traits every remote source implements (`docs/architecture.md` §5).
//!
//! A provider does HTTP and translation, nothing else. It never touches the database: it gets
//! what it needs in a `SourceContext` and returns plain values. The sync engine is the only
//! caller, and does all database work (§7).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use crate::model::{Access, EventStatus, SourceKind};
use crate::secrets::Secret;

/// What the UI may offer for calendars of this provider. Plain data, serialisable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// Events can be created, edited and deleted.
    pub writable: bool,
    /// `pull` can return only what changed since a cursor.
    pub incremental_sync: bool,
    /// Reminders round-trip to the server.
    pub reminders: bool,
}

/// What a provider gets for one source, built by the engine at the start of every cycle.
#[derive(Debug, Clone)]
pub struct SourceContext {
    pub source_id: String,
    /// The source's secret from the keychain, such as the feed URL of an ICS source. `None` if
    /// it has none.
    pub secret: Option<Secret>,
}

/// A calendar as the provider reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCalendar {
    pub remote_id: String,
    pub name: String,
    /// `#rrggbb`.
    pub color: Option<String>,
    /// IANA name.
    pub time_zone: Option<String>,
    pub access: Access,
}

/// Which calendar `pull` should read: the provider's own id, plus what the provider reported
/// about it before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarRef {
    pub remote_id: String,
    /// IANA name. Google gives events without their own zone the zone of their calendar.
    pub time_zone: Option<String>,
}

/// Where an event stands in a recurring series (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesRole {
    /// Not part of a series.
    Single,
    /// The master of a series.
    Master {
        /// `RRULE`, `RDATE` and `EXDATE` lines, one per line.
        recurrence: String,
    },
    /// An edited or cancelled occurrence of a series.
    Exception {
        /// The master's `remote_id`. Known as soon as the exception arrives, whatever the order.
        master_remote_id: String,
        /// The stored start of the occurrence this one replaces.
        original_start_utc: i64,
    },
}

/// One event as the provider reports it. The fields map to the `events` columns of the same
/// name; times are stored values as described in §4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEvent {
    pub remote_id: String,
    pub ical_uid: Option<String>,
    pub status: EventStatus,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub color: Option<String>,
    pub all_day: bool,
    pub start_utc: i64,
    /// Exclusive.
    pub end_utc: i64,
    /// IANA name, an offset such as `+05:30`, or `None` for floating and all-day times.
    pub start_tz: Option<String>,
    pub end_tz: Option<String>,
    pub series: SeriesRole,
    pub remote_updated_at: Option<i64>,
}

/// A position in the provider's change history. Opaque: the engine stores it and hands it
/// back, nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCursor(pub String);

/// The next page of one pull. Opaque, like the cursor, and never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageToken(pub String);

/// A server's version tag of one event, sent as `If-Match`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Etag(pub String);

/// One page of changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    /// Events to insert or update. In any order: an exception may come before its master.
    pub upserts: Vec<RemoteEvent>,
    /// `remote_id`s of events that are gone.
    pub deletions: Vec<String>,
    /// This pull describes the whole calendar; anything not seen must be deleted locally.
    pub full: bool,
    pub next: Next,
}

/// What follows a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// More pages follow. Nothing is saved as a cursor yet.
    Page(PageToken),
    /// Last page. The engine stores this cursor together with the page's changes.
    Done(SyncCursor),
}

/// Why a provider call failed, classified by the provider. The engine only reacts to these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// Network trouble, HTTP 5xx, rate limit. Retry later.
    #[error("the server could not be reached; trying again later")]
    Transient { retry_after: Option<Duration> },
    /// Credentials rejected. The user has to sign in again.
    #[error("the server rejected the credentials")]
    Auth,
    /// The remote copy changed since `if_match`.
    #[error("the event was changed elsewhere")]
    Conflict,
    /// The cursor is no longer valid. Start over with a full pull.
    #[error("the sync position is no longer valid")]
    CursorInvalid,
    #[error("not found")]
    NotFound,
    /// Will never succeed (bad request, forbidden). Do not retry.
    #[error("{0}")]
    Permanent(String),
}

/// A source of calendars that can be read: ICS feeds, Google, CalDAV.
//
// Rust note: `#[async_trait]` rewrites every `async fn` below into a plain `fn` that returns a
// boxed future, `Pin<Box<dyn Future<Output = ...> + Send>>`. Without it the trait could not be
// used as `dyn CalendarProvider`: each implementation's `async fn` returns its own hidden future
// type, and a trait object needs one fixed signature. The cost is one small allocation per
// call, nothing next to an HTTP request. `Send + Sync` lets the engine share a provider
// between tasks on tokio's thread pool.
#[async_trait]
pub trait CalendarProvider: Send + Sync {
    fn kind(&self) -> SourceKind;
    fn capabilities(&self) -> Capabilities;

    /// The calendars this source currently exposes.
    async fn list_calendars(
        &self,
        cx: &SourceContext,
    ) -> Result<Vec<RemoteCalendar>, ProviderError>;

    /// One page of changes. `cursor = None` asks for everything.
    async fn pull(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        cursor: Option<&SyncCursor>,
        page: Option<&PageToken>,
    ) -> Result<ChangeSet, ProviderError>;

    /// `Some` if this provider can write. The default is read-only.
    ///
    /// `capabilities().writable` must equal `as_writable().is_some()`.
    fn as_writable(&self) -> Option<&dyn WritableProvider> {
        None
    }
}

/// A provider that can also write. Nothing implements it yet: Google write (v0.2) is the
/// first. `create_event` and `update_event` join it then, together with the draft and update
/// types they take (§5).
//
// Rust note: `WritableProvider: CalendarProvider` is a supertrait. Everything writable is also
// readable, and a `&dyn WritableProvider` can call the read methods too.
#[async_trait]
pub trait WritableProvider: CalendarProvider {
    async fn get_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
    ) -> Result<Option<RemoteEvent>, ProviderError>;

    /// `if_match` is the ETag the delete was based on. A mismatch is `ProviderError::Conflict`.
    async fn delete_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
        if_match: Option<&Etag>,
    ) -> Result<(), ProviderError>;
}

/// The provider for each kind of source. A kind without one, such as `local`, is never synced.
//
// Rust note: `Arc<dyn CalendarProvider>` is a shared pointer to "some type that implements the
// trait". The map holds ICS and Google providers side by side and calls them the same way.
#[derive(Clone, Default)]
pub struct Providers {
    by_kind: HashMap<SourceKind, Arc<dyn CalendarProvider>>,
}

impl Providers {
    pub fn new() -> Providers {
        Providers::default()
    }

    /// Adds `provider` for the kind it reports, replacing an earlier one.
    pub fn with(mut self, provider: Arc<dyn CalendarProvider>) -> Providers {
        self.by_kind.insert(provider.kind(), provider);
        self
    }

    pub fn get(&self, kind: SourceKind) -> Option<Arc<dyn CalendarProvider>> {
        self.by_kind.get(&kind).cloned()
    }
}

impl fmt::Debug for Providers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.by_kind.keys()).finish()
    }
}
