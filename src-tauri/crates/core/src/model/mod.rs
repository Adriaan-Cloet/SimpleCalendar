//! The calendar data model (`docs/architecture.md` §4).
//!
//! So far: how an event's stored times are meant, the computed `Occurrence`, and the values of
//! the text columns the sync engine writes. `Source`, `Calendar` and `Event` structs follow with
//! the features that need them whole.

use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::{Serialize, Serializer};

/// The kind of a source, stored in `sources.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// The built-in source of local calendars. It has no provider and is never synced.
    Local,
    Ics,
    Google,
    Caldav,
}

impl SourceKind {
    /// The value in `sources.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Local => "local",
            SourceKind::Ics => "ics",
            SourceKind::Google => "google",
            SourceKind::Caldav => "caldav",
        }
    }

    /// Reads a `sources.kind` value. The column's `CHECK` allows no others.
    pub fn parse(value: &str) -> Option<SourceKind> {
        [
            SourceKind::Local,
            SourceKind::Ics,
            SourceKind::Google,
            SourceKind::Caldav,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == value)
    }
}

/// What the account may do on a calendar, stored in `calendars.access`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
}

impl Access {
    /// The value in `calendars.access`.
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }
}

/// The status of an event, stored in `events.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    Confirmed,
    Tentative,
    /// A deleted occurrence of a series, or a deleted series when it is the master (§4).
    Cancelled,
}

impl EventStatus {
    /// The value in `events.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            EventStatus::Confirmed => "confirmed",
            EventStatus::Tentative => "tentative",
            EventStatus::Cancelled => "cancelled",
        }
    }
}

/// How an event's stored `start_utc` / `end_utc` are meant, read from `(start_tz, all_day)`.
///
/// "Stored value" in this crate means the number in those columns. For `Zoned` and `Offset`
/// it is a real instant. For `Floating` and `AllDay` it is a wall-clock time written as if it
/// were UTC, which the viewer's zone turns into an instant (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeKind {
    /// An IANA zone, for example `Europe/Brussels`.
    Zoned(chrono_tz::Tz),
    /// A fixed offset such as `+05:30`, for ICS zones that could not be mapped (§7.7).
    Offset(FixedOffset),
    /// The same wall-clock time wherever the viewer is.
    Floating,
    /// A date, the same everywhere. The end is exclusive.
    AllDay,
}

impl TimeKind {
    /// Reads the kind from the `start_tz` and `all_day` columns.
    ///
    /// A zone name that is neither IANA nor an offset is logged and treated as floating: the
    /// event still shows, at its wall-clock time. The ICS provider resolves names before
    /// storing them (§7.7), so this is for damaged data.
    pub fn from_columns(start_tz: Option<&str>, all_day: bool) -> TimeKind {
        if all_day {
            return TimeKind::AllDay;
        }
        let Some(name) = start_tz else {
            return TimeKind::Floating;
        };
        // An offset always starts with a sign; an IANA name never does.
        if name.starts_with(['+', '-']) {
            if let Ok(offset) = name.parse() {
                return TimeKind::Offset(offset);
            }
        } else if let Ok(tz) = name.parse() {
            return TimeKind::Zoned(tz);
        }
        log::warn!("unknown time zone {name:?}; showing the event as floating");
        TimeKind::Floating
    }
}

/// A start or end as the views show it, in the viewer's zone. Serialises as an ISO 8601 string:
/// `2026-10-20T09:00:00+02:00` for a timed value, `2026-10-20` for a date (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceTime {
    /// A timed value with the viewer's offset at that instant.
    Timed(DateTime<FixedOffset>),
    /// An all-day value. As an end it is exclusive: a one-day event on 25 December ends on
    /// 26 December.
    Date(NaiveDate),
}

impl Serialize for OccurrenceTime {
    // Rust note: `collect_str` serialises anything that implements `Display` as a string,
    // without building a `String` first. Chrono's `format` returns such a value.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            OccurrenceTime::Timed(t) => serializer.collect_str(&t.format("%Y-%m-%dT%H:%M:%S%:z")),
            OccurrenceTime::Date(d) => serializer.collect_str(&d.format("%Y-%m-%d")),
        }
    }
}

/// One appearance of an event on the timeline. Computed per query, never stored (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Occurrence {
    /// The event row. For a series, also when an exception replaced this occurrence, it is the
    /// master's id. Together with `original_start` it identifies the occurrence.
    pub event_id: i64,
    /// The stored start the rule produced, even if an exception moved the occurrence. Equal to
    /// the start for a single event.
    pub original_start: i64,
    pub calendar_id: i64,
    pub title: String,
    pub location: Option<String>,
    /// Event colour, else the user's calendar colour, else the provider's calendar colour.
    pub color: Option<String>,
    pub start: OccurrenceTime,
    pub end: OccurrenceTime,
    /// Whether the UI may offer editing: the calendar is writable and the event's calendar
    /// organises it or lets guests change it (§4).
    pub editable: bool,
}
