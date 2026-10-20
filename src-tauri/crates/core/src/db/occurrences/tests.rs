use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::America::New_York;
use chrono_tz::Europe::Brussels;
use chrono_tz::Pacific::Auckland;
use rusqlite::{Connection, params};

use super::*;
use crate::db::migrations::test_connection;
use crate::model::OccurrenceTime;

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

// ---- helpers -------------------------------------------------------------------------------

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .unwrap()
        .timestamp()
}

fn local(tz: Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .unwrap()
        .timestamp()
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

/// A source with one visible, read-only calendar. Returns the calendar id.
fn setup(conn: &Connection) -> i64 {
    conn.execute(
        "INSERT INTO sources (id, kind, display_name, created_at) VALUES ('src-1', 'ics', 'Feed', 0)",
        [],
    )
    .unwrap();
    add_calendar(conn, "cal-1", "read", true)
}

fn add_calendar(conn: &Connection, remote_id: &str, access: &str, visible: bool) -> i64 {
    conn.execute(
        "INSERT INTO calendars (source_id, remote_id, name, color, access, visible)
         VALUES ('src-1', ?1, 'Calendar', '#336699', ?2, ?3)",
        params![remote_id, access, visible],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// One row of `events`. Start from `Ev::new` and override what a test needs.
struct Ev<'a> {
    remote_id: &'a str,
    status: &'a str,
    title: &'a str,
    start: i64,
    end: i64,
    tz: Option<&'a str>,
    all_day: bool,
    recurrence: Option<&'a str>,
    recurring_remote_id: Option<&'a str>,
    recurring_event_id: Option<i64>,
    original_start: Option<i64>,
    color: Option<&'a str>,
}

impl<'a> Ev<'a> {
    fn new(remote_id: &'a str, start: i64, end: i64, tz: Option<&'a str>) -> Ev<'a> {
        Ev {
            remote_id,
            status: "confirmed",
            title: remote_id,
            start,
            end,
            tz,
            all_day: false,
            recurrence: None,
            recurring_remote_id: None,
            recurring_event_id: None,
            original_start: None,
            color: None,
        }
    }

    /// An exception that replaces the occurrence of `master` at `original_start`.
    fn exception(self, master: (&'a str, i64), original_start: i64) -> Ev<'a> {
        Ev {
            recurring_remote_id: Some(master.0),
            recurring_event_id: Some(master.1),
            original_start: Some(original_start),
            ..self
        }
    }

    fn insert(&self, conn: &Connection, calendar_id: i64) -> i64 {
        conn.execute(
            "INSERT INTO events (calendar_id, remote_id, status, title, color, all_day,
                                 start_utc, end_utc, start_tz, end_tz, recurrence,
                                 recurring_remote_id, recurring_event_id, original_start_utc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10, ?11, ?12, ?13)",
            params![
                calendar_id,
                self.remote_id,
                self.status,
                self.title,
                self.color,
                self.all_day,
                self.start,
                self.end,
                self.tz,
                self.recurrence,
                self.recurring_remote_id,
                self.recurring_event_id,
                self.original_start,
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    }
}

/// Weekly on Mondays at 09:00 Brussels from 1 June 2026, one hour. Returns its id.
fn weekly_brussels(conn: &Connection, cal: i64) -> i64 {
    let start = local(Brussels, 2026, 6, 1, 9, 0);
    Ev {
        recurrence: Some("RRULE:FREQ=WEEKLY"),
        ..Ev::new("series", start, start + HOUR, Some("Europe/Brussels"))
    }
    .insert(conn, cal)
}

fn list(conn: &Connection, from: i64, to: i64, viewer: Tz) -> Vec<Occurrence> {
    list_occurrences(conn, from, to, viewer).unwrap()
}

fn starts(occurrences: &[Occurrence]) -> Vec<OccurrenceTime> {
    occurrences.iter().map(|o| o.start).collect()
}

fn count_events(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap()
}

// ---- single events -------------------------------------------------------------------------

#[test]
fn single_event_is_shown_in_the_viewer_zone() {
    let conn = test_connection();
    let cal = setup(&conn);
    let start = local(Brussels, 2026, 6, 15, 9, 0);
    let id = Ev::new("one", start, start + HOUR, Some("Europe/Brussels")).insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 15, 0, 0),
        utc(2026, 6, 16, 0, 0),
        New_York,
    );
    assert_eq!(
        got,
        vec![Occurrence {
            event_id: id,
            original_start: start,
            calendar_id: cal,
            title: "one".into(),
            location: None,
            color: Some("#336699".into()),
            start: timed(New_York, 2026, 6, 15, 3, 0),
            end: timed(New_York, 2026, 6, 15, 4, 0),
            editable: false,
        }]
    );
}

#[test]
fn editable_follows_calendar_write_access() {
    let conn = test_connection();
    setup(&conn);
    let cal = add_calendar(&conn, "cal-w", "write", true);
    let start = utc(2026, 6, 15, 9, 0);
    Ev::new("one", start, start + HOUR, Some("UTC")).insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 15, 0, 0),
        utc(2026, 6, 16, 0, 0),
        Brussels,
    );
    assert!(got[0].editable);
}

#[test]
fn colour_prefers_event_then_override_then_calendar() {
    let conn = test_connection();
    let cal = setup(&conn);
    let start = utc(2026, 6, 15, 9, 0);
    Ev {
        color: Some("#ff0000"),
        ..Ev::new("own", start, start + HOUR, Some("UTC"))
    }
    .insert(&conn, cal);
    Ev::new("inherits", start + HOUR, start + 2 * HOUR, Some("UTC")).insert(&conn, cal);
    conn.execute(
        "UPDATE calendars SET color_override = '#00ff00' WHERE id = ?1",
        [cal],
    )
    .unwrap();

    let got = list(
        &conn,
        utc(2026, 6, 15, 0, 0),
        utc(2026, 6, 16, 0, 0),
        Brussels,
    );
    let colours: Vec<_> = got.iter().map(|o| o.color.as_deref()).collect();
    assert_eq!(colours, [Some("#ff0000"), Some("#00ff00")]);
}

#[test]
fn floating_event_shows_at_its_wall_clock_time_for_every_viewer() {
    let conn = test_connection();
    let cal = setup(&conn);
    let stored = utc(2026, 6, 15, 9, 0);
    Ev::new("float", stored, stored + HOUR, None).insert(&conn, cal);

    for viewer in [New_York, Auckland] {
        // The viewer's own 15 June, as instants.
        let from = local(viewer, 2026, 6, 15, 0, 0);
        let got = list(&conn, from, from + DAY, viewer);
        assert_eq!(starts(&got), [timed(viewer, 2026, 6, 15, 9, 0)], "{viewer}");
    }
}

#[test]
fn all_day_event_falls_on_its_own_date_in_every_zone() {
    let conn = test_connection();
    let cal = setup(&conn);
    let stored = utc(2026, 12, 25, 0, 0);
    Ev {
        all_day: true,
        ..Ev::new("xmas", stored, stored + DAY, None)
    }
    .insert(&conn, cal);

    for viewer in [New_York, Auckland] {
        let day = |d: u32| local(viewer, 2026, 12, d, 0, 0);
        let got = list(&conn, day(25), day(26), viewer);
        assert_eq!(got.len(), 1, "{viewer}");
        assert_eq!(
            (got[0].start, got[0].end),
            (date(2026, 12, 25), date(2026, 12, 26))
        );
        // The neighbouring days in the viewer's zone do not show it.
        assert!(list(&conn, day(24), day(25), viewer).is_empty(), "{viewer}");
        assert!(list(&conn, day(26), day(27), viewer).is_empty(), "{viewer}");
    }
}

#[test]
fn range_boundaries_are_start_inclusive_and_end_exclusive() {
    let conn = test_connection();
    let cal = setup(&conn);
    let tz = Some("UTC");
    // 08:00-10:00 is still running at 09:00; 07:00-09:00 ends exactly at 09:00;
    // 12:00-13:00 starts exactly at the end of the range.
    Ev::new(
        "running",
        utc(2026, 6, 15, 8, 0),
        utc(2026, 6, 15, 10, 0),
        tz,
    )
    .insert(&conn, cal);
    Ev::new("ended", utc(2026, 6, 15, 7, 0), utc(2026, 6, 15, 9, 0), tz).insert(&conn, cal);
    Ev::new(
        "later",
        utc(2026, 6, 15, 12, 0),
        utc(2026, 6, 15, 13, 0),
        tz,
    )
    .insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 15, 9, 0),
        utc(2026, 6, 15, 12, 0),
        Brussels,
    );
    let titles: Vec<_> = got.iter().map(|o| o.title.as_str()).collect();
    assert_eq!(titles, ["running"]);
}

#[test]
fn hidden_calendar_is_excluded() {
    let conn = test_connection();
    setup(&conn);
    let hidden = add_calendar(&conn, "cal-hidden", "read", false);
    weekly_brussels(&conn, hidden);
    let start = utc(2026, 6, 15, 9, 0);
    Ev::new("one", start, start + HOUR, Some("UTC")).insert(&conn, hidden);

    assert!(
        list(
            &conn,
            utc(2026, 6, 1, 0, 0),
            utc(2026, 7, 1, 0, 0),
            Brussels
        )
        .is_empty()
    );
}

// ---- series and exceptions -----------------------------------------------------------------

#[test]
fn series_is_expanded_and_sorted_with_single_events() {
    let conn = test_connection();
    let cal = setup(&conn);
    let id = weekly_brussels(&conn, cal);
    let single = utc(2026, 6, 3, 12, 0);
    Ev::new("single", single, single + HOUR, Some("UTC")).insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 1, 0, 0),
        utc(2026, 6, 15, 0, 0),
        Brussels,
    );
    assert_eq!(
        starts(&got),
        [
            timed(Brussels, 2026, 6, 1, 9, 0),
            timed(Brussels, 2026, 6, 3, 14, 0),
            timed(Brussels, 2026, 6, 8, 9, 0)
        ]
    );
    assert_eq!(got[2].event_id, id);
    assert_eq!(got[2].original_start, local(Brussels, 2026, 6, 8, 9, 0));
    assert_eq!(got[2].end, timed(Brussels, 2026, 6, 8, 10, 0));
}

#[test]
fn series_seen_from_new_york_across_both_dst_changes() {
    let conn = test_connection();
    let cal = setup(&conn);
    let start = local(Brussels, 2026, 3, 2, 9, 0);
    Ev {
        recurrence: Some("RRULE:FREQ=WEEKLY;COUNT=5"),
        ..Ev::new("series", start, start + HOUR, Some("Europe/Brussels"))
    }
    .insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 3, 1, 0, 0),
        utc(2026, 4, 1, 0, 0),
        New_York,
    );
    assert_eq!(
        starts(&got),
        [
            timed(New_York, 2026, 3, 2, 3, 0),
            timed(New_York, 2026, 3, 9, 4, 0),
            timed(New_York, 2026, 3, 16, 4, 0),
            timed(New_York, 2026, 3, 23, 4, 0),
            timed(New_York, 2026, 3, 30, 3, 0)
        ]
    );
}

#[test]
fn all_day_series_keeps_its_dates_for_every_viewer() {
    let conn = test_connection();
    let cal = setup(&conn);
    let stored = utc(2026, 12, 21, 0, 0);
    Ev {
        all_day: true,
        recurrence: Some("RRULE:FREQ=WEEKLY;COUNT=3"),
        ..Ev::new("weekly", stored, stored + DAY, None)
    }
    .insert(&conn, cal);

    for viewer in [New_York, Auckland] {
        let from = local(viewer, 2026, 12, 1, 0, 0);
        let to = local(viewer, 2027, 2, 1, 0, 0);
        let got = list(&conn, from, to, viewer);
        assert_eq!(
            starts(&got),
            [date(2026, 12, 21), date(2026, 12, 28), date(2027, 1, 4)],
            "{viewer}"
        );
    }
}

#[test]
fn moved_exception_replaces_its_occurrence() {
    let conn = test_connection();
    let cal = setup(&conn);
    let master = weekly_brussels(&conn, cal);
    let original = local(Brussels, 2026, 6, 8, 9, 0);
    let moved = local(Brussels, 2026, 6, 9, 14, 0);
    Ev {
        title: "moved",
        ..Ev::new(
            "series_20260608",
            moved,
            moved + HOUR,
            Some("Europe/Brussels"),
        )
    }
    .exception(("series", master), original)
    .insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 1, 0, 0),
        utc(2026, 6, 15, 0, 0),
        Brussels,
    );
    assert_eq!(
        starts(&got),
        [
            timed(Brussels, 2026, 6, 1, 9, 0),
            timed(Brussels, 2026, 6, 9, 14, 0)
        ]
    );
    assert_eq!(got[1].title, "moved");
    assert_eq!(got[1].event_id, master);
    assert_eq!(got[1].original_start, original);
}

#[test]
fn occurrence_moved_into_the_range_appears() {
    let conn = test_connection();
    let cal = setup(&conn);
    let master = weekly_brussels(&conn, cal);
    // The 1 June occurrence moves to 16 June; the range covers only 15 to 22 June.
    let moved = local(Brussels, 2026, 6, 16, 11, 0);
    Ev::new(
        "series_20260601",
        moved,
        moved + HOUR,
        Some("Europe/Brussels"),
    )
    .exception(("series", master), local(Brussels, 2026, 6, 1, 9, 0))
    .insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 15, 0, 0),
        utc(2026, 6, 22, 0, 0),
        Brussels,
    );
    assert_eq!(
        starts(&got),
        [
            timed(Brussels, 2026, 6, 15, 9, 0),
            timed(Brussels, 2026, 6, 16, 11, 0)
        ]
    );
}

#[test]
fn occurrence_moved_out_of_the_range_disappears() {
    let conn = test_connection();
    let cal = setup(&conn);
    let master = weekly_brussels(&conn, cal);
    // The 15 June occurrence moves to 30 June, outside the range.
    let moved = local(Brussels, 2026, 6, 30, 9, 0);
    Ev::new(
        "series_20260615",
        moved,
        moved + HOUR,
        Some("Europe/Brussels"),
    )
    .exception(("series", master), local(Brussels, 2026, 6, 15, 9, 0))
    .insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 15, 0, 0),
        utc(2026, 6, 23, 0, 0),
        Brussels,
    );
    assert_eq!(starts(&got), [timed(Brussels, 2026, 6, 22, 9, 0)]);
}

#[test]
fn cancelled_exception_removes_its_occurrence() {
    let conn = test_connection();
    let cal = setup(&conn);
    let master = weekly_brussels(&conn, cal);
    let original = local(Brussels, 2026, 6, 8, 9, 0);
    Ev {
        status: "cancelled",
        ..Ev::new(
            "series_20260608",
            original,
            original + HOUR,
            Some("Europe/Brussels"),
        )
    }
    .exception(("series", master), original)
    .insert(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 1, 0, 0),
        utc(2026, 6, 15, 0, 0),
        Brussels,
    );
    assert_eq!(starts(&got), [timed(Brussels, 2026, 6, 1, 9, 0)]);
}

#[test]
fn unlinked_exception_is_invisible() {
    let conn = test_connection();
    let cal = setup(&conn);
    // Its master has not arrived yet (§7.2), so `recurring_event_id` is still NULL.
    let start = utc(2026, 6, 10, 9, 0);
    Ev {
        recurring_remote_id: Some("not-yet"),
        original_start: Some(utc(2026, 6, 9, 9, 0)),
        ..Ev::new("not-yet_20260609", start, start + HOUR, Some("UTC"))
    }
    .insert(&conn, cal);

    assert!(
        list(
            &conn,
            utc(2026, 6, 1, 0, 0),
            utc(2026, 7, 1, 0, 0),
            Brussels
        )
        .is_empty()
    );
}

#[test]
fn cancelled_series_is_not_shown() {
    let conn = test_connection();
    let cal = setup(&conn);
    let start = local(Brussels, 2026, 6, 1, 9, 0);
    Ev {
        status: "cancelled",
        recurrence: Some("RRULE:FREQ=WEEKLY"),
        ..Ev::new("series", start, start + HOUR, Some("Europe/Brussels"))
    }
    .insert(&conn, cal);

    assert!(
        list(
            &conn,
            utc(2026, 6, 1, 0, 0),
            utc(2026, 7, 1, 0, 0),
            Brussels
        )
        .is_empty()
    );
}

#[test]
fn malformed_rule_shows_only_the_first_occurrence() {
    let conn = test_connection();
    let cal = setup(&conn);
    let start = local(Brussels, 2026, 6, 1, 9, 0);
    Ev {
        recurrence: Some("RRULE:FREQ=SOMETIMES"),
        ..Ev::new("broken", start, start + HOUR, Some("Europe/Brussels"))
    }
    .insert(&conn, cal);
    weekly_brussels(&conn, cal);

    let got = list(
        &conn,
        utc(2026, 6, 1, 0, 0),
        utc(2026, 6, 15, 0, 0),
        Brussels,
    );
    let titles: Vec<_> = got.iter().map(|o| o.title.as_str()).collect();
    // The working series is unaffected; the broken one shows once.
    assert_eq!(titles, ["broken", "series", "series"]);
}

#[test]
fn endless_series_far_ahead_is_computed_and_never_stored() {
    let conn = test_connection();
    let cal = setup(&conn);
    let start = local(Brussels, 2020, 1, 6, 9, 0);
    Ev {
        recurrence: Some("RRULE:FREQ=DAILY"),
        ..Ev::new("daily", start, start + HOUR, Some("Europe/Brussels"))
    }
    .insert(&conn, cal);
    let before = count_events(&conn);

    let got = list(
        &conn,
        utc(2040, 6, 4, 0, 0),
        utc(2040, 6, 11, 0, 0),
        Brussels,
    );
    assert_eq!(got.len(), 7);
    assert_eq!(count_events(&conn), before);
}
