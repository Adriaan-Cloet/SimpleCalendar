use chrono::{TimeZone, Utc};
use chrono_tz::Europe::Brussels;

use super::*;
use crate::provider::ics::parse::parse_feed;
use crate::provider::{Field, Reminder, ReminderMethod, Reminders, RemoteEvent, SeriesRole};

const HOUR: i64 = 3600;
const NOW: i64 = 1_793_000_000; // 2026-10-26 07:33:20 UTC

// ---- helpers -------------------------------------------------------------------------------

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

/// A resource around the given lines, with CRLF line ends.
fn resource(body: &str) -> String {
    let lines: Vec<&str> = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Other//EN"]
        .into_iter()
        .chain(body.lines().map(str::trim).filter(|l| !l.is_empty()))
        .chain(["END:VCALENDAR"])
        .collect();
    lines.join("\r\n") + "\r\n"
}

/// Every unfolded line of `text`.
fn lines(text: &str) -> Vec<String> {
    text.replace("\r\n ", "")
        .split("\r\n")
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn has_line(text: &str, line: &str) -> bool {
    lines(text).iter().any(|l| l == line)
}

/// The lines of `text` that start with `prefix`.
fn starting(text: &str, prefix: &str) -> Vec<String> {
    lines(text)
        .into_iter()
        .filter(|l| l.starts_with(prefix))
        .collect()
}

fn events(text: &str) -> Vec<RemoteEvent> {
    parse_feed(text).expect("the result parses").events
}

fn only(text: &str) -> RemoteEvent {
    let mut events = events(text);
    assert_eq!(events.len(), 1, "{events:#?}");
    events.remove(0)
}

fn popup(minutes_before: u32) -> Reminder {
    Reminder {
        method: ReminderMethod::Popup,
        minutes_before,
    }
}

fn data() -> EventData {
    EventData {
        title: "Planning".into(),
        description: None,
        location: None,
        all_day: false,
        start_utc: brussels(2026, 11, 2, 14, 0),
        end_utc: brussels(2026, 11, 2, 15, 0),
        start_tz: Some("Europe/Brussels".into()),
        end_tz: Some("Europe/Brussels".into()),
        recurrence: None,
        reminders: Reminders {
            use_default: false,
            overrides: vec![],
        },
    }
}

fn changed(fields: &[Field]) -> FieldSet {
    fields.iter().copied().collect()
}

/// A weekly series at 14:00 Brussels from 2 November, with an exception on 9 November, and
/// lines this app does not know.
fn series() -> String {
    resource(
        "BEGIN:VTIMEZONE\nTZID:Europe/Brussels\nBEGIN:STANDARD\nDTSTART:19701025T030000
         TZOFFSETFROM:+0200\nTZOFFSETTO:+0100\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU
         END:STANDARD\nBEGIN:DAYLIGHT\nDTSTART:19700329T020000\nTZOFFSETFROM:+0100
         TZOFFSETTO:+0200\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\nEND:DAYLIGHT\nEND:VTIMEZONE
         BEGIN:VEVENT\nUID:s1\nDTSTAMP:20260101T000000Z\nSEQUENCE:2
         DTSTART;TZID=Europe/Brussels:20261102T140000\nDURATION:PT1H
         RRULE:FREQ=WEEKLY\nSUMMARY:Weekly\nCATEGORIES:Work,Team\nX-OTHER-APP:keep me
         DESCRIPTION;ALTREP=\"cid:part1\":Notes
         ORGANIZER:mailto:alice@example.com\nATTENDEE;CN=Bob:mailto:bob@example.com
         BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT10M\nX-WR-ALARMUID:A1\nEND:VALARM
         END:VEVENT
         BEGIN:VEVENT\nUID:s1\nDTSTAMP:20260101T000000Z
         RECURRENCE-ID;TZID=Europe/Brussels:20261109T140000
         DTSTART;TZID=Europe/Brussels:20261109T160000\nDTEND;TZID=Europe/Brussels:20261109T170000
         SUMMARY:Moved\nX-OTHER-APP:exception\nEND:VEVENT",
    )
}

fn master_of(text: &str) -> RemoteEvent {
    events(text)
        .into_iter()
        .find(|e| matches!(e.series, SeriesRole::Master { .. } | SeriesRole::Single))
        .expect("a master")
}

fn exception_at(text: &str, original: i64) -> Option<RemoteEvent> {
    events(text).into_iter().find(|e| {
        matches!(e.series, SeriesRole::Exception { original_start_utc, .. }
            if original_start_utc == original)
    })
}

// ---- reading and writing the text ----------------------------------------------------------

#[test]
fn text_is_written_back_as_it_was_read() {
    let text = series();
    assert_eq!(Object::parse(&text).unwrap().to_text(), text);
}

#[test]
fn long_lines_are_folded_at_75_bytes_and_read_unfolded() {
    let long = format!("X-LONG:{}é{}", "a".repeat(80), "b".repeat(80));
    let text = resource(&format!("BEGIN:VEVENT\nUID:a\n{long}\nEND:VEVENT"));
    let written = Object::parse(&text).unwrap().to_text();
    assert!(written.split("\r\n").all(|l| l.len() <= 75), "{written}");
    // A fold never splits a character.
    assert!(
        written
            .split("\r\n")
            .all(|l| std::str::from_utf8(l.as_bytes()).is_ok())
    );
    assert!(has_line(&written, &long));
    // Folded input is read back joined, also with LF line ends and tabs.
    let folded = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nX-LONG:one\n two\n\tthree\nEND:VEVENT\nEND:VCALENDAR\n";
    assert!(has_line(
        &Object::parse(folded).unwrap().to_text(),
        "X-LONG:onetwothree"
    ));
}

#[test]
fn text_that_is_not_icalendar_is_refused() {
    for text in [
        "<html></html>",
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nEND:VCALENDAR\n",
        "UID:a\n",
    ] {
        assert!(Object::parse(text).is_none(), "{text}");
    }
}

// ---- a new event ---------------------------------------------------------------------------

#[test]
fn a_new_event_reads_back_as_written() {
    let mut cases = vec![data()];
    // All-day, over two days.
    cases.push(EventData {
        all_day: true,
        start_utc: utc(2026, 11, 2, 0, 0),
        end_utc: utc(2026, 11, 4, 0, 0),
        start_tz: None,
        end_tz: None,
        ..data()
    });
    // Floating.
    cases.push(EventData {
        start_utc: utc(2026, 11, 2, 9, 0),
        end_utc: utc(2026, 11, 2, 10, 0),
        start_tz: None,
        end_tz: None,
        ..data()
    });
    // In UTC, ending in another zone, with text that needs escaping and reminders.
    cases.push(EventData {
        title: "Lunch, then; talk \\ more".into(),
        description: Some("Line one\nLine two".into()),
        location: Some("Room 1, floor 2".into()),
        start_utc: utc(2026, 11, 2, 11, 0),
        end_utc: utc(2026, 11, 2, 20, 0),
        start_tz: Some("UTC".into()),
        end_tz: Some("America/New_York".into()),
        reminders: Reminders {
            use_default: false,
            overrides: vec![
                popup(0),
                popup(30),
                Reminder {
                    method: ReminderMethod::Email,
                    minutes_before: 1440,
                },
            ],
        },
        ..data()
    });
    // A series.
    cases.push(EventData {
        recurrence: Some(
            "RRULE:FREQ=WEEKLY;BYDAY=MO,WE\nEXDATE;TZID=Europe/Brussels:20261104T140000".into(),
        ),
        ..data()
    });
    for case in cases {
        let text = create("abc", &case, NOW);
        let event = only(&text);
        assert_eq!(EventData::from(&event), case, "{text}");
        assert_eq!(event.ical_uid.as_deref(), Some("abc"));
    }
}

#[test]
fn a_new_event_has_the_lines_servers_ask_for() {
    let text = create("abc", &data(), NOW);
    for line in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//SimpleCalendar//EN",
        "UID:abc",
        "DTSTAMP:20261026T073320Z",
        "SEQUENCE:0",
        "DTSTART;TZID=Europe/Brussels:20261102T140000",
        "DTEND;TZID=Europe/Brussels:20261102T150000",
        "SUMMARY:Planning",
    ] {
        assert!(has_line(&text, line), "{line} in {text}");
    }
    // Each zone a time names is defined, once.
    assert_eq!(starting(&text, "BEGIN:VTIMEZONE").len(), 1);
    assert!(has_line(&text, "TZID:Europe/Brussels"));
    // A cleared text is no line at all.
    assert!(starting(&text, "DESCRIPTION").is_empty());
}

#[test]
fn a_reminder_on_a_new_event_is_an_alarm() {
    let text = create(
        "abc",
        &EventData {
            reminders: Reminders {
                use_default: false,
                overrides: vec![popup(15)],
            },
            ..data()
        },
        NOW,
    );
    let alarm = lines(&text)
        .into_iter()
        .skip_while(|l| l != "BEGIN:VALARM")
        .take_while(|l| l != "END:VALARM")
        .collect::<Vec<_>>();
    assert_eq!(
        alarm,
        [
            "BEGIN:VALARM",
            "ACTION:DISPLAY",
            "DESCRIPTION:Planning",
            "TRIGGER:-PT15M"
        ]
    );
}

// ---- changing an event ---------------------------------------------------------------------

#[test]
fn a_change_rewrites_only_the_changed_lines() {
    let desired = EventData {
        title: "Weekly sync".into(),
        ..EventData::from(&master_of(&series()))
    };
    let text = update(&series(), None, &desired, &changed(&[Field::Title]), NOW).unwrap();
    assert!(has_line(&text, "SUMMARY:Weekly sync"));
    for kept in [
        "CATEGORIES:Work,Team",
        "X-OTHER-APP:keep me",
        "DESCRIPTION;ALTREP=\"cid:part1\":Notes",
        "DTSTART;TZID=Europe/Brussels:20261102T140000",
        "DURATION:PT1H",
        "RRULE:FREQ=WEEKLY",
        "X-WR-ALARMUID:A1",
        "PRODID:-//Other//EN",
        // The exception is left alone.
        "SUMMARY:Moved",
        "X-OTHER-APP:exception",
    ] {
        assert!(has_line(&text, kept), "{kept} in {text}");
    }
    assert!(has_line(&text, "SEQUENCE:3"));
    assert!(has_line(&text, "DTSTAMP:20261026T073320Z"));
    assert!(has_line(&text, "LAST-MODIFIED:20261026T073320Z"));
    assert_eq!(EventData::from(&master_of(&text)), desired);
}

#[test]
fn a_changed_description_drops_its_html_copy() {
    let text = resource(
        "BEGIN:VEVENT\nUID:a\nDTSTART:20261102T140000Z\nDESCRIPTION:old
         X-ALT-DESC;FMTTYPE=text/html:<b>old</b>\nEND:VEVENT",
    );
    let desired = EventData {
        description: Some("new".into()),
        ..EventData::from(&only(&text))
    };
    let text = update(&text, None, &desired, &changed(&[Field::Description]), NOW).unwrap();
    assert!(has_line(&text, "DESCRIPTION:new"));
    assert!(starting(&text, "X-ALT-DESC").is_empty());
}

#[test]
fn new_times_replace_the_duration_and_drop_the_exceptions() {
    let desired = EventData {
        start_utc: brussels(2026, 11, 3, 9, 0),
        end_utc: brussels(2026, 11, 3, 9, 30),
        ..EventData::from(&master_of(&series()))
    };
    let text = update(&series(), None, &desired, &changed(&[Field::Times]), NOW).unwrap();
    assert!(starting(&text, "DURATION").is_empty());
    assert!(has_line(
        &text,
        "DTSTART;TZID=Europe/Brussels:20261103T090000"
    ));
    assert!(has_line(
        &text,
        "DTEND;TZID=Europe/Brussels:20261103T093000"
    ));
    // The occurrences moved, so the exception no longer replaces one of them.
    assert!(starting(&text, "RECURRENCE-ID").is_empty());
    assert_eq!(events(&text).len(), 1);
    assert_eq!(EventData::from(&master_of(&text)), desired);
}

#[test]
fn a_new_rule_replaces_the_recurrence_lines() {
    let desired = EventData {
        recurrence: Some("RRULE:FREQ=DAILY;COUNT=5".into()),
        ..EventData::from(&master_of(&series()))
    };
    let text = update(
        &series(),
        None,
        &desired,
        &changed(&[Field::Recurrence]),
        NOW,
    )
    .unwrap();
    // The time zone's own rules are not the event's.
    assert_eq!(
        starting(&text, "RRULE:FREQ=DAILY"),
        ["RRULE:FREQ=DAILY;COUNT=5"]
    );
    assert!(starting(&text, "RRULE:FREQ=WEEKLY").is_empty());
    assert!(starting(&text, "RECURRENCE-ID").is_empty());

    // No rule at all makes it a single event.
    let desired = EventData {
        recurrence: None,
        ..desired
    };
    let text = update(&text, None, &desired, &changed(&[Field::Recurrence]), NOW).unwrap();
    assert!(starting(&text, "RRULE:FREQ=DAILY").is_empty());
    assert_eq!(master_of(&text).series, SeriesRole::Single);
}

#[test]
fn reminders_keep_the_alarms_that_stay_and_those_a_reminder_cannot_hold() {
    let text = resource(
        "BEGIN:VEVENT\nUID:a\nDTSTART:20261102T140000Z\nSUMMARY:A
         BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT10M\nX-WR-ALARMUID:keep\nEND:VALARM
         BEGIN:VALARM\nACTION:AUDIO\nTRIGGER:-PT1H\nX-WR-ALARMUID:drop\nEND:VALARM
         BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER;VALUE=DATE-TIME:20261102T120000Z
         X-WR-ALARMUID:fixed\nEND:VALARM\nEND:VEVENT",
    );
    let desired = EventData {
        reminders: Reminders {
            use_default: false,
            overrides: vec![popup(10), popup(1440)],
        },
        ..EventData::from(&only(&text))
    };
    let text = update(&text, None, &desired, &changed(&[Field::Reminders]), NOW).unwrap();
    assert!(has_line(&text, "X-WR-ALARMUID:keep"));
    assert!(has_line(&text, "X-WR-ALARMUID:fixed"));
    assert!(!has_line(&text, "X-WR-ALARMUID:drop"));
    assert!(has_line(&text, "TRIGGER:-PT1440M"));
    assert_eq!(
        only(&text).reminders.overrides,
        vec![popup(10), popup(1440)]
    );
}

#[test]
fn attendees_are_not_mailed_by_the_server() {
    let desired = EventData {
        title: "Weekly sync".into(),
        ..EventData::from(&master_of(&series()))
    };
    let text = update(&series(), None, &desired, &changed(&[Field::Title]), NOW).unwrap();
    assert!(has_line(
        &text,
        "ATTENDEE;SCHEDULE-AGENT=CLIENT;CN=Bob:mailto:bob@example.com"
    ));
    // One the client already decided about is left as it is.
    let text = resource(
        "BEGIN:VEVENT\nUID:a\nDTSTART:20261102T140000Z\nORGANIZER:mailto:a@example.com
         ATTENDEE;SCHEDULE-AGENT=SERVER:mailto:b@example.com\nEND:VEVENT",
    );
    let desired = EventData::from(&only(&text));
    let text = update(&text, None, &desired, &changed(&[Field::Title]), NOW).unwrap();
    assert!(has_line(
        &text,
        "ATTENDEE;SCHEDULE-AGENT=SERVER:mailto:b@example.com"
    ));
}

#[test]
fn a_resource_without_the_event_is_not_found() {
    let only_exception = resource(
        "BEGIN:VEVENT\nUID:s1\nRECURRENCE-ID:20261109T130000Z\nDTSTART:20261109T150000Z
         END:VEVENT",
    );
    assert_eq!(
        update(
            &only_exception,
            None,
            &data(),
            &changed(&[Field::Title]),
            NOW
        ),
        Err(EditError::NoEvent)
    );
    assert_eq!(
        update("nonsense", None, &data(), &changed(&[Field::Title]), NOW),
        Err(EditError::Unreadable)
    );
}

// ---- one occurrence ------------------------------------------------------------------------

#[test]
fn a_changed_occurrence_becomes_an_exception_in_the_same_resource() {
    let original = brussels(2026, 11, 16, 14, 0);
    let desired = EventData {
        title: "Weekly, later".into(),
        start_utc: brussels(2026, 11, 16, 15, 0),
        end_utc: brussels(2026, 11, 16, 16, 0),
        ..EventData::from(&master_of(&series()))
    };
    let desired = EventData {
        recurrence: None,
        ..desired
    };
    let text = update(
        &series(),
        Some(original),
        &desired,
        &changed(&[Field::Title, Field::Times]),
        NOW,
    )
    .unwrap();
    assert!(has_line(
        &text,
        "RECURRENCE-ID;TZID=Europe/Brussels:20261116T140000"
    ));
    let exception = exception_at(&text, original).expect("the new exception");
    assert_eq!(EventData::from(&exception), desired);
    // It keeps what the series has that this app does not know, but not its rule.
    let block = lines(&text)
        .into_iter()
        .skip_while(|l| l != "RECURRENCE-ID;TZID=Europe/Brussels:20261116T140000")
        .take_while(|l| l != "END:VEVENT")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(block.contains("CATEGORIES:Work,Team"), "{block}");
    assert!(block.contains("X-WR-ALARMUID:A1"), "{block}");
    assert!(!block.contains("RRULE"), "{block}");
    // The series and the other exception are as they were.
    assert_eq!(events(&text).len(), 3);
    assert!(has_line(&text, "SUMMARY:Moved"));
}

#[test]
fn a_changed_exception_is_rewritten_in_place() {
    let original = brussels(2026, 11, 9, 14, 0);
    let desired = EventData {
        title: "Moved again".into(),
        ..EventData::from(&exception_at(&series(), original).unwrap())
    };
    let text = update(
        &series(),
        Some(original),
        &desired,
        &changed(&[Field::Title]),
        NOW,
    )
    .unwrap();
    assert!(has_line(&text, "SUMMARY:Moved again"));
    assert!(has_line(&text, "X-OTHER-APP:exception"));
    assert_eq!(starting(&text, "RECURRENCE-ID").len(), 1);
}

#[test]
fn an_occurrence_of_an_all_day_series_is_named_by_its_date() {
    let text = resource(
        "BEGIN:VEVENT\nUID:d\nDTSTART;VALUE=DATE:20261102\nRRULE:FREQ=DAILY\nSUMMARY:D\nEND:VEVENT",
    );
    let original = utc(2026, 11, 4, 0, 0);
    let desired = EventData {
        title: "Changed".into(),
        start_utc: original,
        end_utc: original + 24 * HOUR,
        recurrence: None,
        ..EventData::from(&only(&text))
    };
    let text = update(
        &text,
        Some(original),
        &desired,
        &changed(&[Field::Title]),
        NOW,
    )
    .unwrap();
    assert!(has_line(&text, "RECURRENCE-ID;VALUE=DATE:20261104"));
    assert_eq!(exception_at(&text, original).unwrap().title, "Changed");
}

#[test]
fn a_deleted_occurrence_is_excluded_from_the_series() {
    // An exception goes, and its occurrence is excluded.
    let original = brussels(2026, 11, 9, 14, 0);
    let text = exclude(&series(), original, NOW).unwrap().unwrap();
    assert!(starting(&text, "RECURRENCE-ID").is_empty());
    assert!(has_line(
        &text,
        "EXDATE;TZID=Europe/Brussels:20261109T140000"
    ));
    let master = master_of(&text);
    let SeriesRole::Master { recurrence } = &master.series else {
        panic!("still a series");
    };
    assert!(recurrence.contains("EXDATE;TZID=Europe/Brussels:20261109T140000"));
    assert!(has_line(&text, "SEQUENCE:3"));

    // So is a plain occurrence; once it is, there is nothing left to do.
    let other = brussels(2026, 11, 23, 14, 0);
    let text = exclude(&text, other, NOW).unwrap().unwrap();
    assert!(has_line(
        &text,
        "EXDATE;TZID=Europe/Brussels:20261123T140000"
    ));
    assert_eq!(exclude(&text, other, NOW).unwrap(), None);
}

#[test]
fn an_occurrence_of_a_utc_series_is_named_in_utc() {
    let text =
        resource("BEGIN:VEVENT\nUID:u\nDTSTART:20261102T130000Z\nRRULE:FREQ=DAILY\nEND:VEVENT");
    let text = exclude(&text, utc(2026, 11, 3, 13, 0), NOW)
        .unwrap()
        .unwrap();
    assert!(has_line(&text, "EXDATE:20261103T130000Z"));
}

// ---- time zones ----------------------------------------------------------------------------

/// The lines of the `VTIMEZONE` written for `tz`.
fn zone_lines(tz: &str) -> Vec<String> {
    let text = create(
        "z",
        &EventData {
            start_tz: Some(tz.into()),
            end_tz: Some(tz.into()),
            ..data()
        },
        NOW,
    );
    lines(&text)
        .into_iter()
        .skip_while(|l| l != "BEGIN:VTIMEZONE")
        .take_while(|l| l != "END:VTIMEZONE")
        .collect()
}

#[test]
fn a_zone_with_daylight_saving_has_both_rules() {
    assert_eq!(
        zone_lines("Europe/Brussels"),
        [
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Brussels",
            "BEGIN:DAYLIGHT",
            "DTSTART:20260329T020000",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "DTSTART:20261025T030000",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
            "END:STANDARD",
        ]
    );
    // In the south, summer starts in October, on the first Sunday.
    let sydney = zone_lines("Australia/Sydney");
    assert!(sydney.contains(&"RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=1SU".to_owned()));
    assert!(sydney.contains(&"TZOFFSETTO:+1100".to_owned()));
}

#[test]
fn a_zone_without_daylight_saving_has_one_offset() {
    assert_eq!(
        zone_lines("Asia/Kolkata"),
        [
            "BEGIN:VTIMEZONE",
            "TZID:Asia/Kolkata",
            "BEGIN:STANDARD",
            "DTSTART:19700101T000000",
            "TZOFFSETFROM:+0530",
            "TZOFFSETTO:+0530",
            "TZNAME:IST",
            "END:STANDARD",
        ]
    );
}

#[test]
fn a_written_zone_gives_the_same_times_under_another_name() {
    // A reader that does not know the name goes by the rules alone.
    let text = create("z", &data(), NOW).replace("Europe/Brussels", "Office");
    let event = only(&text);
    assert_eq!(event.start_utc, brussels(2026, 11, 2, 14, 0));
    for (month, day) in [(1, 15), (7, 15)] {
        let summer_or_winter = EventData {
            start_utc: brussels(2027, month, day, 9, 0),
            end_utc: brussels(2027, month, day, 10, 0),
            ..data()
        };
        let text = create("z", &summer_or_winter, NOW).replace("Europe/Brussels", "Office");
        assert_eq!(only(&text).start_utc, summer_or_winter.start_utc);
    }
}
