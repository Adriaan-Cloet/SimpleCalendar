//! Zones that a feed defines itself, in a `VTIMEZONE` block (`docs/architecture.md` §7.7
//! "Feed-defined zones").
//!
//! A `VTIMEZONE` is a list of observances (`STANDARD` and `DAYLIGHT` parts), each saying from
//! which local time on, and by which rule, a new UTC offset applies. `FeedZone` turns them into
//! a sorted list of transitions and answers "which offset applies at this instant" and "which
//! instant is this local time".

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, Offset, TimeZone};
use chrono_tz::Tz;
use rrule::{RRule, RRuleSet, Unvalidated};

/// Onsets are generated up to the start of this year. Later instants keep the last offset.
const LAST_YEAR: i32 = 2200;

const DAY: i64 = 24 * 3600;

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
pub struct FeedZone {
    /// Every onset as `(instant, offset from then on)`, sorted by instant.
    transitions: Vec<(i64, FixedOffset)>,
    /// The offset before the first onset: the `offset_from` of the earliest one.
    initial: FixedOffset,
}

impl FeedZone {
    /// The zone these observances describe, or `None` if there are none.
    ///
    /// A rule that cannot be read is logged and left out; its observance still has its
    /// `DTSTART` and `RDATE` onsets.
    pub fn new(observances: &[Observance]) -> Option<FeedZone> {
        // `(instant, offset before, offset after)` for every onset of every observance.
        let mut onsets: Vec<(i64, FixedOffset, FixedOffset)> = Vec::new();
        for o in observances {
            let from = i64::from(o.offset_from.local_minus_utc());
            for local in local_onsets(o) {
                onsets.push((
                    local.and_utc().timestamp() - from,
                    o.offset_from,
                    o.offset_to,
                ));
            }
        }
        onsets.sort_by_key(|&(instant, ..)| instant);
        onsets.dedup_by_key(|&mut (instant, ..)| instant);
        let &(_, initial, _) = onsets.first()?;
        Some(FeedZone {
            transitions: onsets.into_iter().map(|(at, _, to)| (at, to)).collect(),
            initial,
        })
    }

    /// The offset in force at `instant` (epoch seconds).
    pub fn offset_at(&self, instant: i64) -> FixedOffset {
        // Rust note: `partition_point` is a binary search. It returns how many transitions
        // happened at or before `instant`; the last of those is the one in force.
        match self.transitions.partition_point(|&(at, _)| at <= instant) {
            0 => self.initial,
            n => self.transitions[n - 1].1,
        }
    }

    /// The instant of a local time in this zone, with the RFC 5545 §3.3.5 rules that
    /// `recurrence::resolve_local` applies to IANA zones: a time in a gap is read with the
    /// offset from before the gap, a time that exists twice is the first of the two.
    pub fn to_instant(&self, local: NaiveDateTime) -> i64 {
        let wall = local.and_utc().timestamp();
        let offsets: BTreeSet<i32> = std::iter::once(self.initial)
            .chain(self.transitions.iter().map(|&(_, offset)| offset))
            .map(|offset| offset.local_minus_utc())
            .collect();
        // Every offset that reads `local` as an instant at which that offset is in force. Two
        // of them means the time exists twice; the earlier instant comes first.
        let first = offsets
            .into_iter()
            .map(|offset| wall - i64::from(offset))
            .filter(|&instant| {
                i64::from(self.offset_at(instant).local_minus_utc()) == wall - instant
            })
            .min();
        first.unwrap_or_else(|| {
            // In a gap. The offset a day earlier is the one from before it: changes are months
            // apart.
            wall - i64::from(self.offset_at(wall - DAY).local_minus_utc())
        })
    }

    /// The IANA zones that give the same offset as this zone on the first day of each month,
    /// for `months` months from the month of `from` on, in alphabetical order.
    ///
    /// The offsets are compared at noon UTC, away from the night-time hours when zones change.
    pub fn matching_zones(&self, from: i64, months: u32) -> Vec<Tz> {
        let first = DateTime::from_timestamp(from, 0)
            .unwrap_or_default()
            .date_naive();
        let samples: Vec<(NaiveDateTime, i32)> = (0..months)
            .filter_map(|m| {
                let month0 = first.month0() + m;
                let year = first.year() + i32::try_from(month0 / 12).ok()?;
                let day = NaiveDate::from_ymd_opt(year, month0 % 12 + 1, 1)?;
                let noon = day.and_hms_opt(12, 0, 0)?;
                let offset = self.offset_at(noon.and_utc().timestamp()).local_minus_utc();
                Some((noon, offset))
            })
            .collect();
        let mut zones: Vec<Tz> = chrono_tz::TZ_VARIANTS
            .iter()
            .copied()
            .filter(|tz| {
                samples.iter().all(|(noon, offset)| {
                    tz.offset_from_utc_datetime(noon).fix().local_minus_utc() == *offset
                })
            })
            .collect();
        zones.sort_by_key(|tz| tz.name());
        zones
    }
}

/// Every onset of an observance as a local time: `DTSTART`, the `RDATE`s and what the `RRULE`
/// generates up to `LAST_YEAR`.
fn local_onsets(o: &Observance) -> Vec<NaiveDateTime> {
    let mut onsets = vec![o.start];
    onsets.extend(&o.dates);
    if let Some(rule) = &o.rule {
        match expand_rule(rule, o) {
            Ok(generated) => onsets.extend(generated),
            Err(e) => log::warn!("time zone rule {rule:?} cannot be read: {e}"),
        }
    }
    onsets
}

/// The local times an observance's `RRULE` generates.
///
/// The rule is expanded on the wall clock, written as UTC, as `recurrence::expand` does for
/// floating series. Its `UNTIL` is a UTC instant, so it is moved to the wall clock first by
/// adding `offset_from`, the offset in use just before each onset.
fn expand_rule(rule: &str, o: &Observance) -> Result<Vec<NaiveDateTime>, rrule::RRuleError> {
    let from = i64::from(o.offset_from.local_minus_utc());
    let parts: Vec<String> = rule
        .split(';')
        .map(|part| match part.split_once('=') {
            Some((key, until)) if key.eq_ignore_ascii_case("UNTIL") => {
                format!("UNTIL={}", wall_until(until.trim(), from))
            }
            _ => part.to_owned(),
        })
        .collect();
    let rule: RRule<Unvalidated> = parts.join(";").parse()?;
    let start = o.start.and_utc().with_timezone(&rrule::Tz::UTC);
    let end = NaiveDate::from_ymd_opt(LAST_YEAR, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|end| end.and_utc().with_timezone(&rrule::Tz::UTC))
        .unwrap_or(start);
    let result = RRuleSet::new(start)
        .set_rrules(vec![rule.validate(start)?])
        .before(end)
        .all(u16::MAX);
    Ok(result.dates.iter().map(|dt| dt.naive_utc()).collect())
}

/// An `UNTIL` value on the wall clock, written as UTC. A UTC value is shifted by `from`
/// seconds; a local time or a date (not allowed here, but written by some exporters) is taken
/// as it is, a date including its whole day.
fn wall_until(until: &str, from: i64) -> String {
    let format = |t: NaiveDateTime| t.format("%Y%m%dT%H%M%SZ").to_string();
    if let Some(utc) = until.strip_suffix(['Z', 'z'])
        && let Ok(t) = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S")
    {
        return format(t + chrono::Duration::seconds(from));
    }
    if let Ok(t) = NaiveDateTime::parse_from_str(until, "%Y%m%dT%H%M%S") {
        return format(t);
    }
    if let Some(t) = NaiveDate::parse_from_str(until, "%Y%m%d")
        .ok()
        .and_then(|d| d.and_hms_opt(23, 59, 59))
    {
        return format(t);
    }
    // Unreadable: `rrule` reports it.
    until.to_owned()
}
