//! `CalendarService`: the one entry point into the core (`docs/architecture.md` §2, §10).
//!
//! Tauri commands are thin wrappers around these methods. Reads go straight to SQLite, sync
//! requests go to the engine, and secrets go to the keychain only.

mod events;
mod google;
mod series;
#[cfg(test)]
mod tests;
mod text;

pub use events::{
    EventDetail, EventDraft, EventReminder, OccurrenceRef, Repeat, RepeatFrequency, Scope,
};
pub use google::GoogleClientInfo;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, NaiveTime};
use chrono_tz::Tz;
use reqwest::Url;
use rusqlite::{Connection, OptionalExtension, params};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::CoreEvent;
use crate::db::{Db, DbError, occurrences};
use crate::google::GoogleAuth;
use crate::model::{LOCAL_SOURCE_ID, Occurrence, SourceKind};
use crate::provider::ics::feed_url;
use crate::recurrence::resolve_local;
use crate::secrets::{Secret, SecretError, Secrets, SecretsUnavailable};
use crate::sync::{SyncHandle, SyncState};

/// What `get_status` reports. Sources needing sign-in and open conflicts join this in later
/// versions (§10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    /// `None` while the keychain is usable. Otherwise the structured `secrets_unavailable`
    /// error, with the message to show.
    pub secrets_unavailable: Option<SecretsUnavailable>,
}

/// A source as the UI lists it. Holds nothing secret: an ICS source shows its host, never its
/// URL (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceInfo {
    pub id: String,
    pub kind: SourceKind,
    pub display_name: String,
    /// The ICS host or the Google account.
    pub account_label: Option<String>,
    /// The outcome of the last sync: `ok`, `error` or `auth_required`. While a sync runs, the
    /// `sync:status` events say so.
    pub state: SyncState,
    pub last_error: Option<String>,
    /// When the last sync of any of its calendars finished, in epoch seconds. `None` before the
    /// first successful sync.
    pub last_synced_at: Option<i64>,
    /// The warnings of all its calendars, sorted, each once (§7.7).
    pub warnings: Vec<String>,
}

/// A calendar as the sidebar and the event popover show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CalendarInfo {
    pub id: i64,
    pub source_id: String,
    pub source_kind: SourceKind,
    /// The source's name: a Google account's address, a feed's name.
    pub source_name: String,
    pub name: String,
    /// The user's colour, else the provider's. `None` if neither set one.
    pub color: Option<String>,
    /// Whether events can be created and changed in it: write access (§4).
    pub editable: bool,
    pub visible: bool,
}

/// The user's changes to a calendar, as `update_calendar` receives them. A field that is left
/// out stays as it is. `visible` and `color_override` are local preferences and never synced
/// (§4); `name` can only change on a local calendar, because a sync rewrites remote names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CalendarUpdate {
    pub name: Option<String>,
    pub visible: Option<bool>,
    /// `Some(Some("#rrggbb"))` sets the colour, `Some(None)` (JSON `null`) goes back to the
    /// provider's.
    #[serde(default, deserialize_with = "present")]
    pub color_override: Option<Option<String>>,
}

// Rust note: serde reads both a missing field and `null` as `None` for an `Option`. To tell
// them apart, `#[serde(default)]` handles the missing field (`None`), and this function, which
// serde only calls when the field is there, wraps whatever it holds (`null` too) in `Some`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::deserialize(d).map(Some)
}

const SOURCE_GONE: &str = "This subscription no longer exists.";
const CALENDAR_GONE: &str = "This calendar no longer exists.";
const NOT_LOCAL_RENAME: &str = "Only calendars on this computer can be renamed.";
const NOT_LOCAL_REMOVE: &str =
    "Only calendars on this computer can be deleted. Remove the account or subscription instead.";

/// The longest range `list_occurrences` accepts. A month view needs six weeks; this leaves room
/// for a year view or an agenda without letting a typo expand a series for centuries.
const MAX_RANGE_DAYS: i64 = 400;

/// Why a service call failed. Serialises as `{"kind": ..., "message": ...}` for the frontend;
/// `secrets_unavailable` also carries its `reason` (§8).
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    SecretsUnavailable(SecretsUnavailable),
    /// The user's input cannot be used. The message says what to change.
    #[error("{0}")]
    InvalidInput(String),
    /// The thing the call names is gone, for example removed by a sync. The message says what.
    #[error("{0}")]
    NotFound(&'static str),
    #[error(transparent)]
    Secrets(SecretError),
    #[error(transparent)]
    Db(#[from] DbError),
    /// No Google client was compiled in and the user has not entered their own (§8).
    #[error(
        "Google sign-in needs the client id of a Google Cloud project. Enter yours to continue."
    )]
    GoogleClientMissing,
    /// The Google sign-in did not complete. The message says why and what to do.
    #[error("{0}")]
    SignIn(String),
    /// The user cancelled the sign-in, or started another one.
    #[error("Sign-in was cancelled.")]
    Cancelled,
    /// The event or calendar cannot be changed: the calendar is read-only, or the event is
    /// organised by someone else who does not let guests change it (§4).
    #[error("This event can’t be changed: its calendar is read-only or someone else organises it.")]
    NotEditable,
}

impl From<SecretError> for ServiceError {
    fn from(e: SecretError) -> ServiceError {
        match e {
            SecretError::Unavailable(unavailable) => ServiceError::SecretsUnavailable(unavailable),
            other => ServiceError::Secrets(other),
        }
    }
}

impl From<SecretsUnavailable> for ServiceError {
    fn from(e: SecretsUnavailable) -> ServiceError {
        ServiceError::SecretsUnavailable(e)
    }
}

impl Serialize for ServiceError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let ServiceError::SecretsUnavailable(unavailable) = self {
            return unavailable.serialize(serializer);
        }
        let kind = match self {
            ServiceError::SecretsUnavailable(_) => unreachable!("handled above"),
            ServiceError::InvalidInput(_) => "invalid_input",
            ServiceError::NotFound(_) => "not_found",
            ServiceError::Secrets(_) => "keychain",
            ServiceError::Db(_) => "database",
            ServiceError::GoogleClientMissing => "google_client_missing",
            ServiceError::SignIn(_) => "sign_in",
            ServiceError::Cancelled => "cancelled",
            ServiceError::NotEditable => "not_editable",
        };
        let mut s = serializer.serialize_struct("ServiceError", 2)?;
        s.serialize_field("kind", kind)?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

/// The core, as the shell sees it. Cloning is cheap: every field is a handle.
#[derive(Clone)]
pub struct CalendarService {
    db: Db,
    secrets: Secrets,
    sync: SyncHandle,
    events: mpsc::UnboundedSender<CoreEvent>,
    google: Arc<GoogleAuth>,
    /// Cancels the Google sign-in that is waiting for the browser, if one is.
    sign_in: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl CalendarService {
    /// `events` is the same channel the sync engine reports on; `google` is shared with the
    /// Google provider.
    pub fn new(
        db: Db,
        secrets: Secrets,
        sync: SyncHandle,
        events: mpsc::UnboundedSender<CoreEvent>,
        google: Arc<GoogleAuth>,
    ) -> CalendarService {
        CalendarService {
            db,
            secrets,
            sync,
            events,
            google,
            sign_in: Arc::new(Mutex::new(None)),
        }
    }

    /// The app's status. If the keychain was unavailable, it is probed again first, so a "Try
    /// again" button only calls this.
    pub async fn status(&self) -> Status {
        Status {
            secrets_unavailable: self.secrets.status().await,
        }
    }

    /// Every occurrence on a visible calendar from the start of date `from` to the start of
    /// date `to` in the zone `tz`, sorted by start. Dates are `YYYY-MM-DD`, `tz` an IANA name.
    ///
    /// The frontend passes dates, not instants, so it never needs to know when a day starts in
    /// its zone: a week that contains a DST change is 167 or 169 hours long (§10).
    pub async fn list_occurrences(
        &self,
        from: &str,
        to: &str,
        tz: &str,
    ) -> Result<Vec<Occurrence>, ServiceError> {
        let viewer = parse_zone(tz)?;
        let (from, to) = (parse_date(from)?, parse_date(to)?);
        if !(1..=MAX_RANGE_DAYS).contains(&(to - from).num_days()) {
            return Err(ServiceError::InvalidInput(format!(
                "Choose a range of 1 to {MAX_RANGE_DAYS} days."
            )));
        }
        let midnight = |date: NaiveDate| resolve_local(viewer, date.and_time(NaiveTime::MIN));
        let (from, to) = (midnight(from).timestamp(), midnight(to).timestamp());
        Ok(self
            .db
            .read(move |c| occurrences::list_occurrences(c, from, to, viewer))
            .await?)
    }

    /// Every calendar, local ones included: the oldest source first, in a source the primary
    /// calendar first, then by name.
    pub async fn list_calendars(&self) -> Result<Vec<CalendarInfo>, ServiceError> {
        Ok(self
            .db
            .read(|c| {
                let mut stmt = c.prepare(&format!(
                    "{CALENDAR_INFO}
                     ORDER BY s.created_at, s.id, c.is_primary DESC, c.name COLLATE NOCASE, c.id"
                ))?;
                stmt.query_map([], calendar_info)?.collect()
            })
            .await?)
    }

    /// Changes a calendar's visibility, colour or, for a local calendar, its name, and returns
    /// it as `list_calendars` would.
    ///
    /// A sync never touches visibility and colour, so the choice survives it. `calendar:changed`
    /// names the calendar, so every view reads its occurrences again.
    pub async fn update_calendar(
        &self,
        calendar_id: i64,
        update: CalendarUpdate,
    ) -> Result<CalendarInfo, ServiceError> {
        let color_override = match update.color_override {
            Some(Some(color)) => Some(Some(parse_color(&color)?)),
            other => other,
        };
        let name = update.name.as_deref().map(parse_name).transpose()?;
        // Rust note: the closure must return a `rusqlite::Result`, so a refusal decided inside
        // the transaction travels as the inner `Result` of `Ok(Err(..))`. Returning early drops
        // `tx`, which rolls it back. After `.await`, the first `?` handles the database error,
        // the second the refusal.
        let calendar = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let Some(kind) = calendar_kind(&tx, calendar_id)? else {
                    return Ok(Err(ServiceError::NotFound(CALENDAR_GONE)));
                };
                if name.is_some() && kind != SourceKind::Local {
                    return Ok(Err(ServiceError::InvalidInput(NOT_LOCAL_RENAME.into())));
                }
                // A left-out `name` or `visible` binds `NULL`, so `coalesce` keeps the column.
                // The colour needs a separate flag (?4), because `NULL` is also a value to store
                // there.
                tx.execute(
                    "UPDATE calendars
                     SET name = coalesce(?2, name),
                         visible = coalesce(?3, visible),
                         color_override = CASE WHEN ?4 THEN ?5 ELSE color_override END
                     WHERE id = ?1",
                    params![
                        calendar_id,
                        name,
                        update.visible,
                        color_override.is_some(),
                        color_override.flatten()
                    ],
                )?;
                let calendar = tx.query_row(
                    &format!("{CALENDAR_INFO} WHERE c.id = ?1"),
                    [calendar_id],
                    calendar_info,
                )?;
                tx.commit()?;
                Ok(Ok(calendar))
            })
            .await??;
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![calendar_id],
        });
        Ok(calendar)
    }

    /// Creates a calendar in the local source and returns it as `list_calendars` would. `name`
    /// is trimmed and must not be empty.
    ///
    /// A local calendar has no provider: it is never synced, and writes to it create no outbox
    /// rows (§5). Its `remote_id` is a fresh UUID, which only keeps `(source_id, remote_id)`
    /// unique.
    pub async fn create_local_calendar(&self, name: &str) -> Result<CalendarInfo, ServiceError> {
        let name = parse_name(name)?;
        let remote_id = uuid::Uuid::new_v4().to_string();
        let calendar = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO calendars (source_id, remote_id, name, access)
                     VALUES (?1, ?2, ?3, 'write')",
                    params![LOCAL_SOURCE_ID, remote_id, name],
                )?;
                let calendar = tx.query_row(
                    &format!("{CALENDAR_INFO} WHERE c.id = ?1"),
                    [tx.last_insert_rowid()],
                    calendar_info,
                )?;
                tx.commit()?;
                Ok(calendar)
            })
            .await?;
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![calendar.id],
        });
        Ok(calendar)
    }

    /// Deletes a local calendar with its events. A remote calendar goes only with its source
    /// (`remove_source`): the next sync would bring it back.
    pub async fn remove_calendar(&self, calendar_id: i64) -> Result<(), ServiceError> {
        self.db
            .write(move |c| {
                let tx = c.transaction()?;
                match calendar_kind(&tx, calendar_id)? {
                    None => return Ok(Err(ServiceError::NotFound(CALENDAR_GONE))),
                    Some(SourceKind::Local) => {}
                    Some(_) => return Ok(Err(ServiceError::InvalidInput(NOT_LOCAL_REMOVE.into()))),
                }
                // Its events go with it, through `ON DELETE CASCADE`.
                tx.execute("DELETE FROM calendars WHERE id = ?1", [calendar_id])?;
                tx.commit()?;
                Ok(Ok(()))
            })
            .await??;
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![calendar_id],
        });
        Ok(())
    }

    /// Syncs one source now, or every remote source for `None`. Returns at once; progress
    /// arrives as `sync:status` events.
    pub fn sync_now(&self, source_id: Option<String>) {
        self.sync.sync_now(source_id);
    }

    /// Subscribes to the feed at `url`. `name` may be empty; the feed's host is used then.
    ///
    /// The URL goes to the keychain only. The database stores the host, for display (§8). The
    /// first sync starts at once; whether the address works shows in its `sync:status`.
    pub async fn add_ics_source(&self, url: &str, name: &str) -> Result<SourceInfo, ServiceError> {
        let url = url.trim();
        let host = feed_host(url).ok_or_else(|| {
            ServiceError::InvalidInput(
                "Enter a feed address that starts with https://, http:// or webcal://.".into(),
            )
        })?;
        self.secrets.require().await?;

        let id = uuid::Uuid::new_v4().to_string();
        let display_name = match name.trim() {
            "" => host.clone(),
            name => name.to_owned(),
        };
        // The secret first: a source row without its secret could never sync.
        self.secrets
            .set(
                &id,
                &Secret::IcsUrl {
                    url: url.to_owned(),
                },
            )
            .await?;
        let row = (id.clone(), display_name, host);
        let inserted = self
            .db
            .write(move |c| {
                let (id, display_name, host) = row;
                c.execute(
                    "INSERT INTO sources (id, kind, display_name, account_label, created_at)
                     VALUES (?1, 'ics', ?2, ?3, ?4)",
                    params![id, display_name, host, now()],
                )?;
                Ok(())
            })
            .await;
        if let Err(e) = inserted {
            // Leave nothing behind in the keychain for a source that does not exist.
            if let Err(cleanup) = self.secrets.delete(&id).await {
                log::warn!("source {id}: cannot remove its secret after a failed add: {cleanup}");
            }
            return Err(e.into());
        }

        self.sync.sync_now(Some(id.clone()));
        let source = self.db.read(move |c| source_info(c, &id)).await?;
        source.ok_or(ServiceError::NotFound(SOURCE_GONE))
    }

    /// Every remote source, oldest first. The built-in local source is not listed.
    pub async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        Ok(self
            .db
            .read(|c| {
                let mut stmt = c.prepare(
                    "SELECT id FROM sources WHERE kind <> 'local' ORDER BY created_at, id",
                )?;
                let ids = stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut sources = Vec::with_capacity(ids.len());
                for id in ids {
                    sources.extend(source_info(c, &id)?);
                }
                Ok(sources)
            })
            .await?)
    }

    /// Removes a source with its calendars and events, and its secret from the keychain. A
    /// Google account's access is not revoked at Google; the user can do that in their Google
    /// account settings.
    ///
    /// The secret goes first. If the keychain cannot be reached, the source stays and the error
    /// says why, so a feed URL is never left behind without a source that shows it exists.
    pub async fn remove_source(&self, source_id: &str) -> Result<(), ServiceError> {
        let id = source_id.to_owned();
        let exists = self
            .db
            .read(move |c| {
                c.query_row(
                    "SELECT 1 FROM sources WHERE id = ?1 AND kind <> 'local'",
                    [id],
                    |_| Ok(()),
                )
                .optional()
            })
            .await?;
        if exists.is_none() {
            return Err(ServiceError::NotFound(SOURCE_GONE));
        }

        self.secrets.delete(source_id).await?;
        self.google.forget(source_id).await;
        let id = source_id.to_owned();
        let calendar_ids = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let calendar_ids = tx
                    .prepare("SELECT id FROM calendars WHERE source_id = ?1 ORDER BY id")?
                    .query_map([&id], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?;
                // Calendars and their events go with it, through `ON DELETE CASCADE`.
                tx.execute("DELETE FROM sources WHERE id = ?1", [&id])?;
                tx.commit()?;
                Ok(calendar_ids)
            })
            .await?;
        if !calendar_ids.is_empty() {
            // Only fails when the shell stopped listening, which happens when the app exits.
            let _ = self
                .events
                .send(CoreEvent::CalendarChanged { calendar_ids });
        }
        Ok(())
    }
}

/// The host of a feed address, or `None` if it is not an `http`, `https`, `webcal` or
/// `webcals` URL with a host.
fn feed_host(url: &str) -> Option<String> {
    let parsed = Url::parse(&feed_url(url)).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
}

/// An IANA zone name from the frontend.
fn parse_zone(name: &str) -> Result<Tz, ServiceError> {
    name.parse()
        .map_err(|_| ServiceError::InvalidInput(format!("Unknown time zone {name:?}.")))
}

/// A `YYYY-MM-DD` date from the frontend.
fn parse_date(value: &str) -> Result<NaiveDate, ServiceError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ServiceError::InvalidInput(format!("{value:?} is not a YYYY-MM-DD date.")))
}

/// The columns `calendar_info` reads, without `WHERE` or `ORDER BY`.
const CALENDAR_INFO: &str = "SELECT c.id, c.source_id, s.kind, c.name,
        coalesce(c.color_override, c.color), c.access = 'write', c.visible, s.display_name
    FROM calendars c JOIN sources s ON s.id = c.source_id";

/// One row of `CALENDAR_INFO`.
fn calendar_info(r: &rusqlite::Row<'_>) -> rusqlite::Result<CalendarInfo> {
    let kind: String = r.get(2)?;
    Ok(CalendarInfo {
        id: r.get(0)?,
        source_id: r.get(1)?,
        source_kind: parse_kind(2, &kind)?,
        name: r.get(3)?,
        color: r.get(4)?,
        editable: r.get(5)?,
        visible: r.get(6)?,
        source_name: r.get(7)?,
    })
}

/// A calendar name from the frontend, trimmed. An empty one is refused.
fn parse_name(value: &str) -> Result<String, ServiceError> {
    match value.trim() {
        "" => Err(ServiceError::InvalidInput(
            "Enter a name for the calendar.".into(),
        )),
        name => Ok(name.to_owned()),
    }
}

/// The kind of the source a calendar belongs to, or `None` if there is no such calendar.
fn calendar_kind(c: &Connection, calendar_id: i64) -> rusqlite::Result<Option<SourceKind>> {
    c.query_row(
        "SELECT s.kind FROM calendars c JOIN sources s ON s.id = c.source_id WHERE c.id = ?1",
        [calendar_id],
        |r| parse_kind(0, &r.get::<_, String>(0)?),
    )
    .optional()
}

/// A `#rrggbb` colour from the frontend, in lower case as the database stores colours.
fn parse_color(value: &str) -> Result<String, ServiceError> {
    match value.strip_prefix('#') {
        Some(hex) if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(value.to_ascii_lowercase())
        }
        _ => Err(ServiceError::InvalidInput(format!(
            "{value:?} is not a #rrggbb colour."
        ))),
    }
}

/// A `sources.kind` value read from column `column`.
fn parse_kind(column: usize, kind: &str) -> rusqlite::Result<SourceKind> {
    SourceKind::parse(kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            format!("unknown source kind {kind:?}").into(),
        )
    })
}

/// One source with what its calendars add: the last sync time and the warnings.
fn source_info(c: &Connection, id: &str) -> rusqlite::Result<Option<SourceInfo>> {
    let Some(mut source) = c
        .query_row(
            "SELECT s.id, s.kind, s.display_name, s.account_label, s.status, s.last_error,
                    (SELECT max(last_synced_at) FROM calendars WHERE source_id = s.id)
             FROM sources s WHERE s.id = ?1",
            [id],
            |r| {
                let kind: String = r.get(1)?;
                let status: String = r.get(4)?;
                Ok(SourceInfo {
                    id: r.get(0)?,
                    kind: parse_kind(1, &kind)?,
                    display_name: r.get(2)?,
                    account_label: r.get(3)?,
                    state: match status.as_str() {
                        "auth_required" => SyncState::AuthRequired,
                        "error" => SyncState::Error,
                        _ => SyncState::Ok,
                    },
                    last_error: r.get(5)?,
                    last_synced_at: r.get(6)?,
                    warnings: Vec::new(),
                })
            },
        )
        .optional()?
    else {
        return Ok(None);
    };

    let mut warnings = BTreeSet::new();
    let mut stmt = c.prepare("SELECT warnings FROM calendars WHERE source_id = ?1")?;
    for json in stmt.query_map([id], |r| r.get::<_, String>(0))? {
        // A value this code did not write is skipped rather than failing the list.
        warnings.extend(serde_json::from_str::<Vec<String>>(&json?).unwrap_or_default());
    }
    source.warnings = warnings.into_iter().collect();
    Ok(Some(source))
}

/// The current time in epoch seconds.
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Locks a plain value, also after a panic elsewhere: it is always valid on its own.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
