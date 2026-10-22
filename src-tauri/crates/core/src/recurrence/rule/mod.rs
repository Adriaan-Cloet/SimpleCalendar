//! The repeat rules the event editor offers: every day, week, month or year, until an optional
//! last day (`docs/architecture.md` §10, #17).
//!
//! A series stores its rule as `RRULE` text like any other (§4). `SimpleRule` writes that text
//! and reads it back when it has this simple shape; anything else (an interval, chosen
//! weekdays, a count, extra or excluded dates) is a rule the editor shows as "custom" and keeps.

#[cfg(test)]
mod tests;

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Serialize};

use super::{DateValue, end_of_day, from_wall_clock, split_line, wall_clock};
use crate::model::TimeKind;

/// How often a simple rule repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// A rule the editor can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimpleRule {
    pub frequency: Frequency,
    /// The last day an occurrence may start on, in the series' own zone. `None` repeats forever.
    pub until: Option<NaiveDate>,
}

impl Frequency {
    fn as_str(self) -> &'static str {
        match self {
            Frequency::Daily => "DAILY",
            Frequency::Weekly => "WEEKLY",
            Frequency::Monthly => "MONTHLY",
            Frequency::Yearly => "YEARLY",
        }
    }
}

impl SimpleRule {
    /// The `recurrence` column for a series of `kind`.
    ///
    /// `UNTIL` takes the form RFC 5545 §3.3.10 asks for next to such a start: a date for an
    /// all-day series, a local time for a floating one, and otherwise the last second of that
    /// day in the series' zone, in UTC. That is also what Google expects.
    pub fn to_recurrence(&self, kind: TimeKind) -> String {
        let freq = self.frequency.as_str();
        let Some(until) = self.until else {
            return format!("RRULE:FREQ={freq}");
        };
        let until = match kind {
            TimeKind::AllDay => until.format("%Y%m%d").to_string(),
            TimeKind::Floating => end_of_day(until).format("%Y%m%dT%H%M%S").to_string(),
            TimeKind::Zoned(_) | TimeKind::Offset(_) => {
                let utc = from_wall_clock(end_of_day(until), kind);
                // Rust note: see `instant` in the parent module for why this cannot fail.
                DateTime::from_timestamp(utc, 0)
                    .unwrap_or_default()
                    .format("%Y%m%dT%H%M%SZ")
                    .to_string()
            }
        };
        format!("RRULE:FREQ={freq};UNTIL={until}")
    }

    /// The rule in `recurrence`, if it is one the editor can show: one `RRULE` line with a
    /// daily to yearly `FREQ`, and at most an `UNTIL`, an `INTERVAL` of 1 and a `WKST`.
    //
    // Rust note: `?` on an `Option` returns `None` from the function at the first `None`, as
    // it returns the error for a `Result`. `.ok()` turns a `Result` into an `Option` for that.
    pub fn read(recurrence: &str, kind: TimeKind) -> Option<SimpleRule> {
        let mut lines = recurrence.lines().map(str::trim).filter(|l| !l.is_empty());
        let line = lines.next()?;
        if lines.next().is_some() {
            return None;
        }
        let (name, params, value) = split_line(line).ok()?;
        if name != "RRULE" || !params.is_empty() {
            return None;
        }

        let (mut frequency, mut until) = (None, None);
        for part in value.split(';') {
            let (key, value) = part.split_once('=')?;
            match key.to_ascii_uppercase().as_str() {
                "FREQ" => {
                    frequency = Some(match value.to_ascii_uppercase().as_str() {
                        "DAILY" => Frequency::Daily,
                        "WEEKLY" => Frequency::Weekly,
                        "MONTHLY" => Frequency::Monthly,
                        "YEARLY" => Frequency::Yearly,
                        _ => return None,
                    })
                }
                "UNTIL" => until = Some(last_day(value, kind)?),
                "INTERVAL" if value == "1" => {}
                "WKST" => {}
                _ => return None,
            }
        }
        Some(SimpleRule {
            frequency: frequency?,
            until,
        })
    }
}

/// The day an `UNTIL` value falls on in the series' own zone.
fn last_day(value: &str, kind: TimeKind) -> Option<NaiveDate> {
    Some(match DateValue::parse(value, &[]).ok()? {
        DateValue::Date(date) => date,
        other => wall_clock(other.to_stored(kind), kind).date(),
    })
}
