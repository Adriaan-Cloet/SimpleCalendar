//! Creating, reading, changing and deleting events (`docs/architecture.md` §4, §10).
//!
//! Every write checks editability itself (§4): the UI hiding a button is not enough. A write to
//! a remote calendar also queues an outbox row in the same transaction (§7.3) and wakes the sync
//! engine, which sends it before its next pull. A deleted remote event stays hidden with
//! `pending_delete` until the server confirms. What "only this event" and "all events" do to a
//! series is in `series.rs` (#17).
//
// Rust note: like `google.rs`, this child module adds methods to `CalendarService` in its own
// `impl` block, and can use the private items of `service` (`CALENDAR_GONE`, `parse_date`).

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};

use super::series;
use super::text::plain_text;
use super::{CALENDAR_GONE, CalendarService, ServiceError, parse_date, parse_zone};
use crate::CoreEvent;
use crate::db::occurrences;
use crate::model::{Occurrence, SourceKind, TimeKind};
use crate::outbox::{self, Op};
use crate::provider::{MAX_REMINDER_MINUTES, ReminderMethod};
use crate::recurrence::rule::{Frequency, SimpleRule};
use crate::recurrence::{Series, resolve_local, wall_clock};

pub(super) const EVENT_GONE: &str = "This event no longer exists.";
const NO_MOVE: &str = "Only events on this computer can move to another calendar.";
const NO_RULE: &str = "Choose how often the event repeats.";
/// What a blank title becomes.
const UNTITLED: &str = "New event";

/// How `get_event` writes a wall-clock time, and how a draft gives one.
const MINUTES: &str = "%Y-%m-%dT%H:%M";

/// An occurrence as commands name it: the `event_id` and `original_start` of an `Occurrence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct OccurrenceRef {
    pub event_id: i64,
    pub original_start: i64,
}

/// Which part of a series a change applies to (§10). For a single event both mean the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Only the occurrence named.
    This,
    /// The whole series.
    All,
}

/// One reminder of an event.
//
// Rust note: `derive(Ord)` compares the fields in the order they are declared, so a sorted
// list puts the earliest reminder first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventReminder {
    /// 0 to 40320 (four weeks).
    pub minutes_before: u32,
    pub method: ReminderMethod,
}

impl EventReminder {
    pub fn popup(minutes_before: u32) -> EventReminder {
        EventReminder {
            minutes_before,
            method: ReminderMethod::Popup,
        }
    }
}

/// How often an event repeats, as the editor offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
    /// A rule the editor cannot show, such as every two weeks or on chosen weekdays. In a
    /// draft it keeps the series' rule as it is.
    Custom,
}

/// How an event repeats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repeat {
    pub frequency: RepeatFrequency,
    /// The last day an occurrence may start on, `YYYY-MM-DD`, in the event's zone. `None`
    /// repeats forever. Always `None` for `Custom`, and ignored in a draft.
    #[serde(default)]
    pub until: Option<String>,
}

/// An event as the editor and the popover show it (`get_event`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventDetail {
    /// For an occurrence of a series, the series' id.
    pub id: i64,
    pub calendar_id: i64,
    pub title: String,
    /// Plain text. A Google description written as HTML has been turned into text (§7.5).
    pub description: Option<String>,
    pub location: Option<String>,
    pub all_day: bool,
    /// The wall-clock start in `time_zone`, `2026-10-20T09:00`, or `2026-10-20` all-day.
    pub start: String,
    /// Like `start`. Exclusive, so an all-day end is the day after the last day.
    pub end: String,
    /// The event's IANA zone, or an offset such as `+05:30` for an ICS event whose zone could
    /// not be mapped. `None` for all-day and floating times.
    pub time_zone: Option<String>,
    /// The calendar's default reminders apply; `reminders` lists them.
    pub use_default_reminders: bool,
    /// The reminders that apply, earliest first.
    pub reminders: Vec<EventReminder>,
    /// The rule of §4: the calendar is writable, and the event is the calendar's or lets guests
    /// change it.
    pub editable: bool,
    /// The series' rule, also for one of its occurrences. `None` for a single event.
    pub repeat: Option<Repeat>,
}

/// An event as `create_event` and `update_event` receive it. The whole event, not a change:
/// a field that is left out is empty.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EventDraft {
    pub calendar_id: i64,
    /// Trimmed; a blank title becomes "New event".
    pub title: String,
    /// Trimmed; blank means none.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    pub all_day: bool,
    /// `YYYY-MM-DDTHH:MM` in `time_zone`, or `YYYY-MM-DD` for an all-day event.
    pub start: String,
    /// Like `start`, not before it. An all-day end is exclusive, so it lies after the start.
    pub end: String,
    /// An IANA name; needed unless the event is all-day.
    #[serde(default)]
    pub time_zone: Option<String>,
    /// Use the calendar's default reminders. `reminders` is then ignored.
    #[serde(default)]
    pub use_default_reminders: bool,
    #[serde(default)]
    pub reminders: Vec<EventReminder>,
    /// `None` for an event that does not repeat. Its `until` may not lie before the start.
    #[serde(default)]
    pub repeat: Option<Repeat>,
}

/// A series' rule, as the editor sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Rule {
    Simple(SimpleRule),
    /// Kept as it is.
    Custom,
}

impl Rule {
    /// The rule of a `recurrence` column, for a series of `kind`.
    fn read(recurrence: &str, kind: TimeKind) -> Rule {
        SimpleRule::read(recurrence, kind).map_or(Rule::Custom, Rule::Simple)
    }

    /// The `recurrence` column for a series of `kind` that had `before` until now.
    pub(super) fn to_recurrence(&self, kind: TimeKind, before: Option<&str>) -> Option<String> {
        match self {
            Rule::Simple(rule) => Some(rule.to_recurrence(kind)),
            Rule::Custom => before.map(str::to_owned),
        }
    }

    fn repeat(&self) -> Repeat {
        let Rule::Simple(rule) = self else {
            return Repeat {
                frequency: RepeatFrequency::Custom,
                until: None,
            };
        };
        Repeat {
            frequency: match rule.frequency {
                Frequency::Daily => RepeatFrequency::Daily,
                Frequency::Weekly => RepeatFrequency::Weekly,
                Frequency::Monthly => RepeatFrequency::Monthly,
                Frequency::Yearly => RepeatFrequency::Yearly,
            },
            until: rule.until.map(|d| d.format("%Y-%m-%d").to_string()),
        }
    }
}

/// The values of one event row that the editor changes, as stored: from a draft that passed
/// every check, or read from the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fields {
    pub calendar_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub all_day: bool,
    pub start_utc: i64,
    pub end_utc: i64,
    /// `start_tz`; `end_tz` is written the same.
    pub tz: Option<String>,
    pub use_default_reminders: bool,
    /// Sorted, each once. Empty in a draft when the defaults apply.
    pub reminders: Vec<EventReminder>,
}

impl Fields {
    pub fn kind(&self) -> TimeKind {
        TimeKind::from_columns(self.tz.as_deref(), self.all_day)
    }

    pub fn duration(&self) -> i64 {
        self.end_utc - self.start_utc
    }

    /// The same values for an occurrence that starts at `start`.
    pub fn at(&self, start: i64) -> Fields {
        Fields {
            start_utc: start,
            end_utc: start + self.duration(),
            ..self.clone()
        }
    }
}

/// A draft that passed every check.
pub(super) struct Valid {
    pub fields: Fields,
    /// `None` if the event does not repeat.
    pub rule: Option<Rule>,
}

/// An event row as the writes and `get_event` read it.
pub(super) struct Stored {
    pub id: i64,
    pub fields: Fields,
    pub recurrence: Option<String>,
    /// An exception (§4): it replaces the occurrence of its series at `original_start`.
    pub exception: bool,
    pub original_start: Option<i64>,
    pub cancelled: bool,
}

impl Stored {
    /// Its rule, if it is a series.
    pub fn rule(&self) -> Option<Rule> {
        let recurrence = self.recurrence.as_deref()?;
        Some(Rule::read(recurrence, self.fields.kind()))
    }

    /// Whether this series has an occurrence that the rule starts at `start`.
    pub fn produces(&self, start: i64) -> bool {
        self.recurrence.as_deref().is_some_and(|recurrence| {
            crate::recurrence::produces(
                &Series {
                    start: self.fields.start_utc,
                    duration: self.fields.duration(),
                    kind: self.fields.kind(),
                    recurrence,
                },
                start,
            )
        })
    }
}

impl CalendarService {
    /// One event in full, for the editor and the popover. For an occurrence of a series, the
    /// values of that occurrence: the exception's if one replaced it, else the series' at that
    /// date. A single event ignores `original_start`.
    pub async fn get_event(&self, target: OccurrenceRef) -> Result<EventDetail, ServiceError> {
        self.db
            .read(move |c| read_event(c, target))
            .await?
            .ok_or(ServiceError::NotFound(EVENT_GONE))
    }

    /// Creates an event, or a series when the draft repeats, and returns its (first)
    /// occurrence with times in the viewer's zone `tz`, as `list_occurrences` would.
    ///
    /// The calendar must be writable. The event gets a fresh remote id in the form Google
    /// accepts, sent with the insert so a retry cannot create it twice (§7.3), and is the
    /// calendar's own (`is_organizer`, the column default), so it is always editable.
    pub async fn create_event(
        &self,
        draft: EventDraft,
        tz: &str,
    ) -> Result<Occurrence, ServiceError> {
        let viewer = parse_zone(tz)?;
        let valid = validate(draft)?;
        if valid.rule == Some(Rule::Custom) {
            return Err(ServiceError::InvalidInput(NO_RULE.into()));
        }
        let remote_id = uuid::Uuid::new_v4().simple().to_string();
        let occurrence = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                if let Err(refused) = check_calendar(&tx, valid.fields.calendar_id)? {
                    return Ok(Err(refused));
                }
                tx.execute(
                    "INSERT INTO events (calendar_id, remote_id, title, start_utc, end_utc)
                     VALUES (?1, ?2, '', 0, 0)",
                    params![valid.fields.calendar_id, remote_id],
                )?;
                let id = tx.last_insert_rowid();
                write_event(&tx, id, &valid.fields)?;
                let kind = valid.fields.kind();
                let recurrence = valid.rule.and_then(|r| r.to_recurrence(kind, None));
                set_recurrence(&tx, id, recurrence.as_deref())?;
                let push = outbox::queue(&tx, Op::Create, id, None, now())?;
                let occurrence = occurrences::occurrence(&tx, id, valid.fields.start_utc, viewer)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                tx.commit()?;
                Ok(Ok((occurrence, push)))
            })
            .await??;
        let (occurrence, push) = occurrence;
        self.push(push);
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![occurrence.calendar_id],
        });
        Ok(occurrence)
    }

    /// Replaces an event, or one or all occurrences of a series (`series.rs`), with `draft`,
    /// possibly in another calendar. Returns the occurrence that was changed in the viewer's
    /// zone `tz`, or `None` if, after a change to the whole series, the rule no longer produces
    /// it.
    ///
    /// The event must be editable (§4), or this is `NotEditable`, whatever `draft` says. A new
    /// calendar must be writable as well, and only an event on this computer can move: another
    /// account needs its provider's move, which v1 does not use. A single event ignores
    /// `scope`; a draft that repeats turns it into a series.
    pub async fn update_event(
        &self,
        target: OccurrenceRef,
        scope: Scope,
        draft: EventDraft,
        tz: &str,
    ) -> Result<Option<Occurrence>, ServiceError> {
        let viewer = parse_zone(tz)?;
        let valid = validate(draft)?;
        let after = valid.fields.calendar_id;
        let (before, occurrence, push) = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let target = resolve(&tx, target)?;
                let event = match check_event(&tx, target.event_id)? {
                    Ok(event) => event,
                    Err(refused) => return Ok(Err(refused)),
                };
                if let Err(refused) = check_calendar(&tx, valid.fields.calendar_id)? {
                    return Ok(Err(refused));
                }
                let (from, to) = (event.fields.calendar_id, valid.fields.calendar_id);
                if from != to
                    && (outbox::remote_source(&tx, from)?.is_some()
                        || outbox::remote_source(&tx, to)?.is_some())
                {
                    return Ok(Err(ServiceError::InvalidInput(NO_MOVE.into())));
                }
                let (changed, push) = if event.recurrence.is_some() {
                    match series::change(&tx, &event, target.original_start, scope, &valid)? {
                        Ok(changed) => changed,
                        Err(refused) => return Ok(Err(refused)),
                    }
                } else {
                    if valid.rule == Some(Rule::Custom) {
                        return Ok(Err(ServiceError::InvalidInput(NO_RULE.into())));
                    }
                    let base = outbox::read_data(&tx, event.id)?;
                    write_event(&tx, event.id, &valid.fields)?;
                    let kind = valid.fields.kind();
                    let recurrence = valid.rule.and_then(|r| r.to_recurrence(kind, None));
                    set_recurrence(&tx, event.id, recurrence.as_deref())?;
                    let push = outbox::queue(&tx, Op::Update, event.id, base.as_ref(), now())?;
                    (valid.fields.start_utc, push)
                };
                let occurrence = occurrences::occurrence(&tx, event.id, changed, viewer)?;
                tx.commit()?;
                Ok(Ok((event.fields.calendar_id, occurrence, push)))
            })
            .await??;
        self.push(push);
        let mut calendar_ids = vec![before];
        if after != before {
            calendar_ids.push(after);
        }
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self
            .events
            .send(CoreEvent::CalendarChanged { calendar_ids });
        Ok(occurrence)
    }

    /// Deletes an event with its reminders, or one or all occurrences of a series. Like
    /// `update_event`, it must be editable.
    pub async fn delete_event(
        &self,
        target: OccurrenceRef,
        scope: Scope,
    ) -> Result<(), ServiceError> {
        let (calendar_id, push) = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let target = resolve(&tx, target)?;
                let event = match check_event(&tx, target.event_id)? {
                    Ok(event) => event,
                    Err(refused) => return Ok(Err(refused)),
                };
                let push = if event.recurrence.is_some() && scope == Scope::This {
                    match series::cancel(&tx, &event, target.original_start)? {
                        Ok(push) => push,
                        Err(refused) => return Ok(Err(refused)),
                    }
                } else if outbox::remote_source(&tx, event.fields.calendar_id)?.is_some() {
                    // Hidden until the server confirms; then the engine deletes the row.
                    let base = outbox::read_data(&tx, event.id)?;
                    tx.execute(
                        "UPDATE events SET pending_delete = 1 WHERE id = ?1",
                        [event.id],
                    )?;
                    outbox::queue(&tx, Op::Delete, event.id, base.as_ref(), now())?
                } else {
                    // Its reminders and the series' exceptions go with it, through
                    // `ON DELETE CASCADE`.
                    tx.execute("DELETE FROM events WHERE id = ?1", [event.id])?;
                    None
                };
                tx.commit()?;
                Ok(Ok((event.fields.calendar_id, push)))
            })
            .await??;
        self.push(push);
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![calendar_id],
        });
        Ok(())
    }
}

impl CalendarService {
    /// Asks the engine to send what a write just queued for `source`, if anything.
    fn push(&self, source: Option<String>) {
        if let Some(source) = source {
            self.sync.local_write(source);
        }
    }
}

/// The current time in epoch seconds.
pub(super) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Checks a draft and turns it into stored values. Nothing here needs the database.
fn validate(draft: EventDraft) -> Result<Valid, ServiceError> {
    let (start_utc, end_utc, tz, first_day) = if draft.all_day {
        let (start, end) = (parse_date(&draft.start)?, parse_date(&draft.end)?);
        if end <= start {
            return Err(ServiceError::InvalidInput(
                "The event must end on or after the day it starts.".into(),
            ));
        }
        // All-day values are stored as midnight UTC of their date (§4).
        let stored = |date: NaiveDate| date.and_time(NaiveTime::MIN).and_utc().timestamp();
        (stored(start), stored(end), None, start)
    } else {
        let name = draft
            .time_zone
            .as_deref()
            .ok_or_else(|| ServiceError::InvalidInput("Choose a time zone.".into()))?;
        let zone: Tz = parse_zone(name)?;
        let start = parse_wall_clock(&draft.start)?;
        let end = resolve_local(zone, parse_wall_clock(&draft.end)?).timestamp();
        let first_day = start.date();
        let start = resolve_local(zone, start).timestamp();
        if end < start {
            return Err(ServiceError::InvalidInput(
                "The event ends before it starts.".into(),
            ));
        }
        (start, end, Some(zone.name().to_owned()), first_day)
    };

    let mut reminders = if draft.use_default_reminders {
        Vec::new()
    } else {
        draft.reminders
    };
    if reminders
        .iter()
        .any(|r| r.minutes_before > MAX_REMINDER_MINUTES)
    {
        return Err(ServiceError::InvalidInput(
            "A reminder can be at most four weeks before the event.".into(),
        ));
    }
    reminders.sort();
    reminders.dedup();

    let title = draft.title.trim();
    Ok(Valid {
        fields: Fields {
            calendar_id: draft.calendar_id,
            title: if title.is_empty() { UNTITLED } else { title }.to_owned(),
            description: non_blank(draft.description),
            location: non_blank(draft.location),
            all_day: draft.all_day,
            start_utc,
            end_utc,
            tz,
            use_default_reminders: draft.use_default_reminders,
            reminders,
        },
        rule: draft
            .repeat
            .map(|repeat| validate_repeat(repeat, first_day))
            .transpose()?,
    })
}

/// The rule of a draft whose first occurrence is on `first_day`.
//
// Rust note: `transpose` above turns the `Option<Result<..>>` that `map` gives into a
// `Result<Option<..>>`, so `?` can return the error.
fn validate_repeat(repeat: Repeat, first_day: NaiveDate) -> Result<Rule, ServiceError> {
    let frequency = match repeat.frequency {
        RepeatFrequency::Daily => Frequency::Daily,
        RepeatFrequency::Weekly => Frequency::Weekly,
        RepeatFrequency::Monthly => Frequency::Monthly,
        RepeatFrequency::Yearly => Frequency::Yearly,
        RepeatFrequency::Custom => return Ok(Rule::Custom),
    };
    let until = repeat.until.as_deref().map(parse_date).transpose()?;
    if until.is_some_and(|until| until < first_day) {
        return Err(ServiceError::InvalidInput(
            "The event must stop repeating on or after the day it starts.".into(),
        ));
    }
    Ok(Rule::Simple(SimpleRule { frequency, until }))
}

/// A `YYYY-MM-DDTHH:MM` wall-clock time from the frontend. Seconds are accepted as well.
fn parse_wall_clock(value: &str) -> Result<NaiveDateTime, ServiceError> {
    NaiveDateTime::parse_from_str(value, MINUTES)
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|_| ServiceError::InvalidInput(format!("{value:?} is not a date and time.")))
}

/// The trimmed text, or `None` if there is none.
fn non_blank(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Whether events may be created in, or moved to, `calendar_id`. The outer `Result` is the
/// database's, the inner one the refusal (see `update_calendar`).
///
/// `access` is `write` only where the provider writes too (§4), so a subscription never is.
fn check_calendar(c: &Connection, calendar_id: i64) -> rusqlite::Result<Result<(), ServiceError>> {
    let writable: Option<bool> = c
        .query_row(
            "SELECT access = 'write' FROM calendars WHERE id = ?1",
            [calendar_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match writable {
        None => Err(ServiceError::NotFound(CALENDAR_GONE)),
        Some(false) => Err(ServiceError::NotEditable),
        Some(true) => Ok(()),
    })
}

/// Event `event_id`, a single event or a series, if it may be changed or deleted.
fn check_event(c: &Connection, event_id: i64) -> rusqlite::Result<Result<Stored, ServiceError>> {
    let editable: Option<bool> = c
        .query_row(
            "SELECT c.access = 'write' AND (e.is_organizer OR e.guests_can_modify)
             FROM events e JOIN calendars c ON c.id = e.calendar_id
             WHERE e.id = ?1 AND e.pending_delete = 0",
            [event_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match editable {
        None => Err(ServiceError::NotFound(EVENT_GONE)),
        Some(false) => Err(ServiceError::NotEditable),
        // `resolve` has replaced a linked exception by its series. One without a series is
        // invisible (§7.2), so there is nothing to change.
        Some(_) => match load(c, "id = ?1", &[event_id])?.pop() {
            Some(event) if !event.exception => Ok(event),
            _ => Err(ServiceError::NotFound(EVENT_GONE)),
        },
    })
}

/// The occurrence `target` names. The views never name an exception by its own row id, but
/// anything else that does means the occurrence of the series it replaces.
fn resolve(c: &Connection, target: OccurrenceRef) -> rusqlite::Result<OccurrenceRef> {
    Ok(c.query_row(
        "SELECT recurring_event_id, original_start_utc FROM events
         WHERE id = ?1 AND recurring_event_id IS NOT NULL",
        [target.event_id],
        |r| {
            Ok(OccurrenceRef {
                event_id: r.get(0)?,
                original_start: r.get(1)?,
            })
        },
    )
    .optional()?
    .unwrap_or(target))
}

/// The event rows that match `condition`, with their reminders.
pub(super) fn load(
    c: &Connection,
    condition: &str,
    values: &[i64],
) -> rusqlite::Result<Vec<Stored>> {
    let mut events = c
        .prepare(&format!(
            "SELECT id, calendar_id, title, description, location, all_day, start_utc, end_utc,
                    start_tz, use_default_reminders, recurrence, recurring_remote_id IS NOT NULL,
                    original_start_utc, status = 'cancelled'
             FROM events WHERE {condition}"
        ))?
        .query_map(params_from_iter(values), |r| {
            Ok(Stored {
                id: r.get(0)?,
                fields: Fields {
                    calendar_id: r.get(1)?,
                    title: r.get(2)?,
                    description: r.get(3)?,
                    location: r.get(4)?,
                    all_day: r.get(5)?,
                    start_utc: r.get(6)?,
                    end_utc: r.get(7)?,
                    tz: r.get(8)?,
                    use_default_reminders: r.get(9)?,
                    reminders: Vec::new(),
                },
                recurrence: r.get(10)?,
                exception: r.get(11)?,
                original_start: r.get(12)?,
                cancelled: r.get(13)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut reminders =
        c.prepare("SELECT method, minutes_before FROM event_reminders WHERE event_id = ?1")?;
    for event in &mut events {
        event.fields.reminders = reminders
            .query_map([event.id], |r| {
                let method: String = r.get(0)?;
                Ok(EventReminder {
                    // The column's `CHECK` allows only these two.
                    method: ReminderMethod::parse(&method).unwrap_or(ReminderMethod::Popup),
                    minutes_before: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        event.fields.reminders.sort();
    }
    Ok(events)
}

/// Writes every value of `fields` to event `id`, and replaces its reminders. Status, colour,
/// the series columns and the organiser flags stay as they are.
pub(super) fn write_event(c: &Connection, id: i64, fields: &Fields) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE events
         SET calendar_id = ?2, title = ?3, description = ?4, location = ?5, all_day = ?6,
             start_utc = ?7, end_utc = ?8, start_tz = ?9, end_tz = ?9,
             use_default_reminders = ?10
         WHERE id = ?1",
        params![
            id,
            fields.calendar_id,
            fields.title,
            fields.description,
            fields.location,
            fields.all_day,
            fields.start_utc,
            fields.end_utc,
            fields.tz,
            fields.use_default_reminders,
        ],
    )?;
    c.execute("DELETE FROM event_reminders WHERE event_id = ?1", [id])?;
    for reminder in &fields.reminders {
        c.execute(
            "INSERT INTO event_reminders (event_id, method, minutes_before) VALUES (?1, ?2, ?3)",
            params![id, reminder.method.as_str(), reminder.minutes_before],
        )?;
    }
    Ok(())
}

/// Makes event `id` a series with `recurrence`, or a single event with `None`.
pub(super) fn set_recurrence(
    c: &Connection,
    id: i64,
    recurrence: Option<&str>,
) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE events SET recurrence = ?2 WHERE id = ?1",
        params![id, recurrence],
    )?;
    Ok(())
}

/// The event or occurrence `target` names, in full, or `None` if there is none.
fn read_event(c: &Connection, target: OccurrenceRef) -> rusqlite::Result<Option<EventDetail>> {
    let target = resolve(c, target)?;
    let Some((editable, source_kind, defaults)) = c
        .query_row(
            "SELECT c.access = 'write' AND (e.is_organizer OR e.guests_can_modify),
                    s.kind, c.default_reminders
             FROM events e
             JOIN calendars c ON c.id = e.calendar_id
             JOIN sources s ON s.id = c.source_id
             WHERE e.id = ?1 AND e.pending_delete = 0",
            [target.event_id],
            |r| {
                Ok((
                    r.get::<_, bool>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let Some(event) = load(c, "id = ?1", &[target.event_id])?.pop() else {
        return Ok(None);
    };

    let (fields, repeat) = match event.rule() {
        // An exception whose series has not arrived is invisible (§7.2).
        None if event.exception => return Ok(None),
        None => (event.fields, None),
        Some(rule) => match series::find(c, &event, target.original_start)? {
            Some((_, fields)) => (fields, Some(rule.repeat())),
            None => return Ok(None),
        },
    };
    let kind = fields.kind();
    let mut reminders = if fields.use_default_reminders {
        // A value this code did not write is read as no defaults rather than failing.
        serde_json::from_str::<Vec<u32>>(&defaults)
            .unwrap_or_default()
            .into_iter()
            .map(EventReminder::popup)
            .collect()
    } else {
        fields.reminders
    };
    reminders.sort();
    Ok(Some(EventDetail {
        id: event.id,
        calendar_id: fields.calendar_id,
        title: fields.title,
        // Only Google writes HTML. A local description is shown as it was typed, also when it
        // contains something that looks like a tag.
        description: if source_kind == SourceKind::Google.as_str() {
            fields.description.map(|d| plain_text(&d))
        } else {
            fields.description
        },
        location: fields.location,
        all_day: fields.all_day,
        start: written(fields.start_utc, kind),
        end: written(fields.end_utc, kind),
        time_zone: match kind {
            TimeKind::Zoned(tz) => Some(tz.name().to_owned()),
            TimeKind::Offset(offset) => Some(offset.to_string()),
            TimeKind::Floating | TimeKind::AllDay => None,
        },
        use_default_reminders: fields.use_default_reminders,
        reminders,
        editable,
        repeat,
    }))
}

/// A stored value as the wall clock of its own zone, as `get_event` writes it.
fn written(stored: i64, kind: TimeKind) -> String {
    let at = wall_clock(stored, kind);
    match kind {
        TimeKind::AllDay => at.date().format("%Y-%m-%d").to_string(),
        _ => at.format(MINUTES).to_string(),
    }
}
