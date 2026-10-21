//! Turning a feed's text into events (`docs/architecture.md` §7.6, §7.7).
//!
//! Only `icalendar`'s low-level parser is used: it gives every property with its raw value and
//! parameters, and every component with its sub-components, `VTIMEZONE` parts included. The
//! typed API would hide the raw `TZID`s that §7.7 needs (§13 point 3).

#[cfg(test)]
mod tests;
mod zones;

use std::collections::BTreeSet;

use chrono::{Days, NaiveDate, NaiveDateTime, NaiveTime};
use chrono_tz::Tz;
use icalendar::parser::{self, Component, Property};

use crate::model::EventStatus;
use crate::provider::{Reminders, RemoteEvent, SeriesRole};
use crate::recurrence::resolve_local;
use zones::{Zone, Zones, local_time};

const DAY: i64 = 24 * 3600;

/// Why a feed could not be read at all. A single broken event is skipped instead.
#[derive(Debug, thiserror::Error)]
#[error("not an iCalendar feed: {0}")]
pub(super) struct ParseError(&'static str);

/// What a feed holds.
#[derive(Debug)]
pub(super) struct Feed {
    pub events: Vec<RemoteEvent>,
    /// Problems the user should know about, such as a time zone that could not be resolved
    /// (§7.7). Each is listed once, however many events it affects.
    pub warnings: Vec<String>,
}

/// Every event in the feed. Events without a `UID` or a usable `DTSTART` are logged and left
/// out, so one broken event never hides the rest.
pub(super) fn parse_feed(text: &str) -> Result<Feed, ParseError> {
    // Lines may be folded (RFC 5545 §3.1): a line that starts with a space continues the one
    // before it. The parser expects them joined.
    let unfolded = parser::unfold(text.trim_start_matches('\u{feff}'));
    // The parser's own error quotes the input around the problem. That would put calendar
    // contents into the source's `last_error`, so only a fixed text is kept.
    let roots = parser::read_calendar_simple(&unfolded).map_err(|_| ParseError("syntax error"))?;
    let calendars: Vec<&Component<'_>> = roots.iter().filter(|c| is(c, "VCALENDAR")).collect();
    if calendars.is_empty() {
        return Err(ParseError("no VCALENDAR"));
    }

    // `VALARM`s are sub-components of their event and never looked at, `VTODO`s and
    // `VJOURNAL`s are skipped here, and `VTIMEZONE`s are read by `Zones`.
    let mut events = Vec::new();
    let mut warnings = BTreeSet::new();
    for calendar in calendars {
        let mut zones = Zones::of(calendar);
        events.extend(
            calendar
                .components
                .iter()
                .filter(|c| is(c, "VEVENT"))
                .filter_map(|c| {
                    event(c, &mut zones)
                        .inspect_err(|e| log::warn!("skipping an event in the feed: {e}"))
                        .ok()
                }),
        );
        warnings.extend(zones.into_warnings());
    }
    Ok(Feed {
        events,
        warnings: warnings.into_iter().collect(),
    })
}

/// One `VEVENT` as an event row.
fn event(c: &Component<'_>, zones: &mut Zones) -> Result<RemoteEvent, String> {
    let uid = prop(c, "UID")
        .map(|p| p.val.as_str().trim())
        .filter(|uid| !uid.is_empty())
        .ok_or("no UID")?;
    let start = Time::parse(
        prop(c, "DTSTART").ok_or_else(|| format!("{uid}: no DTSTART"))?,
        zones,
    )
    .map_err(|e| format!("{uid}: DTSTART {e}"))?;
    let mut end = match (prop(c, "DTEND"), prop(c, "DURATION")) {
        (Some(p), _) => Time::parse(p, zones).map_err(|e| format!("{uid}: DTEND {e}"))?,
        (None, Some(p)) => start.plus(Duration::parse(p.val.as_str())?),
        // RFC 5545 §3.6.1: a date lasts one day, a date-time ends where it starts.
        (None, None) if start.all_day => start.plus(Duration::days(1)),
        (None, None) => start,
    };
    if end.stored < start.stored {
        log::warn!("{uid}: ends before it starts; using its start as end");
        end = start;
    }

    let series = match prop(c, "RECURRENCE-ID") {
        Some(p) => SeriesRole::Exception {
            master_remote_id: uid.to_owned(),
            original_start_utc: Time::parse(p, zones)
                .map_err(|e| format!("{uid}: RECURRENCE-ID {e}"))?
                .stored,
        },
        None => master(c, zones).unwrap_or(SeriesRole::Single),
    };
    // An exception shares its master's UID; the occurrence it replaces makes it unique.
    let remote_id = match &series {
        SeriesRole::Exception {
            original_start_utc, ..
        } => format!("{uid}#{original_start_utc}"),
        _ => uid.to_owned(),
    };

    Ok(RemoteEvent {
        remote_id,
        ical_uid: Some(uid.to_owned()),
        status: match prop(c, "STATUS").map(|p| p.val.as_str().trim()) {
            Some(s) if s.eq_ignore_ascii_case("CANCELLED") => EventStatus::Cancelled,
            Some(s) if s.eq_ignore_ascii_case("TENTATIVE") => EventStatus::Tentative,
            _ => EventStatus::Confirmed,
        },
        // Text values arrive unescaped: the parser undoes `\,`, `\;`, `\n` and `\\`.
        title: text(c, "SUMMARY").unwrap_or_default(),
        description: text(c, "DESCRIPTION"),
        location: text(c, "LOCATION"),
        color: None,
        all_day: start.all_day,
        start_utc: start.stored,
        end_utc: end.stored,
        start_tz: start.zone_name(),
        end_tz: end.zone_name(),
        series,
        remote_updated_at: prop(c, "LAST-MODIFIED")
            .and_then(|p| Time::parse(p, zones).ok())
            .map(|t| t.stored),
        // A feed is read-only through its calendar's access; these only matter for writing.
        etag: None,
        is_organizer: true,
        guests_can_modify: false,
        reminders: Reminders::default(),
    })
}

/// The master role, if the event has an `RRULE` or `RDATE`. An `EXDATE` alone excludes
/// nothing and does not make a series.
fn master(c: &Component<'_>, zones: &mut Zones) -> Option<SeriesRole> {
    let lines: Vec<&Property<'_>> = c
        .properties
        .iter()
        .filter(|p| ["RRULE", "RDATE", "EXDATE"].iter().any(|n| named(p, n)))
        .collect();
    let has_rule = lines.iter().any(|p| named(p, "RRULE") || named(p, "RDATE"));
    has_rule.then(|| SeriesRole::Master {
        recurrence: lines
            .into_iter()
            .map(|p| recurrence_line(p, zones))
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

/// A recurrence property as a line the `recurrence` module reads.
///
/// Only `TZID` and `VALUE` are kept. Local times are resolved like `DTSTART` (§7.7 "Mixed
/// forms"): written with the IANA name their `TZID` resolves to, or with the feed's zone if they
/// have none. A zone that only resolves to fixed offsets has no name to write, so its values are
/// converted to UTC instead. Floating values stay without `TZID`.
fn recurrence_line(p: &Property<'_>, zones: &mut Zones) -> String {
    let name = p.name.as_str().to_ascii_uppercase();
    let value = p.val.as_str().trim();
    let value_param = param(p, "VALUE")
        .map(|v| format!(";VALUE={v}"))
        .unwrap_or_default();
    // Dates, UTC times and periods are not affected by a zone.
    let Some(first) = value.split(',').find_map(local_time) else {
        return format!("{name}{value_param}:{value}");
    };
    let tzid = param(p, "TZID");
    let zone = match tzid {
        Some(tzid) => zones.resolve(tzid, first),
        None => zones.default(),
    };
    match (zone, tzid) {
        (Some(Zone::Iana(tz)), _) => format!("{name};TZID={}{value_param}:{value}", tz.name()),
        (Some(Zone::Fixed(_)), Some(tzid)) => {
            let values: Vec<String> = value
                .split(',')
                .map(|v| match local_time(v) {
                    Some(local) => {
                        let instant = zones
                            .resolve(tzid, local)
                            .map_or(local.and_utc().timestamp(), |z| z.instant(local));
                        chrono::DateTime::from_timestamp(instant, 0)
                            .unwrap_or_default()
                            .format("%Y%m%dT%H%M%SZ")
                            .to_string()
                    }
                    None => v.trim().to_owned(),
                })
                .collect();
            format!("{name}{value_param}:{}", values.join(","))
        }
        _ => format!("{name}{value_param}:{value}"),
    }
}

/// A `DTSTART`, `DTEND`, `RECURRENCE-ID` or `LAST-MODIFIED` value, as stored (§4).
#[derive(Debug, Clone, Copy)]
struct Time {
    /// The `start_utc` / `end_utc` value: an instant for a zoned value, the wall clock written
    /// as UTC for a floating or all-day one.
    stored: i64,
    /// `None` for floating and all-day values.
    zone: Option<Zone>,
    all_day: bool,
}

impl Time {
    fn parse(p: &Property<'_>, zones: &mut Zones) -> Result<Time, String> {
        let value = p.val.as_str().trim();
        let invalid = || format!("{value:?} is not a date or time");
        let is_date =
            param(p, "VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || value.len() == 8;
        if is_date {
            let date = NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| invalid())?;
            return Ok(Time {
                stored: date.and_time(NaiveTime::MIN).and_utc().timestamp(),
                zone: None,
                all_day: true,
            });
        }

        let (text, utc) = match value.strip_suffix(['Z', 'z']) {
            Some(text) => (text, true),
            None => (value, false),
        };
        let local = NaiveDateTime::parse_from_str(text, "%Y%m%dT%H%M%S").map_err(|_| invalid())?;
        let zone = match param(p, "TZID") {
            _ if utc => Some(Zone::Iana(Tz::UTC)),
            Some(tzid) => zones.resolve(tzid, local),
            None => zones.default(),
        };
        Ok(Time {
            stored: match zone {
                Some(zone) => zone.instant(local),
                None => local.and_utc().timestamp(),
            },
            zone,
            all_day: false,
        })
    }

    /// This time plus `d`. Days and weeks are nominal (RFC 5545 §3.3.6): they move the wall
    /// clock, so a day across a daylight-saving change is 23 or 25 hours long.
    fn plus(self, d: Duration) -> Time {
        let stored = match self.zone {
            Some(Zone::Iana(tz)) => {
                let wall = chrono::DateTime::from_timestamp(self.stored, 0)
                    .unwrap_or_default()
                    .with_timezone(&tz)
                    .naive_local();
                let moved = if d.days >= 0 {
                    wall.checked_add_days(Days::new(d.days.unsigned_abs()))
                } else {
                    wall.checked_sub_days(Days::new(d.days.unsigned_abs()))
                };
                resolve_local(tz, moved.unwrap_or(wall)).timestamp() + d.seconds
            }
            // A fixed offset has no daylight saving: a day is always 24 hours.
            Some(Zone::Fixed(_)) | None => self.stored + d.days * DAY + d.seconds,
        };
        Time { stored, ..self }
    }

    fn zone_name(&self) -> Option<String> {
        self.zone.map(Zone::column)
    }
}

/// A `DURATION` value, split into nominal days and exact seconds.
#[derive(Debug, Clone, Copy)]
struct Duration {
    days: i64,
    seconds: i64,
}

impl Duration {
    fn days(days: i64) -> Duration {
        Duration { days, seconds: 0 }
    }

    /// Parses `[+-]P[nW][nD][T[nH][nM][nS]]`.
    fn parse(value: &str) -> Result<Duration, String> {
        let invalid = || format!("{value:?} is not a duration");
        let value = value.trim();
        let (sign, rest) = match value.strip_prefix('-') {
            Some(rest) => (-1, rest),
            None => (1, value.strip_prefix('+').unwrap_or(value)),
        };
        let rest = rest.strip_prefix(['P', 'p']).ok_or_else(invalid)?;
        let (mut days, mut seconds) = (0, 0);
        let mut number = String::new();
        let mut in_time = false;
        for ch in rest.chars() {
            match ch.to_ascii_uppercase() {
                '0'..='9' => number.push(ch),
                'T' if number.is_empty() => in_time = true,
                unit => {
                    let n: i64 = number.parse().map_err(|_| invalid())?;
                    number.clear();
                    match (unit, in_time) {
                        ('W', false) => days += 7 * n,
                        ('D', false) => days += n,
                        ('H', true) => seconds += 3600 * n,
                        ('M', true) => seconds += 60 * n,
                        ('S', true) => seconds += n,
                        _ => return Err(invalid()),
                    }
                }
            }
        }
        if !number.is_empty() {
            return Err(invalid());
        }
        Ok(Duration {
            days: sign * days,
            seconds: sign * seconds,
        })
    }
}

fn is(c: &Component<'_>, name: &str) -> bool {
    c.name.as_str().eq_ignore_ascii_case(name)
}

fn named(p: &Property<'_>, name: &str) -> bool {
    p.name.as_str().eq_ignore_ascii_case(name)
}

/// The first property called `name`. Names are case-insensitive (RFC 5545 §2).
fn prop<'c, 'a>(c: &'c Component<'a>, name: &str) -> Option<&'c Property<'a>> {
    c.properties.iter().find(|p| named(p, name))
}

/// A text property, if present and not empty.
fn text(c: &Component<'_>, name: &str) -> Option<String> {
    prop(c, name)
        .map(|p| p.val.as_str().trim())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// The value of parameter `key`, quotes removed.
fn param<'p>(p: &'p Property<'_>, key: &str) -> Option<&'p str> {
    p.params
        .iter()
        .find(|param| param.key.as_str().eq_ignore_ascii_case(key))
        .and_then(|param| param.val.as_ref())
        .map(|v| v.as_str().trim_matches('"'))
}
