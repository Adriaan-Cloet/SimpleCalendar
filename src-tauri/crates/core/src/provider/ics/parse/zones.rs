//! Resolving a feed's `TZID`s (`docs/architecture.md` §7.7).
//!
//! One `Zones` per `VCALENDAR`: it knows the calendar's `VTIMEZONE` blocks and its
//! `X-WR-TIMEZONE`, remembers what each name resolved to, and collects the warnings for the
//! source.

use std::collections::{BTreeSet, HashMap};

use chrono::{Datelike, FixedOffset, NaiveDate, NaiveDateTime};
use chrono_tz::Tz;
use icalendar::parser::Component;

use super::super::windows_zones::WINDOWS_ZONES;
use super::{is, named, prop};
use crate::recurrence::resolve_local;
use crate::recurrence::vtimezone::{FeedZone, Observance};

/// Behavioural matching compares this many months: three years from the start (§7.7).
const MATCH_MONTHS: u32 = 37;

/// What a zoned time is read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Zone {
    Iana(Tz),
    /// For a feed-defined zone that no IANA zone matches. The offset is the one the
    /// `VTIMEZONE` gives at that time, so start and end of one event can differ.
    Fixed(FixedOffset),
}

impl Zone {
    /// The instant of a local time in this zone.
    pub fn instant(self, local: NaiveDateTime) -> i64 {
        match self {
            Zone::Iana(tz) => resolve_local(tz, local).timestamp(),
            Zone::Fixed(offset) => {
                local.and_utc().timestamp() - i64::from(offset.local_minus_utc())
            }
        }
    }

    /// The `start_tz` / `end_tz` value: an IANA name, or an offset such as `+05:30`.
    pub fn column(self) -> String {
        match self {
            Zone::Iana(tz) => tz.name().to_owned(),
            Zone::Fixed(offset) => offset.to_string(),
        }
    }
}

/// A `VTIMEZONE` block, evaluated only once a `TZID` actually needs it.
struct Defined {
    /// `X-LIC-LOCATION`, if it is an IANA name.
    hint: Option<Tz>,
    observances: Vec<Observance>,
    /// `None` until first used; then `Some(None)` if the block has no usable observance.
    zone: Option<Option<FeedZone>>,
}

pub(super) struct Zones {
    /// `X-WR-TIMEZONE`, if it resolves to a zone.
    default: Option<Tz>,
    /// `VTIMEZONE` blocks by their `TZID`.
    defined: HashMap<String, Defined>,
    /// Steps 1 to 3 for each name seen so far.
    named: HashMap<String, Option<Tz>>,
    /// Behavioural matches by name and year.
    matched: HashMap<(String, i32), Option<Tz>>,
    warnings: BTreeSet<String>,
}

impl Zones {
    /// The zones of one `VCALENDAR`.
    pub fn of(calendar: &Component<'_>) -> Zones {
        let defined = calendar
            .components
            .iter()
            .filter(|c| is(c, "VTIMEZONE"))
            .filter_map(|c| {
                let tzid = clean(prop(c, "TZID")?.val.as_str()).to_owned();
                let hint = prop(c, "X-LIC-LOCATION").and_then(|p| iana(p.val.as_str()));
                let observances = c
                    .components
                    .iter()
                    .filter(|o| is(o, "STANDARD") || is(o, "DAYLIGHT"))
                    .filter_map(observance)
                    .collect();
                Some((
                    tzid,
                    Defined {
                        hint,
                        observances,
                        zone: None,
                    },
                ))
            })
            .collect();
        let default = prop(calendar, "X-WR-TIMEZONE").and_then(|p| by_name(p.val.as_str()));
        Zones {
            default,
            defined,
            named: HashMap::new(),
            matched: HashMap::new(),
            warnings: BTreeSet::new(),
        }
    }

    /// The zone for a time without `TZID` and without `Z`: the feed's `X-WR-TIMEZONE`, or
    /// `None` for floating (§7.7 "Floating times").
    pub fn default(&self) -> Option<Zone> {
        self.default.map(Zone::Iana)
    }

    /// The zone a `TZID` names, for the local time `local`. `None` means floating.
    ///
    /// The steps of §7.7, first match wins: an IANA name, a name that ends in one, a Windows
    /// name, a `VTIMEZONE` in the feed, and then the feed's own zone or floating.
    pub fn resolve(&mut self, tzid: &str, local: NaiveDateTime) -> Option<Zone> {
        let name = clean(tzid);
        // Rust note: `entry(..).or_insert_with(..)` computes the value only the first time a
        // name is seen; every later event with the same `TZID` gets the stored answer.
        if let Some(tz) = *self
            .named
            .entry(name.to_owned())
            .or_insert_with(|| by_name(name))
        {
            return Some(Zone::Iana(tz));
        }
        if let Some(zone) = self.defined_zone(name, local) {
            return Some(zone);
        }
        match self.default {
            Some(tz) => {
                self.warnings.insert(format!(
                    "Unknown time zone \"{name}\": its times are read in {}, the feed's own zone.",
                    tz.name()
                ));
                Some(Zone::Iana(tz))
            }
            None => {
                self.warnings.insert(format!(
                    "Unknown time zone \"{name}\": its times are shown at the same clock time \
                     in every zone."
                ));
                None
            }
        }
    }

    /// Step 4: a zone the feed defines. `None` if the block has no usable observance.
    fn defined_zone(&mut self, name: &str, local: NaiveDateTime) -> Option<Zone> {
        let defined = self.defined.get_mut(name)?;
        // 1. The hint.
        if let Some(tz) = defined.hint {
            return Some(Zone::Iana(tz));
        }
        let zone = defined
            .zone
            .get_or_insert_with(|| FeedZone::new(&defined.observances))
            .as_ref()?;

        // 2. Match by behaviour. All values of one year share one match, so a feed with
        // thousands of events compares each zone only a few times. The window starts at the
        // beginning of that year and runs three years.
        let year = local.year();
        let default = self.default;
        let matched = *self
            .matched
            .entry((name.to_owned(), year))
            .or_insert_with(|| {
                let from = NaiveDate::from_ymd_opt(year, 1, 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map(|t| t.and_utc().timestamp())
                    .unwrap_or_default();
                let candidates = zone.matching_zones(from, MATCH_MONTHS);
                match default {
                    Some(tz) if candidates.contains(&tz) => Some(tz),
                    _ => candidates.first().copied(),
                }
            });
        if let Some(tz) = matched {
            return Some(Zone::Iana(tz));
        }

        // 3. A fixed offset: the one the block gives at this time.
        self.warnings.insert(format!(
            "Time zone \"{name}\" matches no known zone: its times use fixed offsets, so \
             recurring events may be an hour off after a daylight-saving change."
        ));
        Some(Zone::Fixed(zone.offset_at(zone.to_instant(local))))
    }

    /// The warnings collected so far, sorted, each once.
    pub fn into_warnings(self) -> BTreeSet<String> {
        self.warnings
    }
}

/// A `TZID` with quotes and surrounding spaces removed.
fn clean(tzid: &str) -> &str {
    tzid.trim().trim_matches('"').trim()
}

/// Steps 1 to 3: an IANA name, a longer id ending in one, or a Windows name.
fn by_name(tzid: &str) -> Option<Tz> {
    let name = clean(tzid);
    iana(name)
        .or_else(|| {
            // Rust note: `match_indices` gives the byte position of every `/`, from the left,
            // so the first suffix that parses is the longest one.
            name.match_indices('/')
                .find_map(|(i, _)| iana(&name[i + 1..]))
        })
        .or_else(|| {
            WINDOWS_ZONES
                .iter()
                .find(|(windows, _)| windows.eq_ignore_ascii_case(name))
                .and_then(|(_, iana_name)| iana_name.parse().ok())
        })
}

fn iana(name: &str) -> Option<Tz> {
    clean(name).parse().ok()
}

/// A `STANDARD` or `DAYLIGHT` part. One without a readable `DTSTART` or offsets is logged and
/// left out.
fn observance(c: &Component<'_>) -> Option<Observance> {
    let read = || -> Option<Observance> {
        let start = local_time(prop(c, "DTSTART")?.val.as_str())?;
        let offset_from = utc_offset(prop(c, "TZOFFSETFROM")?.val.as_str())?;
        let offset_to = utc_offset(prop(c, "TZOFFSETTO")?.val.as_str())?;
        let rule = prop(c, "RRULE").map(|p| p.val.as_str().trim().to_owned());
        let dates = c
            .properties
            .iter()
            .filter(|p| named(p, "RDATE"))
            .flat_map(|p| p.val.as_str().split(','))
            .filter_map(local_time)
            .collect();
        Some(Observance {
            start,
            offset_from,
            offset_to,
            rule,
            dates,
        })
    };
    let result = read();
    if result.is_none() {
        log::warn!("skipping a time zone part without a readable start or offsets");
    }
    result
}

/// A local date-time such as `19701025T030000`.
pub(super) fn local_time(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value.trim(), "%Y%m%dT%H%M%S").ok()
}

/// A `TZOFFSETFROM` / `TZOFFSETTO` value: `+hhmm` or `+hhmmss`.
fn utc_offset(value: &str) -> Option<FixedOffset> {
    let value = value.trim();
    let (sign, digits) = match value.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };
    if !matches!(digits.len(), 4 | 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |i: usize| digits.get(i..i + 2).and_then(|d| d.parse::<i32>().ok());
    let seconds = part(0)? * 3600 + part(2)? * 60 + part(4).unwrap_or(0);
    FixedOffset::east_opt(sign * seconds)
}
