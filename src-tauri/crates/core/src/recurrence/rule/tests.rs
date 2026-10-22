//! The editor's rules on their own, and how `expand` reads them. 2026 dates: Europe changes to
//! summer time on 29 March and back on 25 October.

use chrono::{FixedOffset, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Europe::Brussels;

use super::*;
use crate::recurrence::{Series, expand, from_wall_clock, wall_clock};

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

// ---- helpers -------------------------------------------------------------------------------

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
    date(y, mo, d).and_hms_opt(h, mi, 0).unwrap()
}

fn rule(frequency: Frequency, until: Option<NaiveDate>) -> SimpleRule {
    SimpleRule { frequency, until }
}

const ZONED: TimeKind = TimeKind::Zoned(Brussels);

/// The starts of `recurrence` from `start` within 2026 and 2027, as wall-clock times in its zone.
fn starts(start: i64, kind: TimeKind, recurrence: &str) -> Vec<String> {
    let series = Series {
        start,
        duration: HOUR,
        kind,
        recurrence,
    };
    let (from, to) = (
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp(),
        Utc.with_ymd_and_hms(2028, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp(),
    );
    expand(&series, from, to)
        .unwrap()
        .into_iter()
        .map(|s| wall_clock(s, kind).format("%Y-%m-%d %H:%M").to_string())
        .collect()
}

// ---- writing -------------------------------------------------------------------------------

#[test]
fn a_rule_without_an_end_is_only_its_frequency() {
    assert_eq!(
        rule(Frequency::Daily, None).to_recurrence(ZONED),
        "RRULE:FREQ=DAILY"
    );
    assert_eq!(
        rule(Frequency::Weekly, None).to_recurrence(TimeKind::AllDay),
        "RRULE:FREQ=WEEKLY"
    );
    assert_eq!(
        rule(Frequency::Monthly, None).to_recurrence(TimeKind::Floating),
        "RRULE:FREQ=MONTHLY"
    );
    assert_eq!(
        rule(Frequency::Yearly, None).to_recurrence(ZONED),
        "RRULE:FREQ=YEARLY"
    );
}

#[test]
fn a_zoned_end_is_the_last_second_of_that_day_in_utc() {
    // RFC 5545 §3.3.10: next to a zoned start, UNTIL is in UTC. 31 December is in winter time.
    assert_eq!(
        rule(Frequency::Weekly, Some(date(2026, 12, 31))).to_recurrence(ZONED),
        "RRULE:FREQ=WEEKLY;UNTIL=20261231T225959Z"
    );
    // In summer the offset is two hours.
    assert_eq!(
        rule(Frequency::Daily, Some(date(2026, 7, 1))).to_recurrence(ZONED),
        "RRULE:FREQ=DAILY;UNTIL=20260701T215959Z"
    );
    let india = TimeKind::Offset(FixedOffset::east_opt(5 * 3600 + 1800).unwrap());
    assert_eq!(
        rule(Frequency::Daily, Some(date(2026, 7, 1))).to_recurrence(india),
        "RRULE:FREQ=DAILY;UNTIL=20260701T182959Z"
    );
}

#[test]
fn an_all_day_end_is_a_date_and_a_floating_one_a_local_time() {
    assert_eq!(
        rule(Frequency::Yearly, Some(date(2030, 12, 25))).to_recurrence(TimeKind::AllDay),
        "RRULE:FREQ=YEARLY;UNTIL=20301225"
    );
    assert_eq!(
        rule(Frequency::Daily, Some(date(2026, 7, 1))).to_recurrence(TimeKind::Floating),
        "RRULE:FREQ=DAILY;UNTIL=20260701T235959"
    );
}

// ---- reading -------------------------------------------------------------------------------

#[test]
fn every_rule_the_editor_writes_reads_back() {
    let kinds = [
        ZONED,
        TimeKind::Offset(FixedOffset::west_opt(3 * 3600).unwrap()),
        TimeKind::Floating,
        TimeKind::AllDay,
    ];
    let frequencies = [
        Frequency::Daily,
        Frequency::Weekly,
        Frequency::Monthly,
        Frequency::Yearly,
    ];
    for kind in kinds {
        for frequency in frequencies {
            for until in [None, Some(date(2026, 3, 29)), Some(date(2026, 10, 25))] {
                let written = rule(frequency, until);
                let text = written.to_recurrence(kind);
                assert_eq!(
                    SimpleRule::read(&text, kind),
                    Some(written),
                    "{text} {kind:?}"
                );
            }
        }
    }
}

#[test]
fn rules_from_elsewhere_read_when_the_editor_can_show_them() {
    // Google writes this for "weekly until 31 December" in Brussels.
    assert_eq!(
        SimpleRule::read("RRULE:FREQ=WEEKLY;UNTIL=20261231T225959Z", ZONED),
        Some(rule(Frequency::Weekly, Some(date(2026, 12, 31))))
    );
    // A date UNTIL next to a timed start, a lower-case name, an interval of 1 and a week start
    // change nothing that the editor shows.
    assert_eq!(
        SimpleRule::read("rrule:FREQ=DAILY;UNTIL=20261231", ZONED),
        Some(rule(Frequency::Daily, Some(date(2026, 12, 31))))
    );
    assert_eq!(
        SimpleRule::read("RRULE:INTERVAL=1;FREQ=MONTHLY;WKST=MO\n", ZONED),
        Some(rule(Frequency::Monthly, None))
    );
}

#[test]
fn rules_the_editor_cannot_show_are_custom() {
    for text in [
        "RRULE:FREQ=WEEKLY;INTERVAL=2",
        "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
        "RRULE:FREQ=DAILY;COUNT=5",
        "RRULE:FREQ=HOURLY",
        "RRULE:FREQ=MONTHLY;BYMONTHDAY=-1",
        "RRULE:FREQ=DAILY\nEXDATE:20261021T070000Z",
        "RRULE:FREQ=DAILY\nRRULE:FREQ=WEEKLY",
        "RDATE:20261021T070000Z",
        "RRULE;X-PARAM=1:FREQ=DAILY",
        "RRULE:UNTIL=20261231",
        "RRULE:FREQ=DAILY;UNTIL=nonsense",
        "",
    ] {
        assert_eq!(SimpleRule::read(text, ZONED), None, "{text:?}");
    }
}

// ---- expanding -----------------------------------------------------------------------------

#[test]
fn a_weekly_rule_keeps_its_time_across_daylight_saving_and_ends_on_its_last_day() {
    let start = from_wall_clock(at(2026, 10, 20, 9, 0), ZONED);
    let text = rule(Frequency::Weekly, Some(date(2026, 11, 3))).to_recurrence(ZONED);
    assert_eq!(
        starts(start, ZONED, &text),
        vec!["2026-10-20 09:00", "2026-10-27 09:00", "2026-11-03 09:00"]
    );
}

#[test]
fn an_all_day_yearly_rule_ends_on_its_last_day() {
    let start = from_wall_clock(at(2026, 12, 25, 0, 0), TimeKind::AllDay);
    let text = rule(Frequency::Yearly, Some(date(2027, 12, 25))).to_recurrence(TimeKind::AllDay);
    assert_eq!(
        starts(start, TimeKind::AllDay, &text),
        vec!["2026-12-25 00:00", "2027-12-25 00:00"]
    );
}

#[test]
fn a_monthly_rule_on_the_31st_skips_shorter_months() {
    // RFC 5545 §3.3.10: an invalid date is skipped, not moved. Google does the same.
    let start = from_wall_clock(at(2026, 1, 31, 9, 0), ZONED);
    let text = rule(Frequency::Monthly, Some(date(2026, 5, 31))).to_recurrence(ZONED);
    assert_eq!(
        starts(start, ZONED, &text),
        vec!["2026-01-31 09:00", "2026-03-31 09:00", "2026-05-31 09:00"]
    );
}

// ---- wall clock ----------------------------------------------------------------------------

#[test]
fn the_wall_clock_of_a_stored_value_round_trips() {
    // The night summer time ends: 02:30 exists twice, and is read as the first (RFC 5545).
    let first = from_wall_clock(at(2026, 10, 25, 2, 30), ZONED);
    assert_eq!(wall_clock(first, ZONED), at(2026, 10, 25, 2, 30));
    assert_eq!(wall_clock(first + HOUR, ZONED), at(2026, 10, 25, 2, 30));
    assert_eq!(
        from_wall_clock(at(2026, 10, 25, 2, 30) + chrono::Duration::days(1), ZONED) - first,
        DAY + HOUR
    );
    // The night it starts: 02:30 does not exist and becomes 03:30.
    let gap = from_wall_clock(at(2026, 3, 29, 2, 30), ZONED);
    assert_eq!(wall_clock(gap, ZONED), at(2026, 3, 29, 3, 30));
    // Wall-clock kinds are stored as written.
    for kind in [TimeKind::Floating, TimeKind::AllDay] {
        let stored = from_wall_clock(at(2026, 3, 29, 2, 30), kind);
        assert_eq!(wall_clock(stored, kind), at(2026, 3, 29, 2, 30));
    }
    let offset = TimeKind::Offset(FixedOffset::east_opt(5 * 3600 + 1800).unwrap());
    let stored = from_wall_clock(at(2026, 3, 29, 2, 30), offset);
    assert_eq!(wall_clock(stored, offset), at(2026, 3, 29, 2, 30));
}
