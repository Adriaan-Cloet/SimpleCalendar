//! The ICS time-zone cases of `docs/architecture.md` §12, numbered as in its table. Each case
//! is a feed in `fixtures/tz/`, except case 7, which is built from the Windows table.
//!
//! A case is parsed, stored like the sync engine stores it, and read back through
//! `list_occurrences`, so it checks what the views would show, not only the parsed values.

use std::collections::HashSet;

use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::America::New_York;
use chrono_tz::Europe::{Brussels, Paris};
use chrono_tz::Tz;

use super::parse::{Feed, parse_feed};
use super::windows_zones::WINDOWS_ZONES;
use crate::db::migrations::test_connection;
use crate::db::occurrences::list_occurrences;
use crate::model::{OccurrenceTime, TimeKind};
use crate::provider::RemoteEvent;
use crate::sync::store::apply_page;

const HOUR: i64 = 3600;

// ---- helpers -------------------------------------------------------------------------------

/// Rust note: `include_str!` reads the file at compile time, relative to this source file, so
/// the tests need no path at run time.
macro_rules! fixture {
    ($name:literal) => {
        parse_feed(include_str!(concat!("fixtures/tz/", $name, ".ics"))).expect($name)
    };
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .unwrap()
        .timestamp()
}

fn at(tz: Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .unwrap()
        .timestamp()
}

/// The event with this remote id.
fn event<'f>(feed: &'f Feed, remote_id: &str) -> &'f RemoteEvent {
    feed.events
        .iter()
        .find(|e| e.remote_id == remote_id)
        .unwrap_or_else(|| panic!("no event {remote_id} in {:#?}", feed.events))
}

/// The only event of a feed.
fn only(feed: &Feed) -> &RemoteEvent {
    assert_eq!(feed.events.len(), 1, "{:#?}", feed.events);
    &feed.events[0]
}

fn kind(e: &RemoteEvent) -> TimeKind {
    TimeKind::from_columns(e.start_tz.as_deref(), e.all_day)
}

fn no_warnings(feed: &Feed) {
    assert!(feed.warnings.is_empty(), "{:?}", feed.warnings);
}

fn one_warning(feed: &Feed, mentions: &str) {
    assert_eq!(feed.warnings.len(), 1, "{:?}", feed.warnings);
    assert!(
        feed.warnings[0].contains(mentions),
        "{:?} does not mention {mentions:?}",
        feed.warnings[0]
    );
}

/// What the views show for `[from, to)` in the `viewer` zone, once the feed is stored: one
/// `"start - end title"` line per occurrence, in the viewer's local time.
fn shown(feed: &Feed, from: i64, to: i64, viewer: Tz) -> Vec<String> {
    let mut conn = test_connection();
    conn.execute(
        "INSERT INTO sources (id, kind, display_name, created_at) VALUES ('s', 'ics', 'Feed', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO calendars (id, source_id, remote_id, name, access)
         VALUES (1, 's', 'feed', 'Feed', 'read')",
        [],
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    apply_page(&tx, 1, &feed.events, &[], &mut HashSet::new()).unwrap();
    tx.commit().unwrap();

    let format = |t: OccurrenceTime| match t {
        OccurrenceTime::Timed(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        OccurrenceTime::Date(d) => d.to_string(),
    };
    list_occurrences(&conn, from, to, viewer)
        .unwrap()
        .into_iter()
        .map(|o| format!("{} - {} {}", format(o.start), format(o.end), o.title))
        .collect()
}

/// June 2026, the month most cases are in.
fn june() -> (i64, i64) {
    (utc(2026, 6, 1, 0, 0), utc(2026, 7, 1, 0, 0))
}

/// Every start of a weekly series in its own zone, as `HH:MM`, for the year 2026.
fn local_starts(feed: &Feed, tz: Tz) -> Vec<String> {
    shown(feed, utc(2026, 1, 1, 0, 0), utc(2027, 1, 1, 0, 0), tz)
        .into_iter()
        .map(|line| line[11..16].to_owned())
        .collect()
}

// ---- 1 to 5: names that are, or contain, an IANA name --------------------------------------

#[test]
fn case_01_iana_name() {
    let feed = fixture!("01-iana");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"));
    assert_eq!(e.start_utc, at(Brussels, 2026, 6, 1, 9, 0));
    assert_eq!(e.end_utc, at(Brussels, 2026, 6, 1, 10, 0));
}

#[test]
fn case_02_utc() {
    let feed = fixture!("02-utc");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("UTC"));
    assert_eq!(e.start_utc, utc(2026, 6, 1, 7, 0));
    let (from, to) = june();
    assert_eq!(
        shown(&feed, from, to, Brussels),
        vec!["2026-06-01 09:00 - 2026-06-01 10:00 UTC"]
    );
}

#[test]
fn case_03_all_day() {
    let feed = fixture!("03-all-day");
    no_warnings(&feed);
    for (id, days) in [("case-3-one", 1), ("case-3-three", 3)] {
        let e = event(&feed, id);
        assert!(e.all_day);
        assert_eq!(kind(e), TimeKind::AllDay);
        assert_eq!(e.end_utc - e.start_utc, days * 24 * HOUR, "{id}");
    }
    let shown = shown(
        &feed,
        utc(2026, 12, 1, 0, 0),
        utc(2027, 1, 1, 0, 0),
        New_York,
    );
    assert_eq!(
        shown,
        vec![
            "2026-12-25 - 2026-12-26 One day",
            "2026-12-28 - 2026-12-31 Three days",
        ]
    );
}

#[test]
fn case_04_quoted_name() {
    let feed = fixture!("04-quoted");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"));
    assert_eq!(e.start_utc, at(Brussels, 2026, 6, 1, 9, 0));
}

#[test]
fn case_05_name_with_a_prefix() {
    let feed = fixture!("05-suffix");
    no_warnings(&feed);
    for id in ["case-5-freeassociation", "case-5-mozilla"] {
        let e = event(&feed, id);
        assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"), "{id}");
        assert_eq!(e.start_utc, at(Brussels, 2026, 6, 1, 9, 0), "{id}");
    }
}

// ---- 6 to 8: Windows names -----------------------------------------------------------------

#[test]
fn case_06_windows_name_series_across_both_changes() {
    let feed = fixture!("06-windows");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Paris"));
    assert_eq!(e.start_utc, at(Paris, 2026, 3, 2, 9, 0));
    let starts = local_starts(&feed, Paris);
    assert_eq!(starts.len(), 40);
    assert!(starts.iter().all(|s| s == "09:00"), "{starts:?}");
}

#[test]
fn case_07_every_windows_name_resolves_to_a_known_zone() {
    assert!(WINDOWS_ZONES.len() > 100);
    let events: String = WINDOWS_ZONES
        .iter()
        .enumerate()
        .map(|(i, (windows, _))| {
            format!("BEGIN:VEVENT\nUID:w{i}\nDTSTART;TZID={windows}:20260601T090000\nEND:VEVENT\n")
        })
        .collect();
    let feed = parse_feed(&format!(
        "BEGIN:VCALENDAR\nVERSION:2.0\n{events}END:VCALENDAR\n"
    ))
    .unwrap();
    no_warnings(&feed);
    for (i, (windows, iana)) in WINDOWS_ZONES.iter().enumerate() {
        let tz: Tz = iana
            .parse()
            .unwrap_or_else(|_| panic!("{windows}: chrono-tz does not know {iana}"));
        let e = event(&feed, &format!("w{i}"));
        assert_eq!(e.start_tz.as_deref(), Some(*iana), "{windows}");
        assert_eq!(
            e.start_utc,
            crate::recurrence::resolve_local(
                tz,
                NaiveDate::from_ymd_opt(2026, 6, 1)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap()
            )
            .timestamp(),
            "{windows}"
        );
    }
}

#[test]
fn case_08_windows_name_in_another_letter_case() {
    let feed = fixture!("08-windows-case");
    no_warnings(&feed);
    let lower = event(&feed, "case-8-lower");
    assert_eq!(lower.start_tz.as_deref(), Some("Europe/Paris"));
    assert_eq!(lower.start_utc, at(Paris, 2026, 6, 1, 9, 0));
    let upper = event(&feed, "case-8-upper");
    assert_eq!(upper.start_tz.as_deref(), Some("Europe/Paris"));
    assert_eq!(upper.start_utc, at(Paris, 2026, 12, 1, 9, 0));
}

// ---- 9 to 14: zones the feed defines -------------------------------------------------------

#[test]
fn case_09_display_name_with_european_rules() {
    let feed = fixture!("09-display-name");
    no_warnings(&feed);
    let e = only(&feed);
    let TimeKind::Zoned(tz) = kind(e) else {
        panic!("not matched to a zone: {:?}", e.start_tz);
    };
    // Any matching zone will do, as long as it behaves like Paris over the compared period.
    for month in 1..=12 {
        let instant = utc(2027, month, 1, 12, 0);
        let offset = |tz: Tz| {
            Utc.timestamp_opt(instant, 0)
                .unwrap()
                .with_timezone(&tz)
                .format("%:z")
                .to_string()
        };
        assert_eq!(offset(tz), offset(Paris), "{} in month {month}", tz.name());
    }
    assert_eq!(e.start_utc, at(Paris, 2026, 3, 2, 9, 0));
    let starts = local_starts(&feed, Paris);
    assert_eq!(starts.len(), 40);
    assert!(starts.iter().all(|s| s == "09:00"), "{starts:?}");
}

#[test]
fn case_10_custom_zone_at_a_constant_offset() {
    let feed = fixture!("10-custom-constant");
    no_warnings(&feed);
    let e = only(&feed);
    let TimeKind::Zoned(tz) = kind(e) else {
        panic!("not matched to a zone: {:?}", e.start_tz);
    };
    let offset = Utc
        .timestamp_opt(e.start_utc, 0)
        .unwrap()
        .with_timezone(&tz)
        .format("%:z")
        .to_string();
    assert_eq!(offset, "+05:30", "{}", tz.name());
    assert_eq!(e.start_utc, utc(2026, 6, 1, 3, 30));
}

#[test]
fn case_11_location_hint() {
    let feed = fixture!("11-lic-location");
    no_warnings(&feed);
    let e = only(&feed);
    // The block says a constant -05:00, which zones without daylight saving would match. The
    // hint names New York, which is -04:00 in June.
    assert_eq!(e.start_tz.as_deref(), Some("America/New_York"));
    assert_eq!(e.start_utc, at(New_York, 2026, 6, 1, 9, 0));
}

#[test]
fn case_12_zone_that_matches_nothing() {
    let feed = fixture!("12-unmatched");
    one_warning(&feed, "Strange");
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("+01:17"));
    assert_eq!(e.start_utc, utc(2026, 6, 1, 7, 43));
    assert_eq!(e.end_utc, utc(2026, 6, 1, 8, 43));
    let (from, to) = june();
    assert_eq!(
        shown(&feed, from, to, Brussels),
        vec!["2026-06-01 09:43 - 2026-06-01 10:43 No zone matches"]
    );
}

#[test]
fn case_13_feed_zone_among_the_matches() {
    let feed = fixture!("13-xwr-among-candidates");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Amsterdam"));
    assert_eq!(e.start_utc, at(Brussels, 2026, 6, 1, 9, 0));
}

#[test]
fn case_14_the_name_wins_over_a_definition() {
    let feed = fixture!("14-name-wins");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"));
    assert_eq!(e.start_utc, at(Brussels, 2026, 6, 1, 9, 0));
}

// ---- 15 to 19: unknown zones and floating times --------------------------------------------

#[test]
fn case_15_unknown_zone_falls_back_to_the_feed_zone() {
    let feed = fixture!("15-unknown-xwr");
    one_warning(&feed, "Mars Standard Time");
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("America/New_York"));
    assert_eq!(e.start_utc, at(New_York, 2026, 6, 1, 9, 0));
}

#[test]
fn case_16_unknown_zone_without_feed_zone_is_floating() {
    let feed = fixture!("16-unknown");
    one_warning(&feed, "Mars Standard Time");
    let e = only(&feed);
    assert_eq!(kind(e), TimeKind::Floating);
    assert_eq!(e.start_utc, utc(2026, 6, 1, 9, 0));
}

#[test]
fn case_17_floating_time_is_the_same_clock_time_everywhere() {
    let feed = fixture!("17-floating");
    no_warnings(&feed);
    assert_eq!(kind(only(&feed)), TimeKind::Floating);
    let (from, to) = june();
    for viewer in [Brussels, New_York] {
        assert_eq!(
            shown(&feed, from, to, viewer),
            vec!["2026-06-01 09:00 - 2026-06-01 10:00 Floating"],
            "{}",
            viewer.name()
        );
    }
}

#[test]
fn case_18_time_without_zone_takes_the_feed_zone() {
    let feed = fixture!("18-floating-xwr");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"));
    assert_eq!(e.start_utc, at(Brussels, 2026, 6, 1, 9, 0));
    let (from, to) = june();
    assert_eq!(
        shown(&feed, from, to, New_York),
        vec!["2026-06-01 03:00 - 2026-06-01 04:00 No zone, feed zone set"]
    );
}

#[test]
fn case_19_floating_series_across_a_daylight_saving_change() {
    let feed = fixture!("19-floating-series");
    no_warnings(&feed);
    let shown = shown(
        &feed,
        utc(2026, 3, 20, 0, 0),
        utc(2026, 4, 10, 0, 0),
        Brussels,
    );
    assert_eq!(shown.len(), 7, "{shown:?}");
    for (day, line) in (26..=31).chain(1..=1).zip(&shown) {
        assert!(line.contains(&format!("-{day:02} 09:00 - ")), "{line}");
    }
}

// ---- 20 to 22: one event in several forms --------------------------------------------------

#[test]
fn case_20_exclusion_and_exception_in_other_forms() {
    let feed = fixture!("20-mixed");
    no_warnings(&feed);
    let (from, to) = june();
    assert_eq!(
        shown(&feed, from, to, Brussels),
        vec![
            "2026-06-01 09:00 - 2026-06-01 10:00 Daily",
            "2026-06-03 14:00 - 2026-06-03 15:00 Moved",
            "2026-06-04 09:00 - 2026-06-04 10:00 Daily",
            "2026-06-05 09:00 - 2026-06-05 10:00 Daily",
        ]
    );
}

#[test]
fn case_21_start_and_end_in_different_zones() {
    let feed = fixture!("21-start-end-zones");
    no_warnings(&feed);
    let e = only(&feed);
    assert_eq!(e.end_utc - e.start_utc, 6 * HOUR);
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"));
    assert_eq!(e.end_tz.as_deref(), Some("America/New_York"));
}

#[test]
fn case_22_exception_before_its_master() {
    let feed = fixture!("22-exception-first");
    no_warnings(&feed);
    let (from, to) = june();
    assert_eq!(
        shown(&feed, from, to, Brussels),
        vec![
            "2026-06-01 09:00 - 2026-06-01 10:00 Daily",
            "2026-06-02 11:00 - 2026-06-02 12:00 Moved",
            "2026-06-03 09:00 - 2026-06-03 10:00 Daily",
        ]
    );
}
