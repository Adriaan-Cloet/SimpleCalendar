//! Calendar providers: the traits every remote source implements (`docs/architecture.md` §5).
//!
//! A provider does HTTP and translation, nothing else. It never touches the database: it gets
//! what it needs in a `SourceContext` and returns plain values. The sync engine is the only
//! caller, and does all database work (§7).

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::model::{Access, EventStatus, SourceKind};
use crate::secrets::Secret;

pub mod caldav;
pub mod google;
pub(crate) mod http;
pub mod ics;

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
    /// The name the user gave the source. An ICS feed's one calendar carries it.
    pub display_name: String,
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
    /// The account's main calendar. Listed first in its source.
    pub is_primary: bool,
    /// Popup reminders for events that use the calendar's defaults, in minutes before the start.
    pub default_reminders: Vec<u32>,
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
    /// The server's version tag, sent back as `If-Match` when the event is changed.
    pub etag: Option<String>,
    /// The calendar this copy is on organises the event (Google: `organizer.self`). Together
    /// with `guests_can_modify` it decides whether the event can be changed (§4).
    pub is_organizer: bool,
    pub guests_can_modify: bool,
    pub reminders: Reminders,
}

/// An event's reminders (§7.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminders {
    /// The calendar's default reminders apply, and `overrides` is empty.
    pub use_default: bool,
    pub overrides: Vec<Reminder>,
}

impl Default for Reminders {
    /// The calendar's defaults, which is what a provider without reminders means.
    fn default() -> Reminders {
        Reminders {
            use_default: true,
            overrides: Vec::new(),
        }
    }
}

/// The longest reminder Google allows, and `event_reminders` accepts: four weeks.
pub const MAX_REMINDER_MINUTES: u32 = 40320;

// Rust note: `derive(Ord)` compares the fields in the order they are declared, so a sorted
// list has every popup reminder before the email ones, each earliest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Reminder {
    pub method: ReminderMethod,
    /// 0 to 40320 (four weeks).
    pub minutes_before: u32,
}

/// How a reminder reaches the user. Only `Popup` produces a notification; `Email` is kept so it
/// survives an edit (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderMethod {
    Popup,
    Email,
}

impl ReminderMethod {
    /// The value in `event_reminders.method`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReminderMethod::Popup => "popup",
            ReminderMethod::Email => "email",
        }
    }

    /// Reads an `event_reminders.method` value. The column's `CHECK` allows no others.
    pub fn parse(value: &str) -> Option<ReminderMethod> {
        [ReminderMethod::Popup, ReminderMethod::Email]
            .into_iter()
            .find(|method| method.as_str() == value)
    }
}

/// An event as the app wants it on the server: the values it owns. The outbox stores two of
/// these per edit, before and after (§7.3), and a provider writes from them. Times are stored
/// values as described in §4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventData {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub all_day: bool,
    pub start_utc: i64,
    /// Exclusive.
    pub end_utc: i64,
    pub start_tz: Option<String>,
    pub end_tz: Option<String>,
    /// A master's `RRULE`, `RDATE` and `EXDATE` lines; `None` for anything else.
    pub recurrence: Option<String>,
    /// The overrides sorted, so equal reminders compare equal.
    pub reminders: Reminders,
}

/// A part of an event that an edit can change. Google sends a changed part, and only that
/// (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Title,
    Description,
    Location,
    /// Start, end, their zones and whether it is all-day: they only make sense together.
    Times,
    Recurrence,
    Reminders,
}

/// The parts that changed. A `BTreeSet`, so it lists them in a fixed order.
pub type FieldSet = BTreeSet<Field>;

impl EventData {
    /// The parts in which `self` differs from `base`.
    pub fn changed_from(&self, base: &EventData) -> FieldSet {
        Field::ALL
            .into_iter()
            .filter(|&field| !self.same(base, field))
            .collect()
    }

    /// Whether `other` has the same value as `self` in `field`.
    pub fn same(&self, other: &EventData, field: Field) -> bool {
        match field {
            Field::Title => self.title == other.title,
            Field::Description => self.description == other.description,
            Field::Location => self.location == other.location,
            Field::Times => {
                (
                    self.all_day,
                    self.start_utc,
                    self.end_utc,
                    &self.start_tz,
                    &self.end_tz,
                ) == (
                    other.all_day,
                    other.start_utc,
                    other.end_utc,
                    &other.start_tz,
                    &other.end_tz,
                )
            }
            Field::Recurrence => self.recurrence == other.recurrence,
            Field::Reminders => self.reminders == other.reminders,
        }
    }
}

impl Field {
    pub const ALL: [Field; 6] = [
        Field::Title,
        Field::Description,
        Field::Location,
        Field::Times,
        Field::Recurrence,
        Field::Reminders,
    ];
}

impl From<&RemoteEvent> for EventData {
    /// The values of a remote event that the app owns, to compare with what it sent.
    fn from(event: &RemoteEvent) -> EventData {
        let mut reminders = event.reminders.clone();
        reminders.overrides.sort();
        EventData {
            title: event.title.clone(),
            description: event.description.clone(),
            location: event.location.clone(),
            all_day: event.all_day,
            start_utc: event.start_utc,
            end_utc: event.end_utc,
            start_tz: event.start_tz.clone(),
            end_tz: event.end_tz.clone(),
            recurrence: match &event.series {
                SeriesRole::Master { recurrence } => Some(recurrence.clone()),
                SeriesRole::Single | SeriesRole::Exception { .. } => None,
            },
            reminders,
        }
    }
}

/// An edit, as the engine hands it to a provider (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventUpdate {
    /// The event, or for one occurrence of a series the provider's id of that occurrence.
    pub remote_id: String,
    /// The whole state the user wants. CalDAV will write all of it.
    pub desired: EventData,
    /// What differs from the state the edit was based on. Google sends only this.
    pub changed: FieldSet,
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
    /// Problems with the data the user should know about, such as an ICS time zone that could
    /// not be resolved (§7.7). The warnings of a full pull replace the calendar's earlier ones;
    /// those of an incremental pull are added to them.
    pub warnings: Vec<String>,
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
    /// No connection could be made: no network, the name does not resolve, or nothing listens.
    /// The status indicator shows this as offline (§7.1). Retry soon.
    #[error("no connection to the server; trying again soon")]
    Offline,
    /// Network trouble once connected, HTTP 5xx, rate limit. Retry later.
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

impl ProviderError {
    /// A request that got no answer: `Offline` if no connection was made, else `Transient`.
    ///
    /// No route, a refused connection and a name that does not resolve are all offline. A
    /// failed TLS handshake is a connect error too, but the server was reached: rustls reports
    /// it as an `io::Error` of kind `Other` or `InvalidData`, which those failures never are.
    //
    // Rust note: `source()` walks the chain of causes behind an error, and `downcast_ref`
    // checks whether a `dyn Error` is of one concrete type, giving `Some` if it is.
    pub(crate) fn unanswered(e: &reqwest::Error) -> ProviderError {
        let mut tls = false;
        let mut cause = std::error::Error::source(e);
        while let Some(inner) = cause {
            if let Some(io) = inner.downcast_ref::<std::io::Error>() {
                tls |= matches!(
                    io.kind(),
                    std::io::ErrorKind::Other | std::io::ErrorKind::InvalidData
                );
            }
            cause = inner.source();
        }
        if e.is_connect() && !tls {
            ProviderError::Offline
        } else {
            ProviderError::Transient { retry_after: None }
        }
    }
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

/// A provider that can also write: Google, from v0.2 (§5, §7.5).
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

    /// Creates `event` under `remote_id`, which the app generated (§7.3). If the id exists
    /// already, an earlier attempt got through and only its answer was lost: that is a
    /// success, and the existing event is returned.
    async fn create_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
        event: &EventData,
    ) -> Result<RemoteEvent, ProviderError>;

    /// Writes `update.changed`. `if_match` is the ETag the edit was based on; a mismatch is
    /// `ProviderError::Conflict`. Without one the write is unconditional, which only an
    /// occurrence of a series that was never changed before needs.
    async fn update_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        update: &EventUpdate,
        if_match: Option<&Etag>,
    ) -> Result<RemoteEvent, ProviderError>;

    /// `if_match` is the ETag the delete was based on. A mismatch is `ProviderError::Conflict`.
    /// An event that is already gone is a success.
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
