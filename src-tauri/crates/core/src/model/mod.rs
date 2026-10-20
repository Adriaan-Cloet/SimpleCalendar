//! The calendar data model (`docs/architecture.md` §4).
//!
//! Only what reading a date range needs so far: how an event's stored times are meant, and the
//! computed `Occurrence`. `Source`, `Calendar` and `Event` follow with the features that write
//! them.

use chrono::{DateTime, FixedOffset, NaiveDate};

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
        todo!()
    }
}

/// A start or end as the views show it, in the viewer's zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceTime {
    /// A timed value with the viewer's offset at that instant.
    Timed(DateTime<FixedOffset>),
    /// An all-day value. As an end it is exclusive: a one-day event on 25 December ends on
    /// 26 December.
    Date(NaiveDate),
}

/// One appearance of an event on the timeline. Computed per query, never stored (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Whether the UI may offer editing. In v0.1 this is the calendar's write access; v0.2
    /// adds the organizer rule from §4.
    pub editable: bool,
}
