use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::Europe::Brussels;

use super::*;
use crate::model::{EventStatus, TimeKind};
use crate::provider::SeriesRole;
use crate::recurrence::{self, Series};

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

// ---- helpers -------------------------------------------------------------------------------

/// A feed around the given `VEVENT` lines, with CRLF line ends as RFC 5545 asks.
fn feed(body: &str) -> String {
    let lines: Vec<&str> = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Test//EN"]
        .into_iter()
        .chain(body.lines().map(str::trim).filter(|l| !l.is_empty()))
        .chain(["END:VCALENDAR"])
        .collect();
    lines.join("\r\n") + "\r\n"
}

/// One event from a feed with one `VEVENT` holding `props`.
fn one(props: &str) -> RemoteEvent {
    let events = parse_feed(&feed(&format!("BEGIN:VEVENT\n{props}\nEND:VEVENT")))
        .unwrap()
        .events;
    assert_eq!(events.len(), 1, "{events:#?}");
    events.into_iter().next().unwrap()
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .unwrap()
        .timestamp()
}

fn brussels(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Brussels
        .with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .unwrap()
        .timestamp()
}

/// A wall-clock time written as if it were UTC: how floating and all-day values are stored.
fn wall(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    utc(y, mo, d, h, mi)
}

// ---- times ---------------------------------------------------------------------------------

#[test]
fn utc_time_is_zoned_in_utc() {
    let e = one("UID:a\nDTSTART:20260601T090000Z\nDTEND:20260601T100000Z\nSUMMARY:A");
    assert_eq!(
        (e.start_utc, e.end_utc),
        (utc(2026, 6, 1, 9, 0), utc(2026, 6, 1, 10, 0))
    );
    assert_eq!(e.start_tz.as_deref(), Some("UTC"));
    assert_eq!(e.end_tz.as_deref(), Some("UTC"));
    assert!(!e.all_day);
}

#[test]
fn iana_tzid_is_zoned_in_that_zone() {
    let e = one("UID:a\nDTSTART;TZID=Europe/Brussels:20260601T090000\n\
         DTEND;TZID=Europe/Brussels:20260601T100000");
    assert_eq!(
        (e.start_utc, e.end_utc),
        (brussels(2026, 6, 1, 9, 0), brussels(2026, 6, 1, 10, 0))
    );
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"));
    assert_eq!(e.end_tz.as_deref(), Some("Europe/Brussels"));
}

#[test]
fn quoted_tzid_is_read_without_quotes() {
    let e = one("UID:a\nDTSTART;TZID=\"Europe/Brussels\":20260601T090000");
    assert_eq!(e.start_utc, brussels(2026, 6, 1, 9, 0));
    assert_eq!(e.start_tz.as_deref(), Some("Europe/Brussels"));
}

#[test]
fn start_and_end_keep_their_own_zones() {
    let e = one("UID:a\nDTSTART;TZID=Europe/Brussels:20260601T090000\n\
         DTEND;TZID=America/New_York:20260601T090000");
    assert_eq!(e.end_utc - e.start_utc, 6 * HOUR);
    assert_eq!(e.end_tz.as_deref(), Some("America/New_York"));
}

#[test]
fn time_without_zone_is_floating() {
    let e = one("UID:a\nDTSTART:20260601T090000\nDTEND:20260601T100000");
    assert_eq!(
        (e.start_utc, e.end_utc),
        (wall(2026, 6, 1, 9, 0), wall(2026, 6, 1, 10, 0))
    );
    assert_eq!((e.start_tz, e.end_tz), (None, None));
    assert!(!e.all_day);
}

/// Until #8 resolves Windows names and `VTIMEZONE`s, an unknown zone keeps the event visible at
/// its wall-clock time.
#[test]
fn unknown_tzid_is_floating_for_now() {
    let e = one("UID:a\nDTSTART;TZID=Nowhere Standard Time:20260601T090000");
    assert_eq!(e.start_utc, wall(2026, 6, 1, 9, 0));
    assert_eq!(e.start_tz, None);
}

#[test]
fn all_day_event_without_end_lasts_one_day() {
    let e = one("UID:a\nDTSTART;VALUE=DATE:20261225");
    assert!(e.all_day);
    assert_eq!(
        (e.start_utc, e.end_utc),
        (wall(2026, 12, 25, 0, 0), wall(2026, 12, 26, 0, 0))
    );
    assert_eq!((e.start_tz, e.end_tz), (None, None));
}

#[test]
fn all_day_event_end_is_exclusive() {
    let e = one("UID:a\nDTSTART;VALUE=DATE:20261224\nDTEND;VALUE=DATE:20261227");
    assert_eq!(e.end_utc - e.start_utc, 3 * DAY);
}

#[test]
fn date_without_value_parameter_is_all_day() {
    let e = one("UID:a\nDTSTART:20261225");
    assert!(e.all_day);
    assert_eq!(e.start_utc, wall(2026, 12, 25, 0, 0));
}

#[test]
fn duration_gives_the_end() {
    let e = one("UID:a\nDTSTART:20260601T090000Z\nDURATION:PT1H30M");
    assert_eq!(e.end_utc - e.start_utc, 90 * 60);
    let e = one("UID:a\nDTSTART;VALUE=DATE:20261224\nDURATION:P2D");
    assert_eq!(e.end_utc - e.start_utc, 2 * DAY);
    let e = one("UID:a\nDTSTART:20260601T090000Z\nDURATION:P1W");
    assert_eq!(e.end_utc - e.start_utc, 7 * DAY);
}

/// RFC 5545 §3.3.6: days are nominal. A one-day event over the spring-forward night ends at
/// the same wall-clock time, 23 hours later.
#[test]
fn day_durations_follow_the_wall_clock() {
    let e = one("UID:a\nDTSTART;TZID=Europe/Brussels:20260328T120000\nDURATION:P1D");
    assert_eq!(e.end_utc, brussels(2026, 3, 29, 12, 0));
    assert_eq!(e.end_utc - e.start_utc, 23 * HOUR);
}

#[test]
fn timed_event_without_end_or_duration_has_no_length() {
    let e = one("UID:a\nDTSTART:20260601T090000Z");
    assert_eq!(e.end_utc, e.start_utc);
}

#[test]
fn an_end_before_the_start_is_moved_to_the_start() {
    let e = one("UID:a\nDTSTART:20260601T090000Z\nDTEND:20260601T080000Z");
    assert_eq!(e.end_utc, e.start_utc);
}

// ---- other properties ----------------------------------------------------------------------

#[test]
fn text_is_unescaped() {
    let e = one(
        "UID:a\nDTSTART:20260601T090000Z\nSUMMARY:Lunch\\, then a walk\n\
         DESCRIPTION:Line one\\nLine two\\; still two\nLOCATION:Room 1\\, Building B",
    );
    assert_eq!(e.title, "Lunch, then a walk");
    assert_eq!(
        e.description.as_deref(),
        Some("Line one\nLine two; still two")
    );
    assert_eq!(e.location.as_deref(), Some("Room 1, Building B"));
}

#[test]
fn missing_summary_is_an_empty_title() {
    let e = one("UID:a\nDTSTART:20260601T090000Z");
    assert_eq!(e.title, "");
    assert_eq!((e.description, e.location), (None, None));
}

#[test]
fn status_is_mapped() {
    let status = |s: &str| one(&format!("UID:a\nDTSTART:20260601T090000Z\nSTATUS:{s}")).status;
    assert_eq!(status("CANCELLED"), EventStatus::Cancelled);
    assert_eq!(status("TENTATIVE"), EventStatus::Tentative);
    assert_eq!(status("CONFIRMED"), EventStatus::Confirmed);
    assert_eq!(
        one("UID:a\nDTSTART:20260601T090000Z").status,
        EventStatus::Confirmed
    );
}

#[test]
fn uid_and_last_modified_are_kept() {
    let e = one("UID:abc@example.com\nDTSTART:20260601T090000Z\nLAST-MODIFIED:20260501T120000Z");
    assert_eq!(e.remote_id, "abc@example.com");
    assert_eq!(e.ical_uid.as_deref(), Some("abc@example.com"));
    assert_eq!(e.remote_updated_at, Some(utc(2026, 5, 1, 12, 0)));
    assert_eq!(e.series, SeriesRole::Single);
}

#[test]
fn alarms_are_ignored() {
    let e = one("UID:a\nDTSTART:20260601T090000Z\nSUMMARY:Event\n\
         BEGIN:VALARM\nACTION:DISPLAY\nTRIGGER:-PT15M\nDESCRIPTION:Alarm text\nEND:VALARM");
    assert_eq!(e.title, "Event");
    assert_eq!(e.description, None);
}

// ---- series --------------------------------------------------------------------------------

#[test]
fn master_keeps_its_recurrence_lines() {
    let e = one("UID:m\nDTSTART;TZID=Europe/Brussels:20260601T090000\n\
         RRULE:FREQ=WEEKLY;COUNT=5\n\
         EXDATE;TZID=Europe/Brussels:20260608T090000\n\
         RDATE:20260603T070000Z");
    assert_eq!(e.remote_id, "m");
    let SeriesRole::Master { recurrence } = &e.series else {
        panic!("not a master: {:?}", e.series);
    };
    assert_eq!(
        recurrence.lines().collect::<Vec<_>>(),
        vec![
            "RRULE:FREQ=WEEKLY;COUNT=5",
            "EXDATE;TZID=Europe/Brussels:20260608T090000",
            "RDATE:20260603T070000Z",
        ]
    );
}

#[test]
fn a_quoted_tzid_in_recurrence_lines_is_written_plainly() {
    let e = one(
        "UID:m\nDTSTART;TZID=Europe/Brussels:20260601T090000\nRRULE:FREQ=DAILY\n\
         EXDATE;TZID=\"Europe/Brussels\":20260602T090000,20260603T090000",
    );
    let SeriesRole::Master { recurrence } = e.series else {
        panic!("not a master");
    };
    assert!(
        recurrence.contains("EXDATE;TZID=Europe/Brussels:20260602T090000,20260603T090000"),
        "{recurrence}"
    );
}

/// What the parser stores, the recurrence module must be able to expand.
#[test]
fn a_parsed_master_expands() {
    let e = one("UID:m\nDTSTART;TZID=Europe/Brussels:20260601T090000\n\
         DTEND;TZID=Europe/Brussels:20260601T100000\n\
         RRULE:FREQ=DAILY;COUNT=4\n\
         EXDATE;TZID=Europe/Brussels:20260602T090000");
    let SeriesRole::Master { recurrence } = &e.series else {
        panic!("not a master");
    };
    let series = Series {
        start: e.start_utc,
        duration: e.end_utc - e.start_utc,
        kind: TimeKind::from_columns(e.start_tz.as_deref(), e.all_day),
        recurrence,
    };
    let starts = recurrence::expand(&series, utc(2026, 5, 1, 0, 0), utc(2026, 7, 1, 0, 0)).unwrap();
    assert_eq!(
        starts,
        vec![
            brussels(2026, 6, 1, 9, 0),
            brussels(2026, 6, 3, 9, 0),
            brussels(2026, 6, 4, 9, 0),
        ]
    );
}

#[test]
fn exception_names_its_master_and_the_occurrence_it_replaces() {
    let e = one(
        "UID:m\nRECURRENCE-ID;TZID=Europe/Brussels:20260603T090000\n\
         DTSTART;TZID=Europe/Brussels:20260603T110000\nSUMMARY:Moved",
    );
    let original = brussels(2026, 6, 3, 9, 0);
    assert_eq!(
        e.series,
        SeriesRole::Exception {
            master_remote_id: "m".into(),
            original_start_utc: original,
        }
    );
    assert_eq!(e.remote_id, format!("m#{original}"));
    assert_eq!(e.ical_uid.as_deref(), Some("m"));
    assert_eq!(e.start_utc, brussels(2026, 6, 3, 11, 0));
}

#[test]
fn all_day_exception_uses_the_stored_date() {
    let e = one("UID:m\nRECURRENCE-ID;VALUE=DATE:20261225\nDTSTART;VALUE=DATE:20261226");
    let original = NaiveDate::from_ymd_opt(2026, 12, 25)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    assert_eq!(
        e.series,
        SeriesRole::Exception {
            master_remote_id: "m".into(),
            original_start_utc: original,
        }
    );
}

#[test]
fn components_may_come_in_any_order() {
    let events = parse_feed(&feed(
        "BEGIN:VEVENT\nUID:m\nRECURRENCE-ID:20260603T090000Z\nDTSTART:20260603T110000Z\nEND:VEVENT\n\
         BEGIN:VEVENT\nUID:m\nDTSTART:20260601T090000Z\nRRULE:FREQ=DAILY\nEND:VEVENT\n\
         BEGIN:VEVENT\nUID:s\nDTSTART:20260601T120000Z\nEND:VEVENT",
    ))
    .unwrap()
    .events;
    let mut ids: Vec<_> = events.iter().map(|e| e.remote_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["m", &format!("m#{}", utc(2026, 6, 3, 9, 0)), "s"]);
}

#[test]
fn an_exdate_without_rule_is_a_single_event() {
    let e = one("UID:a\nDTSTART:20260601T090000Z\nEXDATE:20260601T090000Z");
    assert_eq!(e.series, SeriesRole::Single);
}

// ---- the feed as a whole -------------------------------------------------------------------

#[test]
fn folded_lines_and_bare_newlines_are_read() {
    let text = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:a\nDTSTART:20260601T090000Z\n\
                SUMMARY:A long\n  title\nEND:VEVENT\nEND:VCALENDAR\n";
    let events = parse_feed(text).unwrap().events;
    assert_eq!(events[0].title, "A long title");
}

#[test]
fn todos_journals_and_time_zones_are_not_events() {
    let events = parse_feed(&feed(
        "BEGIN:VTIMEZONE\nTZID:Custom\nBEGIN:STANDARD\nDTSTART:19701025T030000\n\
         TZOFFSETFROM:+0200\nTZOFFSETTO:+0100\nEND:STANDARD\nEND:VTIMEZONE\n\
         BEGIN:VTODO\nUID:t\nDTSTART:20260601T090000Z\nEND:VTODO\n\
         BEGIN:VJOURNAL\nUID:j\nDTSTART:20260601T090000Z\nEND:VJOURNAL\n\
         BEGIN:VEVENT\nUID:e\nDTSTART:20260601T090000Z\nEND:VEVENT",
    ))
    .unwrap()
    .events;
    let ids: Vec<_> = events.iter().map(|e| e.remote_id.as_str()).collect();
    assert_eq!(ids, vec!["e"]);
}

#[test]
fn broken_events_are_skipped_and_the_rest_kept() {
    let events = parse_feed(&feed(
        "BEGIN:VEVENT\nDTSTART:20260601T090000Z\nSUMMARY:No UID\nEND:VEVENT\n\
         BEGIN:VEVENT\nUID:no-start\nSUMMARY:No start\nEND:VEVENT\n\
         BEGIN:VEVENT\nUID:bad-start\nDTSTART:tomorrow\nEND:VEVENT\n\
         BEGIN:VEVENT\nUID:good\nDTSTART:20260601T090000Z\nEND:VEVENT",
    ))
    .unwrap()
    .events;
    let ids: Vec<_> = events.iter().map(|e| e.remote_id.as_str()).collect();
    assert_eq!(ids, vec!["good"]);
}

#[test]
fn an_empty_calendar_has_no_events() {
    assert!(parse_feed(&feed("")).unwrap().events.is_empty());
}

#[test]
fn html_is_not_a_feed() {
    assert!(parse_feed("<!doctype html><html><body>Sign in</body></html>").is_err());
    assert!(parse_feed("").is_err());
}
