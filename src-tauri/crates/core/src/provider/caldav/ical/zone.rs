//! A `VTIMEZONE` for an IANA zone (RFC 5545 §3.6.5).
//!
//! `chrono-tz` knows the offset at any instant but not the rules behind it, so the rules are
//! read back from one year: its changes of offset are found by searching, and each becomes a
//! yearly rule on the same weekday of the month ("the last Sunday of March"). That is how
//! every zone with daylight saving has worked for decades. Readers that know the IANA name use
//! it anyway; the block is for those that go by the rules alone.

use chrono::{DateTime, Datelike, Months, NaiveDate, Offset, TimeZone, Weekday};
use chrono_tz::{OffsetComponents, OffsetName, Tz};

use super::Component;

const DAY: i64 = 24 * 3600;

/// The offset in effect at an instant.
#[derive(Debug, Clone, PartialEq, Eq)]
struct State {
    /// Seconds east of UTC.
    offset: i32,
    daylight: bool,
    name: Option<String>,
}

fn state(tz: Tz, at: i64) -> State {
    let utc = DateTime::from_timestamp(at, 0)
        .unwrap_or_default()
        .naive_utc();
    let offset = tz.offset_from_utc_datetime(&utc);
    State {
        offset: offset.fix().local_minus_utc(),
        daylight: !offset.dst_offset().is_zero(),
        name: offset.abbreviation().map(str::to_owned),
    }
}

/// The `VTIMEZONE` for `tz`, with the rules in force in `year`.
pub(super) fn vtimezone(tz: Tz, year: i32) -> Component {
    let start = NaiveDate::from_ymd_opt(year, 1, 1)
        .unwrap_or_default()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc()
        .timestamp();
    let end = start + 366 * DAY;

    let mut changes = Vec::new();
    let mut before = start;
    while before < end {
        let after = (before + DAY).min(end);
        if state(tz, before).offset != state(tz, after).offset {
            // The first second of the new offset.
            let (mut lo, mut hi) = (before, after);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if state(tz, mid).offset == state(tz, lo).offset {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            changes.push(hi);
        }
        before = after;
    }

    let mut zone = Component::new("VTIMEZONE", [format!("TZID:{}", tz.name())]);
    if changes.is_empty() {
        let now = state(tz, start);
        zone.nodes.push(super::Node::Component(observance(
            &now,
            &now,
            "19700101T000000".to_owned(),
            None,
        )));
        return zone;
    }
    for at in changes {
        let (from, to) = (state(tz, at - 1), state(tz, at));
        // The onset is written in the local time before the change.
        let onset = DateTime::from_timestamp(at + i64::from(from.offset), 0)
            .unwrap_or_default()
            .naive_utc();
        zone.nodes.push(super::Node::Component(observance(
            &from,
            &to,
            onset.format("%Y%m%dT%H%M%S").to_string(),
            Some(yearly(onset.date())),
        )));
    }
    zone
}

/// A `STANDARD` or `DAYLIGHT` part, going from one offset to the next.
fn observance(from: &State, to: &State, start: String, rule: Option<String>) -> Component {
    let mut lines = vec![
        format!("DTSTART:{start}"),
        format!("TZOFFSETFROM:{}", offset_text(from.offset)),
        format!("TZOFFSETTO:{}", offset_text(to.offset)),
    ];
    lines.extend(to.name.as_ref().map(|n| format!("TZNAME:{n}")));
    lines.extend(rule);
    Component::new(if to.daylight { "DAYLIGHT" } else { "STANDARD" }, lines)
}

/// The yearly rule that falls on `date`: its weekday, counted from the start of the month, or
/// as the last one when no other follows in that month.
fn yearly(date: NaiveDate) -> String {
    let last_day = date
        .with_day(1)
        .and_then(|first| first.checked_add_months(Months::new(1)))
        .and_then(|next| next.pred_opt())
        .map_or(31, |d| d.day());
    let week = if date.day() + 7 > last_day {
        -1
    } else {
        (date.day() as i32 - 1) / 7 + 1
    };
    format!(
        "RRULE:FREQ=YEARLY;BYMONTH={};BYDAY={week}{}",
        date.month(),
        weekday(date.weekday())
    )
}

fn weekday(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

/// `+0130` / `-0500`. Seconds are dropped; no zone uses them today.
fn offset_text(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    format!("{sign}{:02}{:02}", minutes / 60, minutes % 60)
}
