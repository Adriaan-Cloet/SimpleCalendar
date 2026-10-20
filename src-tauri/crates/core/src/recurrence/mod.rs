//! Recurrence expansion and time-zone conversion (`docs/architecture.md` §4, §6, §7.7).
//!
//! `rrule` is used only inside `expand`, so it can be replaced without touching the rest of
//! the crate (§11). It generates the dates of the `RRULE` lines. `RDATE` and `EXDATE` are
//! parsed here, because `rrule` would read a value without `Z` or `TZID` in the machine's
//! own zone, and the values of one event may each use a different form (§7.7 "Mixed forms").

#[cfg(test)]
mod tests;

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;

use crate::model::{OccurrenceTime, TimeKind};

/// A recurring event, as far as expansion needs it.
#[derive(Debug, Clone)]
pub struct Series<'a> {
    /// Stored start of the first occurrence.
    pub start: i64,
    /// Length of every occurrence in seconds (`end_utc - start_utc` of the master).
    pub duration: i64,
    pub kind: TimeKind,
    /// The `recurrence` column: `RRULE`, `RDATE` and `EXDATE` lines, one per line.
    pub recurrence: &'a str,
}

/// Why a series could not be expanded.
#[derive(Debug, thiserror::Error)]
pub enum RecurrenceError {
    #[error("no RRULE or RDATE line")]
    Empty,
    #[error("unsupported line: {0}")]
    UnsupportedLine(String),
    #[error("invalid date or time: {0}")]
    InvalidDate(String),
    #[error("unknown time zone: {0}")]
    UnknownZone(String),
    #[error("rrule: {0}")]
    Rule(#[from] rrule::RRuleError),
}

/// The stored starts of every occurrence of `series` that overlaps `[from, to)`, in order.
///
/// `from` and `to` are stored values, in the same terms as `series.start`. An occurrence that
/// began before `from` and is still running counts as overlapping; one that ends exactly at
/// `from` does not, because ends are exclusive.
///
/// Local times follow RFC 5545 §3.3.5: a time that does not exist because the clock jumps
/// forward is read with the offset from before the jump (02:30 becomes 03:30), and a time that
/// exists twice is the first of the two.
pub fn expand(series: &Series<'_>, from: i64, to: i64) -> Result<Vec<i64>, RecurrenceError> {
    todo!()
}

/// Converts a stored value to the viewer's zone.
///
/// Zoned and offset values are instants and keep the instant. Floating values keep their
/// wall-clock time in the viewer's zone. All-day values become their date.
pub fn to_viewer(stored: i64, kind: TimeKind, viewer: Tz) -> OccurrenceTime {
    todo!()
}

/// The instant of a wall-clock time in `tz`, with the RFC 5545 rules for gaps and repeats.
pub fn resolve_local(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    todo!()
}
