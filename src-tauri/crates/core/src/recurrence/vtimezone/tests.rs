//! The evaluator on its own. 2026 dates: Europe changes to summer time on 29 March at 01:00 UTC
//! and back on 25 October at 01:00 UTC.

use chrono::{NaiveDate, TimeZone, Utc};

use super::*;

const HOUR: i32 = 3600;

// ---- helpers -------------------------------------------------------------------------------

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .unwrap()
        .timestamp()
}

fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
}

fn offset(seconds: i32) -> FixedOffset {
    FixedOffset::east_opt(seconds).unwrap()
}

fn observance(
    start: NaiveDateTime,
    from: i32,
    to: i32,
    rule: Option<&str>,
    dates: &[NaiveDateTime],
) -> Observance {
    Observance {
        start,
        offset_from: offset(from),
        offset_to: offset(to),
        rule: rule.map(str::to_owned),
        dates: dates.to_vec(),
    }
}

/// Central European rules, as most exporters write them.
fn europe() -> FeedZone {
    FeedZone::new(&[
        observance(
            local(1970, 10, 25, 3, 0),
            2 * HOUR,
            HOUR,
            Some("FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU"),
            &[],
        ),
        observance(
            local(1970, 3, 29, 2, 0),
            HOUR,
            2 * HOUR,
            Some("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU"),
            &[],
        ),
    ])
    .unwrap()
}

fn constant(seconds: i32) -> FeedZone {
    FeedZone::new(&[observance(
        local(1970, 1, 1, 0, 0),
        seconds,
        seconds,
        None,
        &[],
    )])
    .unwrap()
}

fn names(zones: &[Tz]) -> Vec<&str> {
    zones.iter().map(|tz| tz.name()).collect()
}

// ---- offsets -------------------------------------------------------------------------------

#[test]
fn no_observances_is_no_zone() {
    assert!(FeedZone::new(&[]).is_none());
}

#[test]
fn winter_and_summer_offsets() {
    let zone = europe();
    assert_eq!(zone.offset_at(utc(2026, 1, 15, 12, 0)), offset(HOUR));
    assert_eq!(zone.offset_at(utc(2026, 7, 1, 12, 0)), offset(2 * HOUR));
    assert_eq!(zone.offset_at(utc(2026, 12, 1, 12, 0)), offset(HOUR));
}

#[test]
fn the_change_happens_at_the_onset() {
    let zone = europe();
    let spring = utc(2026, 3, 29, 1, 0);
    assert_eq!(zone.offset_at(spring - 1), offset(HOUR));
    assert_eq!(zone.offset_at(spring), offset(2 * HOUR));
    let autumn = utc(2026, 10, 25, 1, 0);
    assert_eq!(zone.offset_at(autumn - 1), offset(2 * HOUR));
    assert_eq!(zone.offset_at(autumn), offset(HOUR));
}

#[test]
fn before_the_first_onset_the_offset_from_applies() {
    let zone = europe();
    assert_eq!(zone.offset_at(utc(1960, 6, 1, 0, 0)), offset(HOUR));
}

#[test]
fn a_constant_zone_has_one_offset() {
    let zone = constant(5 * HOUR + 30 * 60);
    for instant in [
        utc(1960, 1, 1, 0, 0),
        utc(2026, 3, 29, 1, 0),
        utc(2040, 7, 1, 0, 0),
    ] {
        assert_eq!(zone.offset_at(instant), offset(5 * HOUR + 30 * 60));
    }
}

/// Outlook starts its rules in 1601.
#[test]
fn rules_that_start_in_1601_are_evaluated() {
    let zone = FeedZone::new(&[
        observance(
            local(1601, 10, 28, 3, 0),
            2 * HOUR,
            HOUR,
            Some("FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10"),
            &[],
        ),
        observance(
            local(1601, 3, 25, 2, 0),
            HOUR,
            2 * HOUR,
            Some("FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3"),
            &[],
        ),
    ])
    .unwrap();
    assert_eq!(zone.offset_at(utc(2026, 7, 1, 12, 0)), offset(2 * HOUR));
    assert_eq!(zone.offset_at(utc(2026, 3, 29, 1, 0) - 1), offset(HOUR));
}

/// The United States moved its changes in 2007. Feeds keep the old rule with an `UNTIL` next
/// to the new one.
#[test]
fn a_rule_stops_at_its_until() {
    let zone = FeedZone::new(&[
        observance(
            local(1967, 10, 29, 2, 0),
            -4 * HOUR,
            -5 * HOUR,
            Some("FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU;UNTIL=20061029T060000Z"),
            &[],
        ),
        observance(
            local(1987, 4, 5, 2, 0),
            -5 * HOUR,
            -4 * HOUR,
            Some("FREQ=YEARLY;BYMONTH=4;BYDAY=1SU;UNTIL=20060402T070000Z"),
            &[],
        ),
        observance(
            local(2007, 3, 11, 2, 0),
            -5 * HOUR,
            -4 * HOUR,
            Some("FREQ=YEARLY;BYMONTH=3;BYDAY=2SU"),
            &[],
        ),
        observance(
            local(2007, 11, 4, 2, 0),
            -4 * HOUR,
            -5 * HOUR,
            Some("FREQ=YEARLY;BYMONTH=11;BYDAY=1SU"),
            &[],
        ),
    ])
    .unwrap();
    // 2005: summer time from 3 April to 30 October.
    assert_eq!(zone.offset_at(utc(2005, 3, 20, 12, 0)), offset(-5 * HOUR));
    assert_eq!(zone.offset_at(utc(2005, 10, 31, 12, 0)), offset(-5 * HOUR));
    // 2026: from 8 March to 1 November.
    assert_eq!(zone.offset_at(utc(2026, 3, 20, 12, 0)), offset(-4 * HOUR));
    assert_eq!(zone.offset_at(utc(2026, 10, 31, 12, 0)), offset(-4 * HOUR));
    assert_eq!(zone.offset_at(utc(2026, 11, 2, 12, 0)), offset(-5 * HOUR));
}

#[test]
fn rdates_are_onsets_too() {
    let zone = FeedZone::new(&[
        observance(
            local(1970, 1, 1, 0, 0),
            HOUR,
            HOUR,
            None,
            &[local(2026, 11, 1, 0, 0)],
        ),
        observance(local(2026, 6, 1, 0, 0), HOUR, 3 * HOUR, None, &[]),
    ])
    .unwrap();
    assert_eq!(zone.offset_at(utc(2026, 5, 1, 0, 0)), offset(HOUR));
    assert_eq!(zone.offset_at(utc(2026, 7, 1, 0, 0)), offset(3 * HOUR));
    assert_eq!(zone.offset_at(utc(2026, 12, 1, 0, 0)), offset(HOUR));
}

// ---- local times ---------------------------------------------------------------------------

#[test]
fn a_local_time_becomes_its_instant() {
    let zone = europe();
    assert_eq!(
        zone.to_instant(local(2026, 7, 1, 9, 0)),
        utc(2026, 7, 1, 7, 0)
    );
    assert_eq!(
        zone.to_instant(local(2026, 1, 15, 9, 0)),
        utc(2026, 1, 15, 8, 0)
    );
}

#[test]
fn a_time_in_the_spring_gap_uses_the_offset_from_before() {
    // 02:30 does not exist on 29 March; read with +01:00 it is 01:30 UTC, shown as 03:30.
    assert_eq!(
        europe().to_instant(local(2026, 3, 29, 2, 30)),
        utc(2026, 3, 29, 1, 30)
    );
}

#[test]
fn a_time_that_exists_twice_is_the_first() {
    // 02:30 on 25 October happens at +02:00 and again at +01:00.
    assert_eq!(
        europe().to_instant(local(2026, 10, 25, 2, 30)),
        utc(2026, 10, 25, 0, 30)
    );
}

#[test]
fn a_constant_zone_subtracts_its_offset() {
    let zone = constant(HOUR + 17 * 60);
    assert_eq!(
        zone.to_instant(local(2026, 6, 1, 9, 0)),
        utc(2026, 6, 1, 7, 43)
    );
}

// ---- matching ------------------------------------------------------------------------------

#[test]
fn european_rules_match_european_zones() {
    let zones = europe().matching_zones(utc(2026, 1, 1, 0, 0), 37);
    let names = names(&zones);
    for expected in ["Europe/Brussels", "Europe/Paris", "Europe/Berlin"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    assert!(!names.contains(&"Europe/London"));
    assert!(!names.contains(&"Africa/Lagos"), "no daylight saving there");
}

#[test]
fn matches_are_in_alphabetical_order() {
    let zones = europe().matching_zones(utc(2026, 1, 1, 0, 0), 37);
    let names = names(&zones);
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn a_constant_offset_matches_zones_without_daylight_saving() {
    let names = names(&constant(5 * HOUR + 30 * 60).matching_zones(utc(2026, 1, 1, 0, 0), 37))
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert!(names.contains(&"Asia/Kolkata".to_owned()), "{names:?}");
}

#[test]
fn an_offset_no_zone_has_matches_nothing() {
    assert!(
        constant(HOUR + 17 * 60)
            .matching_zones(utc(2026, 1, 1, 0, 0), 37)
            .is_empty()
    );
}

/// The window matters: Europe/Moscow had summer time until 2011, so a constant +03:00 matches
/// it from 2026 on but not from 2008.
#[test]
fn the_window_starts_at_from() {
    let zone = constant(3 * HOUR);
    let names = |from| {
        zone.matching_zones(from, 37)
            .iter()
            .map(|tz| tz.name().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(names(utc(2026, 1, 1, 0, 0)).contains(&"Europe/Moscow".to_owned()));
    assert!(!names(utc(2008, 1, 1, 0, 0)).contains(&"Europe/Moscow".to_owned()));
}
