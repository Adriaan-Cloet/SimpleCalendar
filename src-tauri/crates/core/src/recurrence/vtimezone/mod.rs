//! Zones that a feed defines itself, in a `VTIMEZONE` block (`docs/architecture.md` §7.7
//! "Feed-defined zones").
//!
//! A `VTIMEZONE` is a list of observances (`STANDARD` and `DAYLIGHT` parts), each saying from
//! which local time on, and by which rule, a new UTC offset applies. `FeedZone` turns them into
//! a sorted list of transitions and answers "which offset applies at this instant" and "which
//! instant is this local time".

#[cfg(test)]
mod tests;

use chrono::{FixedOffset, NaiveDateTime};
use chrono_tz::Tz;

/// One `STANDARD` or `DAYLIGHT` part of a `VTIMEZONE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observance {
    /// `DTSTART`: the first onset, as a local time in `offset_from`.
    pub start: NaiveDateTime,
    /// `TZOFFSETFROM`: the offset in use just before each onset.
    pub offset_from: FixedOffset,
    /// `TZOFFSETTO`: the offset from each onset on.
    pub offset_to: FixedOffset,
    /// The `RRULE` value, if any. Its `UNTIL` is in UTC, as RFC 5545 §3.6.5 requires.
    pub rule: Option<String>,
    /// `RDATE` values: more onsets, as local times in `offset_from`.
    pub dates: Vec<NaiveDateTime>,
}

/// A zone defined by a `VTIMEZONE`, ready to evaluate.
#[derive(Debug, Clone)]
pub struct FeedZone {}

impl FeedZone {
    /// The zone these observances describe, or `None` if there are none.
    pub fn new(observances: &[Observance]) -> Option<FeedZone> {
        let _ = observances;
        todo!()
    }

    /// The offset in force at `instant` (epoch seconds).
    pub fn offset_at(&self, instant: i64) -> FixedOffset {
        let _ = instant;
        todo!()
    }

    /// The instant of a local time in this zone, with the RFC 5545 §3.3.5 rules that
    /// `recurrence::resolve_local` applies to IANA zones: a time in a gap is read with the
    /// offset from before the gap, a time that exists twice is the first of the two.
    pub fn to_instant(&self, local: NaiveDateTime) -> i64 {
        let _ = local;
        todo!()
    }

    /// The IANA zones that give the same offset as this zone on the first day of each month,
    /// for `months` months from the month of `from` on, in alphabetical order.
    pub fn matching_zones(&self, from: i64, months: u32) -> Vec<Tz> {
        let _ = (from, months);
        todo!()
    }
}
