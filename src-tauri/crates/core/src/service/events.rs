//! Creating, reading, changing and deleting events (`docs/architecture.md` §4, §10).
//!
//! Every write checks editability itself (§4): the UI hiding a button is not enough. Writes go
//! to local calendars only. A remote calendar needs the outbox (#18) first, or the next sync
//! would quietly undo the change; Google calendars are stored read-only until then (§7.5).
//! Changing one occurrence of a series is #17; until then a series cannot be written.
//
// Rust note: like `google.rs`, this child module adds methods to `CalendarService` in its own
// `impl` block, and can use the private items of `service` (`CALENDAR_GONE`, `parse_date`).

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::text::plain_text;
use super::{CALENDAR_GONE, CalendarService, ServiceError, parse_date, parse_zone};
use crate::CoreEvent;
use crate::db::occurrences::single_occurrence;
use crate::model::{Occurrence, SourceKind, TimeKind};
use crate::provider::{MAX_REMINDER_MINUTES, ReminderMethod};
use crate::recurrence::resolve_local;

const EVENT_GONE: &str = "This event no longer exists.";
const NO_OUTBOX: &str = "Changes to this calendar can’t be saved yet.";
const SERIES_LATER: &str = "Repeating events can’t be changed yet.";
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

/// An event as the editor and the popover show it (`get_event`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventDetail {
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
    /// A series, or one occurrence of one. Writing those comes with #17.
    pub recurring: bool,
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
}

/// A draft that passed every check, in stored values.
struct Valid {
    calendar_id: i64,
    title: String,
    description: Option<String>,
    location: Option<String>,
    all_day: bool,
    start_utc: i64,
    end_utc: i64,
    tz: Option<String>,
    use_default_reminders: bool,
    /// Sorted, each once. Empty when the defaults apply.
    reminders: Vec<EventReminder>,
}

impl CalendarService {
    /// One event in full, for the editor and the popover. For an occurrence of a series pass
    /// the series' id, which is the occurrence's `event_id`.
    pub async fn get_event(&self, event_id: i64) -> Result<EventDetail, ServiceError> {
        self.db
            .read(move |c| read_event(c, event_id))
            .await?
            .ok_or(ServiceError::NotFound(EVENT_GONE))
    }

    /// Creates an event and returns its occurrence with times in the viewer's zone `tz`, as
    /// `list_occurrences` would.
    ///
    /// The calendar must be writable and local. The event gets a fresh remote id in the form
    /// Google accepts, so it can be sent there once remote calendars are writable (§7.3), and
    /// is the calendar's own (`is_organizer`, the column default), so it is always editable.
    pub async fn create_event(
        &self,
        draft: EventDraft,
        tz: &str,
    ) -> Result<Occurrence, ServiceError> {
        let viewer = parse_zone(tz)?;
        let valid = validate(draft)?;
        let remote_id = uuid::Uuid::new_v4().simple().to_string();
        let occurrence = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                if let Err(refused) = check_calendar(&tx, valid.calendar_id)? {
                    return Ok(Err(refused));
                }
                tx.execute(
                    "INSERT INTO events (calendar_id, remote_id, title, start_utc, end_utc)
                     VALUES (?1, ?2, '', 0, 0)",
                    params![valid.calendar_id, remote_id],
                )?;
                let id = tx.last_insert_rowid();
                write_event(&tx, id, &valid)?;
                let occurrence = single_occurrence(&tx, id, viewer)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                tx.commit()?;
                Ok(Ok(occurrence))
            })
            .await??;
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![occurrence.calendar_id],
        });
        Ok(occurrence)
    }

    /// Replaces an event with `draft`, possibly in another calendar, and returns its occurrence
    /// in the viewer's zone `tz`.
    ///
    /// The event must be editable (§4), or this is `NotEditable`, whatever `draft` says. A new
    /// calendar must be writable and local as well. `scope` matters for a series only, which
    /// cannot be written until #17.
    pub async fn update_event(
        &self,
        target: OccurrenceRef,
        _scope: Scope,
        draft: EventDraft,
        tz: &str,
    ) -> Result<Occurrence, ServiceError> {
        let viewer = parse_zone(tz)?;
        let valid = validate(draft)?;
        let (before, occurrence) = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let before = match check_event(&tx, target.event_id)? {
                    Ok(calendar_id) => calendar_id,
                    Err(refused) => return Ok(Err(refused)),
                };
                if let Err(refused) = check_calendar(&tx, valid.calendar_id)? {
                    return Ok(Err(refused));
                }
                write_event(&tx, target.event_id, &valid)?;
                let occurrence = single_occurrence(&tx, target.event_id, viewer)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                tx.commit()?;
                Ok(Ok((before, occurrence)))
            })
            .await??;
        let mut calendar_ids = vec![before];
        if occurrence.calendar_id != before {
            calendar_ids.push(occurrence.calendar_id);
        }
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self
            .events
            .send(CoreEvent::CalendarChanged { calendar_ids });
        Ok(occurrence)
    }

    /// Deletes an event with its reminders. Like `update_event`, it must be editable.
    pub async fn delete_event(
        &self,
        target: OccurrenceRef,
        _scope: Scope,
    ) -> Result<(), ServiceError> {
        let calendar_id = self
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let calendar_id = match check_event(&tx, target.event_id)? {
                    Ok(calendar_id) => calendar_id,
                    Err(refused) => return Ok(Err(refused)),
                };
                // Its reminders go with it, through `ON DELETE CASCADE`.
                tx.execute("DELETE FROM events WHERE id = ?1", [target.event_id])?;
                tx.commit()?;
                Ok(Ok(calendar_id))
            })
            .await??;
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(CoreEvent::CalendarChanged {
            calendar_ids: vec![calendar_id],
        });
        Ok(())
    }
}

/// Checks a draft and turns it into stored values. Nothing here needs the database.
fn validate(draft: EventDraft) -> Result<Valid, ServiceError> {
    let (start_utc, end_utc, tz) = if draft.all_day {
        let (start, end) = (parse_date(&draft.start)?, parse_date(&draft.end)?);
        if end <= start {
            return Err(ServiceError::InvalidInput(
                "The event must end on or after the day it starts.".into(),
            ));
        }
        // All-day values are stored as midnight UTC of their date (§4).
        let stored = |date: NaiveDate| date.and_time(NaiveTime::MIN).and_utc().timestamp();
        (stored(start), stored(end), None)
    } else {
        let name = draft
            .time_zone
            .as_deref()
            .ok_or_else(|| ServiceError::InvalidInput("Choose a time zone.".into()))?;
        let zone: Tz = parse_zone(name)?;
        let start = resolve_local(zone, parse_wall_clock(&draft.start)?).timestamp();
        let end = resolve_local(zone, parse_wall_clock(&draft.end)?).timestamp();
        if end < start {
            return Err(ServiceError::InvalidInput(
                "The event ends before it starts.".into(),
            ));
        }
        (start, end, Some(zone.name().to_owned()))
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
    })
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
fn check_calendar(c: &Connection, calendar_id: i64) -> rusqlite::Result<Result<(), ServiceError>> {
    let found: Option<(String, bool)> = c
        .query_row(
            "SELECT s.kind, c.access = 'write'
             FROM calendars c JOIN sources s ON s.id = c.source_id WHERE c.id = ?1",
            [calendar_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match found {
        None => Err(ServiceError::NotFound(CALENDAR_GONE)),
        Some((_, false)) => Err(ServiceError::NotEditable),
        Some((kind, true)) if kind != SourceKind::Local.as_str() => {
            Err(ServiceError::InvalidInput(NO_OUTBOX.into()))
        }
        Some(_) => Ok(()),
    })
}

/// Whether event `event_id` may be changed or deleted; if so, its calendar.
fn check_event(c: &Connection, event_id: i64) -> rusqlite::Result<Result<i64, ServiceError>> {
    let found: Option<(i64, bool, bool, String)> = c
        .query_row(
            "SELECT e.calendar_id,
                    c.access = 'write' AND (e.is_organizer OR e.guests_can_modify),
                    e.recurrence IS NOT NULL OR e.recurring_remote_id IS NOT NULL,
                    s.kind
             FROM events e
             JOIN calendars c ON c.id = e.calendar_id
             JOIN sources s ON s.id = c.source_id
             WHERE e.id = ?1",
            [event_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(match found {
        None => Err(ServiceError::NotFound(EVENT_GONE)),
        Some((_, false, _, _)) => Err(ServiceError::NotEditable),
        Some((_, _, _, kind)) if kind != SourceKind::Local.as_str() => {
            Err(ServiceError::InvalidInput(NO_OUTBOX.into()))
        }
        Some((_, _, true, _)) => Err(ServiceError::InvalidInput(SERIES_LATER.into())),
        Some((calendar_id, ..)) => Ok(calendar_id),
    })
}

/// Writes every field of `valid` to event `id`, and replaces its reminders. Status, colour,
/// the series columns and the organiser flags stay as they are.
fn write_event(c: &Connection, id: i64, valid: &Valid) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE events
         SET calendar_id = ?2, title = ?3, description = ?4, location = ?5, all_day = ?6,
             start_utc = ?7, end_utc = ?8, start_tz = ?9, end_tz = ?9,
             use_default_reminders = ?10
         WHERE id = ?1",
        params![
            id,
            valid.calendar_id,
            valid.title,
            valid.description,
            valid.location,
            valid.all_day,
            valid.start_utc,
            valid.end_utc,
            valid.tz,
            valid.use_default_reminders,
        ],
    )?;
    c.execute("DELETE FROM event_reminders WHERE event_id = ?1", [id])?;
    for reminder in &valid.reminders {
        c.execute(
            "INSERT INTO event_reminders (event_id, method, minutes_before) VALUES (?1, ?2, ?3)",
            params![id, reminder.method.as_str(), reminder.minutes_before],
        )?;
    }
    Ok(())
}

/// Event `id` in full, or `None` if there is none.
fn read_event(c: &Connection, id: i64) -> rusqlite::Result<Option<EventDetail>> {
    let Some((mut event, defaults)) = c
        .query_row(
            "SELECT e.id, e.calendar_id, e.title, e.description, e.location, e.all_day,
                    e.start_utc, e.end_utc, e.start_tz, e.use_default_reminders,
                    c.access = 'write' AND (e.is_organizer OR e.guests_can_modify),
                    e.recurrence IS NOT NULL OR e.recurring_remote_id IS NOT NULL,
                    s.kind, c.default_reminders
             FROM events e
             JOIN calendars c ON c.id = e.calendar_id
             JOIN sources s ON s.id = c.source_id
             WHERE e.id = ?1",
            [id],
            |r| {
                let all_day: bool = r.get(5)?;
                let start_tz: Option<String> = r.get(8)?;
                let time = TimeKind::from_columns(start_tz.as_deref(), all_day);
                let description: Option<String> = r.get(3)?;
                let source_kind: String = r.get(12)?;
                let event = EventDetail {
                    id: r.get(0)?,
                    calendar_id: r.get(1)?,
                    title: r.get(2)?,
                    // Only Google writes HTML. A local description is shown as it was typed,
                    // also when it contains something that looks like a tag.
                    description: if source_kind == SourceKind::Google.as_str() {
                        description.map(|d| plain_text(&d))
                    } else {
                        description
                    },
                    location: r.get(4)?,
                    all_day,
                    start: wall_clock(r.get(6)?, time),
                    end: wall_clock(r.get(7)?, time),
                    time_zone: match time {
                        TimeKind::Zoned(tz) => Some(tz.name().to_owned()),
                        TimeKind::Offset(offset) => Some(offset.to_string()),
                        TimeKind::Floating | TimeKind::AllDay => None,
                    },
                    use_default_reminders: r.get(9)?,
                    reminders: Vec::new(),
                    editable: r.get(10)?,
                    recurring: r.get(11)?,
                };
                Ok((event, r.get::<_, String>(13)?))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };

    event.reminders = if event.use_default_reminders {
        // A value this code did not write is read as no defaults rather than failing.
        serde_json::from_str::<Vec<u32>>(&defaults)
            .unwrap_or_default()
            .into_iter()
            .map(EventReminder::popup)
            .collect()
    } else {
        let mut stmt =
            c.prepare("SELECT method, minutes_before FROM event_reminders WHERE event_id = ?1")?;
        stmt.query_map([id], |r| {
            let method: String = r.get(0)?;
            Ok(EventReminder {
                // The column's `CHECK` allows only these two.
                method: ReminderMethod::parse(&method).unwrap_or(ReminderMethod::Popup),
                minutes_before: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    event.reminders.sort();
    Ok(Some(event))
}

/// A stored value as the wall clock of its own zone, as `get_event` writes it.
fn wall_clock(stored: i64, time: TimeKind) -> String {
    // A value the database never holds (hundreds of thousands of years away) reads as 1970
    // rather than failing, as in `recurrence`.
    let at = DateTime::from_timestamp(stored, 0).unwrap_or_default();
    match time {
        TimeKind::Zoned(tz) => at.with_timezone(&tz).format(MINUTES).to_string(),
        TimeKind::Offset(offset) => at.with_timezone(&offset).format(MINUTES).to_string(),
        TimeKind::Floating => at.naive_utc().format(MINUTES).to_string(),
        TimeKind::AllDay => at.date_naive().format("%Y-%m-%d").to_string(),
    }
}
