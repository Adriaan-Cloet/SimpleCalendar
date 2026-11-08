//! Expansion and conversion on their own, without the database. 2026 dates: Europe changes to
//! summer time on 29 March and back on 25 October, the US on 8 March and 1 November.

use chrono::{FixedOffset, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use chrono_tz::Europe::Brussels;
use chrono_tz::Pacific::Auckland;

use super::*;

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

// ---- helpers -------------------------------------------------------------------------------

/// A UTC instant, or a wall-clock value stored as if it were UTC.
fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .unwrap()
        .timestamp()
}

/// The instant of an unambiguous local time.
fn local(tz: Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .unwrap()
        .timestamp()
}

fn series(start: i64, duration: i64, kind: TimeKind, recurrence: &str) -> Series<'_> {
    Series {
        start,
        duration,
        kind,
        recurrence,
    }
}

fn brussels(start: i64, recurrence: &str) -> Series<'_> {
    series(start, HOUR, TimeKind::Zoned(Brussels), recurrence)
}

/// Stored instants as wall-clock times in `tz`, for readable assertions.
fn wall(tz: Tz, stored: &[i64]) -> Vec<String> {
    stored
        .iter()
        .map(|s| {
            DateTime::from_timestamp(*s, 0)
                .unwrap()
                .with_timezone(&tz)
                .format("%Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .collect()
}

/// Stored wall-clock values (floating and all-day) as they were written.
fn as_written(stored: &[i64]) -> Vec<String> {
    wall(chrono_tz::UTC, stored)
        .into_iter()
        .map(|s| s.trim_end_matches(" +00:00").to_string())
        .collect()
}

fn timed(tz: Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> OccurrenceTime {
    OccurrenceTime::Timed(
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
            .fixed_offset(),
    )
}

fn date(y: i32, mo: u32, d: u32) -> OccurrenceTime {
    OccurrenceTime::Date(NaiveDate::from_ymd_opt(y, mo, d).unwrap())
}

// ---- daylight saving -----------------------------------------------------------------------

#[test]
fn weekly_series_keeps_local_time_across_spring_forward() {
    // Monday 23 March, before the change; 30 March and 6 April are after it.
    let s = brussels(local(Brussels, 2026, 3, 23, 9, 0), "RRULE:FREQ=WEEKLY");
    let got = expand(&s, utc(2026, 3, 23, 0, 0), utc(2026, 4, 7, 0, 0)).unwrap();
    assert_eq!(
        got,
        vec![
            utc(2026, 3, 23, 8, 0),
            utc(2026, 3, 30, 7, 0),
            utc(2026, 4, 6, 7, 0)
        ]
    );
    assert_eq!(
        wall(Brussels, &got),
        [
            "2026-03-23 09:00 +01:00",
            "2026-03-30 09:00 +02:00",
            "2026-04-06 09:00 +02:00"
        ]
    );
}

#[test]
fn weekly_series_keeps_local_time_across_fall_back() {
    let s = brussels(local(Brussels, 2026, 10, 19, 9, 0), "RRULE:FREQ=WEEKLY");
    let got = expand(&s, utc(2026, 10, 19, 0, 0), utc(2026, 10, 27, 0, 0)).unwrap();
    assert_eq!(got, vec![utc(2026, 10, 19, 7, 0), utc(2026, 10, 26, 8, 0)]);
}

#[test]
fn time_skipped_by_spring_forward_moves_to_after_the_gap() {
    // 02:30 does not exist on 29 March in Brussels: 02:00 CET jumps to 03:00 CEST.
    let s = brussels(local(Brussels, 2026, 3, 28, 2, 30), "RRULE:FREQ=DAILY");
    let got = expand(&s, utc(2026, 3, 28, 0, 0), utc(2026, 3, 31, 0, 0)).unwrap();
    assert_eq!(
        wall(Brussels, &got),
        [
            "2026-03-28 02:30 +01:00",
            "2026-03-29 03:30 +02:00",
            "2026-03-30 02:30 +02:00"
        ]
    );
}

#[test]
fn time_repeated_by_fall_back_occurs_once_at_the_first_instant() {
    // 02:30 exists twice on 25 October in Brussels: first in CEST, then in CET.
    let s = brussels(local(Brussels, 2026, 10, 24, 2, 30), "RRULE:FREQ=DAILY");
    let got = expand(&s, utc(2026, 10, 24, 0, 0), utc(2026, 10, 27, 0, 0)).unwrap();
    assert_eq!(
        wall(Brussels, &got),
        [
            "2026-10-24 02:30 +02:00",
            "2026-10-25 02:30 +02:00",
            "2026-10-26 02:30 +01:00"
        ]
    );
}

#[test]
fn resolve_local_follows_rfc_5545_for_gaps_and_repeats() {
    let at = |d: u32, mo: u32, h: u32, mi: u32| {
        NaiveDate::from_ymd_opt(2026, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    };
    // Normal time.
    assert_eq!(
        resolve_local(Brussels, at(15, 6, 9, 0)).timestamp(),
        utc(2026, 6, 15, 7, 0)
    );
    // Gap: read with the offset from before it (+01:00), so 01:30 UTC = 03:30 CEST.
    assert_eq!(
        resolve_local(Brussels, at(29, 3, 2, 30)).timestamp(),
        utc(2026, 3, 29, 1, 30)
    );
    // Repeat: the first of the two, still in CEST.
    assert_eq!(
        resolve_local(Brussels, at(25, 10, 2, 30)).timestamp(),
        utc(2026, 10, 25, 0, 30)
    );
}

// ---- all-day and floating ------------------------------------------------------------------

#[test]
fn all_day_series_expands_on_dates() {
    let s = series(
        utc(2026, 12, 21, 0, 0),
        DAY,
        TimeKind::AllDay,
        "RRULE:FREQ=WEEKLY;COUNT=3",
    );
    let got = expand(&s, utc(2026, 12, 1, 0, 0), utc(2027, 2, 1, 0, 0)).unwrap();
    assert_eq!(
        as_written(&got),
        ["2026-12-21 00:00", "2026-12-28 00:00", "2027-01-04 00:00"]
    );
}

#[test]
fn all_day_values_are_the_same_date_for_every_viewer() {
    let start = utc(2026, 12, 25, 0, 0);
    for viewer in [New_York, Auckland, Brussels] {
        assert_eq!(
            to_viewer(start, TimeKind::AllDay, viewer),
            date(2026, 12, 25)
        );
        assert_eq!(
            to_viewer(start + DAY, TimeKind::AllDay, viewer),
            date(2026, 12, 26)
        );
    }
}

#[test]
fn floating_series_keeps_wall_clock_without_dst_adjustment() {
    let s = series(
        utc(2026, 3, 27, 9, 0),
        HOUR,
        TimeKind::Floating,
        "RRULE:FREQ=DAILY;COUNT=4",
    );
    let got = expand(&s, utc(2026, 3, 1, 0, 0), utc(2026, 4, 30, 0, 0)).unwrap();
    assert_eq!(
        as_written(&got),
        [
            "2026-03-27 09:00",
            "2026-03-28 09:00",
            "2026-03-29 09:00",
            "2026-03-30 09:00"
        ]
    );
}

#[test]
fn floating_value_shows_at_its_wall_clock_time_for_every_viewer() {
    let stored = utc(2026, 6, 15, 9, 0);
    assert_eq!(
        to_viewer(stored, TimeKind::Floating, New_York),
        timed(New_York, 2026, 6, 15, 9, 0)
    );
    assert_eq!(
        to_viewer(stored, TimeKind::Floating, Auckland),
        timed(Auckland, 2026, 6, 15, 9, 0)
    );
}

#[test]
fn floating_value_in_a_gap_of_the_viewer_zone_moves_after_the_gap() {
    let stored = utc(2026, 3, 29, 2, 30);
    assert_eq!(
        to_viewer(stored, TimeKind::Floating, Brussels),
        timed(Brussels, 2026, 3, 29, 3, 30)
    );
}

// ---- viewer in another zone ----------------------------------------------------------------

#[test]
fn zoned_value_keeps_its_instant_in_the_viewer_zone() {
    // Between 8 and 29 March New York is already on summer time and Brussels is not, so a
    // 09:00 Brussels meeting is at 04:00 in New York instead of the usual 03:00.
    let kind = TimeKind::Zoned(Brussels);
    let cases = [
        (local(Brussels, 2026, 3, 2, 9, 0), (3, 2, 3)),
        (local(Brussels, 2026, 3, 16, 9, 0), (3, 16, 4)),
        (local(Brussels, 2026, 3, 30, 9, 0), (3, 30, 3)),
    ];
    for (stored, (mo, d, h)) in cases {
        assert_eq!(
            to_viewer(stored, kind, New_York),
            timed(New_York, 2026, mo, d, h, 0)
        );
    }
}

// ---- end of a series -----------------------------------------------------------------------

#[test]
fn until_in_utc_is_inclusive() {
    // 3 June 09:00 CEST is 07:00 UTC, exactly the UNTIL value.
    let s = brussels(
        local(Brussels, 2026, 6, 1, 9, 0),
        "RRULE:FREQ=DAILY;UNTIL=20260603T070000Z",
    );
    let got = expand(&s, utc(2026, 5, 1, 0, 0), utc(2026, 7, 1, 0, 0)).unwrap();
    assert_eq!(got.len(), 3);
}

#[test]
fn until_as_a_date_includes_that_whole_day() {
    let s = brussels(
        local(Brussels, 2026, 6, 1, 9, 0),
        "RRULE:FREQ=DAILY;UNTIL=20260603",
    );
    let got = expand(&s, utc(2026, 5, 1, 0, 0), utc(2026, 7, 1, 0, 0)).unwrap();
    assert_eq!(got.len(), 3);
}

#[test]
fn until_as_a_local_time_is_read_in_the_series_zone() {
    let s = brussels(
        local(Brussels, 2026, 6, 1, 9, 0),
        "RRULE:FREQ=DAILY;UNTIL=20260603T090000",
    );
    let got = expand(&s, utc(2026, 5, 1, 0, 0), utc(2026, 7, 1, 0, 0)).unwrap();
    assert_eq!(got.len(), 3);
}

#[test]
fn until_on_an_all_day_series() {
    let s = series(
        utc(2026, 6, 1, 0, 0),
        DAY,
        TimeKind::AllDay,
        "RRULE:FREQ=DAILY;UNTIL=20260603",
    );
    let got = expand(&s, utc(2026, 5, 1, 0, 0), utc(2026, 7, 1, 0, 0)).unwrap();
    assert_eq!(
        as_written(&got),
        ["2026-06-01 00:00", "2026-06-02 00:00", "2026-06-03 00:00"]
    );
}

#[test]
fn count_limits_the_series() {
    let s = brussels(
        local(Brussels, 2026, 6, 1, 9, 0),
        "RRULE:FREQ=WEEKLY;COUNT=4",
    );
    let all = expand(&s, utc(2026, 5, 1, 0, 0), utc(2026, 12, 1, 0, 0)).unwrap();
    assert_eq!(all.len(), 4);
    // A range after the last one is empty, even though the rule itself is weekly.
    let later = expand(&s, utc(2026, 7, 1, 0, 0), utc(2026, 8, 1, 0, 0)).unwrap();
    assert!(later.is_empty());
}

// ---- EXDATE and RDATE ----------------------------------------------------------------------

#[test]
fn exdate_matches_by_instant_in_every_form() {
    // Daily 09:00 Brussels, 1 to 6 June. Four exclusions in three forms; all mean 09:00 local.
    let recurrence = "RRULE:FREQ=DAILY;COUNT=6\n\
                      EXDATE:20260602T070000Z\n\
                      EXDATE;TZID=Europe/Brussels:20260603T090000\n\
                      EXDATE;TZID=America/New_York:20260604T030000,20260605T030000";
    let s = brussels(local(Brussels, 2026, 6, 1, 9, 0), recurrence);
    let got = expand(&s, utc(2026, 5, 1, 0, 0), utc(2026, 7, 1, 0, 0)).unwrap();
    assert_eq!(
        wall(Brussels, &got),
        ["2026-06-01 09:00 +02:00", "2026-06-06 09:00 +02:00"]
    );
}

#[test]
fn exdate_as_a_date_on_an_all_day_series() {
    let s = series(
        utc(2026, 12, 21, 0, 0),
        DAY,
        TimeKind::AllDay,
        "RRULE:FREQ=WEEKLY;COUNT=3\nEXDATE;VALUE=DATE:20261228",
    );
    let got = expand(&s, utc(2026, 12, 1, 0, 0), utc(2027, 2, 1, 0, 0)).unwrap();
    assert_eq!(as_written(&got), ["2026-12-21 00:00", "2027-01-04 00:00"]);
}

#[test]
fn rdate_adds_an_occurrence_and_exdate_wins_over_it() {
    let recurrence = "RRULE:FREQ=DAILY;COUNT=2\n\
                      RDATE;TZID=Europe/Brussels:20260610T140000,20260611T140000\n\
                      EXDATE;TZID=Europe/Brussels:20260611T140000";
    let s = brussels(local(Brussels, 2026, 6, 1, 9, 0), recurrence);
    let got = expand(&s, utc(2026, 5, 1, 0, 0), utc(2026, 7, 1, 0, 0)).unwrap();
    assert_eq!(
        wall(Brussels, &got),
        [
            "2026-06-01 09:00 +02:00",
            "2026-06-02 09:00 +02:00",
            "2026-06-10 14:00 +02:00"
        ]
    );
}

// ---- fixed offset --------------------------------------------------------------------------

#[test]
fn fixed_offset_series_repeats_at_the_same_offset() {
    let offset = FixedOffset::east_opt(5 * HOUR as i32 + 30 * 60).unwrap();
    // 09:00 at +05:30 is 03:30 UTC; the date-form UNTIL is read at that offset too.
    let s = series(
        utc(2026, 3, 23, 3, 30),
        HOUR,
        TimeKind::Offset(offset),
        "RRULE:FREQ=WEEKLY;UNTIL=20260406\nEXDATE:20260330T033000Z",
    );
    let got = expand(&s, utc(2026, 3, 1, 0, 0), utc(2026, 5, 1, 0, 0)).unwrap();
    assert_eq!(got, vec![utc(2026, 3, 23, 3, 30), utc(2026, 4, 6, 3, 30)]);
}

// ---- the range -----------------------------------------------------------------------------

#[test]
fn range_includes_an_occurrence_still_running_and_excludes_one_that_just_ended() {
    // Daily 23:00 to 01:00 UTC.
    let s = series(
        utc(2026, 6, 1, 23, 0),
        2 * HOUR,
        TimeKind::Zoned(chrono_tz::UTC),
        "RRULE:FREQ=DAILY",
    );
    // From midnight: the occurrence that started at 23:00 the day before is still running.
    let got = expand(&s, utc(2026, 6, 3, 0, 0), utc(2026, 6, 3, 12, 0)).unwrap();
    assert_eq!(got, vec![utc(2026, 6, 2, 23, 0)]);
    // From 01:00: that occurrence ended exactly then, and ends are exclusive.
    let got = expand(&s, utc(2026, 6, 3, 1, 0), utc(2026, 6, 3, 12, 0)).unwrap();
    assert!(got.is_empty());
    // An occurrence starting exactly at `to` is outside the range.
    let got = expand(&s, utc(2026, 6, 3, 12, 0), utc(2026, 6, 3, 23, 0)).unwrap();
    assert!(got.is_empty());
}

#[test]
fn endless_series_far_in_the_future_returns_only_the_range() {
    let s = brussels(local(Brussels, 2020, 1, 6, 9, 0), "RRULE:FREQ=DAILY");
    let got = expand(&s, utc(2040, 6, 4, 0, 0), utc(2040, 6, 11, 0, 0)).unwrap();
    assert_eq!(got.len(), 7);
    assert_eq!(wall(Brussels, &got[..1]), ["2040-06-04 09:00 +02:00"]);
}

// ---- moving the dates of a series (#68) ------------------------------------------------------

/// Moves a stored value of `kind` by `by` on its own wall clock, as "all events" does.
fn later(kind: TimeKind, by: i64) -> impl Fn(i64) -> i64 {
    move |stored| from_wall_clock(wall_clock(stored, kind) + Duration::seconds(by), kind)
}

#[test]
fn moved_dates_keep_their_wall_clock_time_across_daylight_saving() {
    let kind = TimeKind::Zoned(Brussels);
    // 20 October is summer time, 3 and 10 November winter time; 12 November is in UTC form.
    let recurrence = "RRULE:FREQ=WEEKLY\n\
                      EXDATE;TZID=Europe/Brussels:20261020T090000,20261103T090000\n\
                      RDATE:20261112T080000Z";

    let moved = move_dates(recurrence, kind, kind, later(kind, HOUR)).unwrap();

    assert_eq!(
        moved,
        "RRULE:FREQ=WEEKLY\n\
         EXDATE;TZID=Europe/Brussels:20261020T100000,20261103T100000\n\
         RDATE;TZID=Europe/Brussels:20261112T100000"
    );
}

#[test]
fn moved_dates_of_an_all_day_series_stay_dates() {
    let recurrence = "RRULE:FREQ=YEARLY\nEXDATE;VALUE=DATE:20271225\nEXDATE:20281225";

    let moved = move_dates(
        recurrence,
        TimeKind::AllDay,
        TimeKind::AllDay,
        later(TimeKind::AllDay, DAY),
    )
    .unwrap();

    assert_eq!(
        moved,
        "RRULE:FREQ=YEARLY\nEXDATE;VALUE=DATE:20271226\nEXDATE;VALUE=DATE:20281226"
    );
}

#[test]
fn moved_dates_take_the_form_of_the_new_kind() {
    // An all-day series that becomes 09:00 in Brussels, and back.
    let zoned = TimeKind::Zoned(Brussels);
    let to_nine = |stored: i64| {
        from_wall_clock(
            wall_clock(stored, TimeKind::AllDay) + Duration::hours(9),
            zoned,
        )
    };
    let timed = move_dates(
        "RRULE:FREQ=WEEKLY\nEXDATE;VALUE=DATE:20261103",
        TimeKind::AllDay,
        zoned,
        to_nine,
    )
    .unwrap();
    assert_eq!(
        timed,
        "RRULE:FREQ=WEEKLY\nEXDATE;TZID=Europe/Brussels:20261103T090000"
    );

    let to_date = |stored: i64| {
        let date = wall_clock(stored, zoned).date();
        from_wall_clock(date.and_time(NaiveTime::MIN), TimeKind::AllDay)
    };
    let dated = move_dates(&timed, zoned, TimeKind::AllDay, to_date).unwrap();
    assert_eq!(dated, "RRULE:FREQ=WEEKLY\nEXDATE;VALUE=DATE:20261103");
}

#[test]
fn moved_floating_and_offset_dates_keep_their_forms() {
    let floating = move_dates(
        "RDATE:20261103T090000\nRRULE:FREQ=DAILY",
        TimeKind::Floating,
        TimeKind::Floating,
        later(TimeKind::Floating, HOUR),
    )
    .unwrap();
    assert_eq!(floating, "RDATE:20261103T100000\nRRULE:FREQ=DAILY");

    let offset = TimeKind::Offset(FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap());
    let moved = move_dates(
        "RRULE:FREQ=DAILY\nEXDATE:20261103T033000Z",
        offset,
        offset,
        later(offset, HOUR),
    )
    .unwrap();
    assert_eq!(moved, "RRULE:FREQ=DAILY\nEXDATE:20261103T043000Z");
}

#[test]
fn a_date_that_cannot_be_read_cannot_be_moved() {
    let kind = TimeKind::Zoned(Brussels);
    assert!(
        move_dates(
            "RRULE:FREQ=DAILY\nRDATE;VALUE=PERIOD:20261103T090000Z/PT1H",
            kind,
            kind,
            later(kind, HOUR),
        )
        .is_err()
    );
}

// ---- errors --------------------------------------------------------------------------------

#[test]
fn malformed_rule_is_an_error() {
    let s = brussels(utc(2026, 6, 1, 7, 0), "RRULE:FREQ=SOMETIMES");
    assert!(expand(&s, utc(2026, 6, 1, 0, 0), utc(2026, 7, 1, 0, 0)).is_err());
}

#[test]
fn recurrence_without_rrule_or_rdate_is_an_error() {
    let s = brussels(utc(2026, 6, 1, 7, 0), "EXDATE:20260602T070000Z");
    assert!(matches!(
        expand(&s, utc(2026, 6, 1, 0, 0), utc(2026, 7, 1, 0, 0)),
        Err(RecurrenceError::Empty)
    ));
}

#[test]
fn time_kind_from_columns() {
    assert_eq!(
        TimeKind::from_columns(Some("Europe/Brussels"), false),
        TimeKind::Zoned(Brussels)
    );
    assert_eq!(
        TimeKind::from_columns(Some("+05:30"), false),
        TimeKind::Offset(FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap())
    );
    assert_eq!(
        TimeKind::from_columns(Some("-03:00"), false),
        TimeKind::Offset(FixedOffset::west_opt(3 * 3600).unwrap())
    );
    assert_eq!(TimeKind::from_columns(None, false), TimeKind::Floating);
    assert_eq!(TimeKind::from_columns(None, true), TimeKind::AllDay);
    // A zone the database should never hold still shows, as floating.
    assert_eq!(
        TimeKind::from_columns(Some("Not/AZone"), false),
        TimeKind::Floating
    );
}
