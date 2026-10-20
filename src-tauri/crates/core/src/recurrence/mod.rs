//! Recurrence expansion and time-zone conversion (`docs/architecture.md` §4, §6, §7.7).
//!
//! `rrule` is used only inside `expand`, so it can be replaced without touching the rest of
//! the crate (§11). It generates the dates of the `RRULE` lines. `RDATE` and `EXDATE` are
//! parsed here, because `rrule` would read a value without `Z` or `TZID` in the machine's
//! own zone, and the values of one event may each use a different form (§7.7 "Mixed forms").

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use chrono::{
    DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Utc,
};
use chrono_tz::Tz;
use rrule::{RRule, RRuleSet, Unvalidated};

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
    let frame = Frame::of(series.kind);
    let mut rules = Vec::new();
    let mut rdates = Vec::new();
    let mut exdates = BTreeSet::new();

    for line in series.recurrence.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        let (name, params, value) = split_line(line)?;
        match name.as_str() {
            "RRULE" => rules.push(with_utc_until(value, series.kind, &frame)?),
            "RDATE" => {
                for v in value.split(',') {
                    rdates.push(DateValue::parse(v, &params)?.to_stored(series.kind));
                }
            }
            "EXDATE" => {
                for v in value.split(',') {
                    exdates.insert(DateValue::parse(v, &params)?.to_stored(series.kind));
                }
            }
            _ => return Err(RecurrenceError::UnsupportedLine(line.to_owned())),
        }
    }
    if rules.is_empty() && rdates.is_empty() {
        return Err(RecurrenceError::Empty);
    }

    // The first occurrence is always part of the series (RFC 5545 §3.8.5.3), also when only
    // RDATEs follow. A `BTreeSet` keeps the starts sorted and drops duplicates.
    let mut starts = BTreeSet::from([series.start]);
    starts.extend(rdates);

    if !rules.is_empty() {
        let dt_start = frame.rrule_time(series.start);
        let validated = rules
            .into_iter()
            .map(|rule| rule.validate(dt_start))
            .collect::<Result<Vec<_>, _>>()?;
        // `after` and `before` are inclusive. Starting one duration before `from` catches the
        // occurrence that is still running at `from`; the exact filter is below.
        let result = RRuleSet::new(dt_start)
            .set_rrules(validated)
            .after(frame.rrule_time(from - series.duration))
            .before(frame.rrule_time(to))
            .all(u16::MAX);
        if result.limited {
            log::warn!(
                "recurrence expansion stopped at rrule's limit: {}",
                series.recurrence
            );
        }
        starts.extend(result.dates.iter().map(|dt| frame.stored_value(dt)));
    }

    Ok(starts
        .into_iter()
        .filter(|start| !exdates.contains(start))
        .filter(|&start| overlaps(start, series.duration, from, to))
        .collect())
}

/// Whether `[start, start + duration)` overlaps `[from, to)`. An event without length counts
/// when it starts inside the range.
pub(crate) fn overlaps(start: i64, duration: i64, from: i64, to: i64) -> bool {
    start < to && (start + duration > from || start >= from)
}

/// Converts a stored value to the viewer's zone.
///
/// Zoned and offset values are instants and keep the instant. Floating values keep their
/// wall-clock time in the viewer's zone. All-day values become their date.
pub fn to_viewer(stored: i64, kind: TimeKind, viewer: Tz) -> OccurrenceTime {
    match kind {
        TimeKind::Zoned(_) | TimeKind::Offset(_) => {
            OccurrenceTime::Timed(instant(stored).with_timezone(&viewer).fixed_offset())
        }
        TimeKind::Floating => OccurrenceTime::Timed(
            resolve_local(viewer, as_written(stored))
                .with_timezone(&viewer)
                .fixed_offset(),
        ),
        TimeKind::AllDay => OccurrenceTime::Date(as_written(stored).date()),
    }
}

/// The instant a stored value stands for when shown in the viewer's zone. For an all-day
/// value that is the viewer's midnight at the start of that date.
pub(crate) fn to_instant(stored: i64, kind: TimeKind, viewer: Tz) -> i64 {
    match kind {
        TimeKind::Zoned(_) | TimeKind::Offset(_) => stored,
        TimeKind::Floating | TimeKind::AllDay => {
            resolve_local(viewer, as_written(stored)).timestamp()
        }
    }
}

/// The instant of a wall-clock time in `tz`, with the RFC 5545 rules for gaps and repeats.
pub fn resolve_local(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(dt) => dt.with_timezone(&Utc),
        // Chrono lists the earlier instant first.
        LocalResult::Ambiguous(first, _) => first.with_timezone(&Utc),
        LocalResult::None => {
            // The offset from before the gap is the zone's offset a day earlier: changes are
            // months apart. Reading the wall clock with it lands after the gap, as the RFC asks.
            let before = tz
                .offset_from_utc_datetime(&(local - Duration::days(1)))
                .fix();
            let utc = local - Duration::seconds(i64::from(before.local_minus_utc()));
            DateTime::from_naive_utc_and_offset(utc, Utc)
        }
    }
}

/// A stored value as an instant.
//
// Rust note: `from_timestamp` returns `None` only for values hundreds of thousands of years
// away, which the database never holds. `unwrap_or_default` falls back to 1970 instead of
// panicking, so damaged data shows up wrong instead of taking the whole query down.
fn instant(stored: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(stored, 0).unwrap_or_default()
}

/// A floating or all-day stored value as the wall-clock time it was written as.
fn as_written(stored: i64) -> NaiveDateTime {
    instant(stored).naive_utc()
}

/// The time frame `rrule` expands in.
///
/// A zoned series is expanded in its own zone, so 09:00 stays 09:00 across daylight saving.
/// Everything else is expanded on the wall clock, written as UTC: floating and all-day values
/// already are, and a fixed offset is shifted there and back, since `rrule` has no fixed
/// offsets and they need no daylight-saving rules anyway.
enum Frame {
    Zone(rrule::Tz),
    /// Seconds to add to a stored value to get the wall clock.
    Shift(i64),
}

impl Frame {
    fn of(kind: TimeKind) -> Frame {
        match kind {
            TimeKind::Zoned(tz) => Frame::Zone(rrule::Tz::Tz(tz)),
            TimeKind::Offset(offset) => Frame::Shift(i64::from(offset.local_minus_utc())),
            TimeKind::Floating | TimeKind::AllDay => Frame::Shift(0),
        }
    }

    fn rrule_time(&self, stored: i64) -> DateTime<rrule::Tz> {
        match self {
            Frame::Zone(tz) => instant(stored).with_timezone(tz),
            Frame::Shift(shift) => instant(stored + shift).with_timezone(&rrule::Tz::UTC),
        }
    }

    fn stored_value(&self, dt: &DateTime<rrule::Tz>) -> i64 {
        match self {
            Frame::Zone(_) => dt.timestamp(),
            Frame::Shift(shift) => dt.timestamp() - shift,
        }
    }
}

/// The parameters of a content line, such as `("TZID", "Europe/Brussels")`.
type Params = Vec<(String, String)>;

/// Splits `NAME;PARAM=x;PARAM=y:value` into its upper-cased name, parameters and value.
fn split_line(line: &str) -> Result<(String, Params, &str), RecurrenceError> {
    let (head, value) = line
        .split_once(':')
        .ok_or_else(|| RecurrenceError::UnsupportedLine(line.to_owned()))?;
    let mut parts = head.split(';');
    // Rust note: `split` always yields at least one item, so this `unwrap_or` never applies.
    let name = parts.next().unwrap_or_default().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_owned()))
        .collect();
    Ok((name, params, value))
}

/// Parses an `RRULE` value with its `UNTIL` rewritten in UTC, in the frame's terms.
///
/// `rrule` reads an `UNTIL` without `Z` in the machine's zone and then rejects it next to a
/// zoned start. RFC 5545 allows the date form and, for floating starts, a local time, and both
/// occur in real feeds. So the value is resolved here like any other date: a date includes the
/// whole day, a local time is read in the series' zone.
fn with_utc_until(
    value: &str,
    kind: TimeKind,
    frame: &Frame,
) -> Result<RRule<Unvalidated>, RecurrenceError> {
    let mut parts = Vec::new();
    for part in value.split(';') {
        match part.split_once('=') {
            Some((key, until)) if key.eq_ignore_ascii_case("UNTIL") => {
                let stored = match DateValue::parse(until, &[])? {
                    DateValue::Date(d) => DateValue::Local(end_of_day(d), None),
                    other => other,
                }
                .to_stored(kind);
                let utc = frame.rrule_time(stored).with_timezone(&Utc);
                parts.push(format!("UNTIL={}", utc.format("%Y%m%dT%H%M%SZ")));
            }
            _ => parts.push(part.to_owned()),
        }
    }
    Ok(parts.join(";").parse()?)
}

fn end_of_day(date: NaiveDate) -> NaiveDateTime {
    // Rust note: 23:59:59 is always a valid time, so `expect` cannot fail here.
    date.and_hms_opt(23, 59, 59).expect("valid time")
}

/// One date or date-time value from an `RDATE`, `EXDATE` or `UNTIL`.
enum DateValue {
    /// Ends in `Z`.
    Utc(NaiveDateTime),
    /// With a `TZID`, or floating when there is none.
    Local(NaiveDateTime, Option<Tz>),
    /// `VALUE=DATE`, or simply eight digits.
    Date(NaiveDate),
}

impl DateValue {
    fn parse(value: &str, params: &[(String, String)]) -> Result<DateValue, RecurrenceError> {
        let value = value.trim();
        let invalid = || RecurrenceError::InvalidDate(value.to_owned());
        if value.len() == 8 {
            return NaiveDate::parse_from_str(value, "%Y%m%d")
                .map(DateValue::Date)
                .map_err(|_| invalid());
        }
        let (text, utc) = match value.strip_suffix(['Z', 'z']) {
            Some(text) => (text, true),
            None => (value, false),
        };
        let naive = NaiveDateTime::parse_from_str(text, "%Y%m%dT%H%M%S").map_err(|_| invalid())?;
        if utc {
            return Ok(DateValue::Utc(naive));
        }
        let tz = match params.iter().find(|(k, _)| k == "TZID") {
            Some((_, name)) => Some(
                name.parse::<Tz>()
                    .map_err(|_| RecurrenceError::UnknownZone(name.clone()))?,
            ),
            None => None,
        };
        Ok(DateValue::Local(naive, tz))
    }

    /// The stored value this date means for a series of `kind`, so it compares with the
    /// generated starts by instant (§7.7 "Mixed forms").
    fn to_stored(&self, kind: TimeKind) -> i64 {
        let wall_clock_series = matches!(kind, TimeKind::Floating | TimeKind::AllDay);
        match *self {
            // A floating or all-day series has no zone to convert an instant into, so the
            // written numbers are compared as they are.
            //
            // Rust note: the `if` guard applies to both patterns joined by `|`.
            DateValue::Utc(naive) | DateValue::Local(naive, _) if wall_clock_series => {
                naive.and_utc().timestamp()
            }
            DateValue::Utc(naive) => naive.and_utc().timestamp(),
            DateValue::Local(naive, Some(tz)) => resolve_local(tz, naive).timestamp(),
            DateValue::Local(naive, None) => local_in_series(naive, kind),
            DateValue::Date(date) => local_in_series(date.and_time(NaiveTime::MIN), kind),
        }
    }
}

/// A wall-clock time without its own zone, read in the series' zone. For a floating or all-day
/// series that is the wall clock itself.
fn local_in_series(naive: NaiveDateTime, kind: TimeKind) -> i64 {
    match kind {
        TimeKind::Zoned(tz) => resolve_local(tz, naive).timestamp(),
        TimeKind::Offset(offset) => {
            naive.and_utc().timestamp() - i64::from(offset.local_minus_utc())
        }
        TimeKind::Floating | TimeKind::AllDay => naive.and_utc().timestamp(),
    }
}
