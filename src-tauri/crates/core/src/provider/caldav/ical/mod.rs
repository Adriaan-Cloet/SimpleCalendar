//! Writing events as iCalendar objects (`docs/architecture.md` §7.8).
//!
//! An object is read into components and raw lines, changed, and written back. A line this app
//! does not own is never parsed further, so it goes back to the server exactly as it came:
//! attendees, categories, other apps' `X-` lines, alarms a reminder cannot hold. Only the lines
//! of the fields that changed are rewritten.
//!
//! Finding the right `VEVENT` and the form of its times is left to the ICS parser
//! (`parse::layout`), which resolves time zones the same way a pull does.

#[cfg(test)]
mod tests;
mod zone;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime};
use chrono_tz::Tz;

use crate::provider::ics::parse::{self, Layout, Role};
use crate::provider::{EventData, Field, FieldSet, ReminderMethod};

const PRODID: &str = "-//SimpleCalendar//EN";

/// Lines that hold an event's recurrence (RFC 5545 §3.8.5).
const RECURRENCE: [&str; 4] = ["RRULE", "RDATE", "EXDATE", "EXRULE"];

/// Why an object could not be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditError {
    /// The text is not an iCalendar object.
    Unreadable,
    /// The object does not hold the event: no master, for a change to one.
    NoEvent,
}

/// One unfolded content line, as it was read: `NAME;PARAM=value:value`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line(String);

impl Line {
    fn name(&self) -> &str {
        let end = self.0.find([';', ':']).unwrap_or(self.0.len());
        &self.0[..end]
    }

    fn is(&self, name: &str) -> bool {
        self.name().eq_ignore_ascii_case(name)
    }

    /// The parameters (each with its leading `;`) and the value, split at the first `:` that is
    /// not inside a quoted parameter value.
    fn split(&self) -> (&str, &str) {
        let start = self.name().len();
        let mut quoted = false;
        for (i, ch) in self.0[start..].char_indices() {
            match ch {
                '"' => quoted = !quoted,
                ':' if !quoted => {
                    return (&self.0[start..start + i], &self.0[start + i + 1..]);
                }
                _ => {}
            }
        }
        (&self.0[start..], "")
    }

    fn value(&self) -> &str {
        self.split().1
    }

    /// The value of parameter `key`, quotes removed.
    fn param(&self, key: &str) -> Option<&str> {
        let (params, _) = self.split();
        let mut quoted = false;
        let mut parts = Vec::new();
        let mut from = 0;
        for (i, ch) in params.char_indices() {
            match ch {
                '"' => quoted = !quoted,
                ';' if !quoted => {
                    parts.push(&params[from..i]);
                    from = i + 1;
                }
                _ => {}
            }
        }
        parts.push(&params[from..]);
        parts.into_iter().find_map(|part| {
            let (k, v) = part.split_once('=')?;
            k.trim()
                .eq_ignore_ascii_case(key)
                .then(|| v.trim().trim_matches('"'))
        })
    }
}

/// Where a `VEVENT` is: the index of its `VCALENDAR`, and its index in there.
type At = (usize, usize);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Line(Line),
    Component(Component),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Component {
    /// In upper case.
    name: String,
    nodes: Vec<Node>,
}

impl Component {
    fn new(name: &str, lines: impl IntoIterator<Item = String>) -> Component {
        Component {
            name: name.to_owned(),
            nodes: lines.into_iter().map(|l| Node::Line(Line(l))).collect(),
        }
    }

    fn lines(&self) -> impl Iterator<Item = &Line> {
        self.nodes.iter().filter_map(|n| match n {
            Node::Line(line) => Some(line),
            Node::Component(_) => None,
        })
    }

    fn first(&self, name: &str) -> Option<&Line> {
        self.lines().find(|l| l.is(name))
    }

    fn components(&self, name: &str) -> impl Iterator<Item = &Component> {
        self.nodes.iter().filter_map(move |n| match n {
            Node::Component(c) if c.name == name => Some(c),
            _ => None,
        })
    }

    /// Replaces every line called one of `names` by `new`, where the first of them was; without
    /// one, `new` goes after the last line, before the sub-components.
    fn replace(&mut self, names: &[&str], new: Vec<String>) {
        let matches = |n: &Node| matches!(n, Node::Line(l) if names.iter().any(|name| l.is(name)));
        let at = self.nodes.iter().position(matches).unwrap_or_else(|| {
            self.nodes
                .iter()
                .position(|n| matches!(n, Node::Component(_)))
                .unwrap_or(self.nodes.len())
        });
        let before = self.nodes[..at].iter().filter(|n| matches(n)).count();
        self.nodes.retain(|n| !matches(n));
        let at = at - before;
        self.nodes
            .splice(at..at, new.into_iter().map(|l| Node::Line(Line(l))));
    }

    /// Adds `line` after the last line called one of `names`, else after the last line.
    fn add_after(&mut self, names: &[&str], line: String) {
        let at = self
            .nodes
            .iter()
            .rposition(|n| matches!(n, Node::Line(l) if names.iter().any(|name| l.is(name))))
            .or_else(|| self.nodes.iter().rposition(|n| matches!(n, Node::Line(_))))
            .map_or(0, |i| i + 1);
        self.nodes.insert(at, Node::Line(Line(line)));
    }

    fn write(&self, out: &mut String) {
        push_line(out, &format!("BEGIN:{}", self.name));
        for node in &self.nodes {
            match node {
                Node::Line(line) => push_line(out, &line.0),
                Node::Component(c) => c.write(out),
            }
        }
        push_line(out, &format!("END:{}", self.name));
    }

    fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

/// Appends `line` folded at 75 bytes (RFC 5545 §3.1), never inside a character, with CRLF.
fn push_line(out: &mut String, line: &str) {
    let mut rest = line;
    let mut limit = 75;
    loop {
        if rest.len() <= limit {
            out.push_str(rest);
            out.push_str("\r\n");
            return;
        }
        let mut cut = limit;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push_str(&rest[..cut]);
        out.push_str("\r\n ");
        rest = &rest[cut..];
        // The space that starts a continuation line counts too.
        limit = 74;
    }
}

/// A whole resource: its `VCALENDAR`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Object {
    roots: Vec<Component>,
}

impl Object {
    /// `None` if the text is not iCalendar: a line outside a component, or components that do
    /// not close.
    pub fn parse(text: &str) -> Option<Object> {
        let unfolded = text
            .trim_start_matches('\u{feff}')
            .replace("\r\n", "\n")
            .replace("\n ", "")
            .replace("\n\t", "");
        let mut stack: Vec<Component> = Vec::new();
        let mut roots = Vec::new();
        for raw in unfolded.split('\n') {
            if raw.trim().is_empty() {
                continue;
            }
            let line = Line(raw.trim_end_matches('\r').to_owned());
            if line.is("BEGIN") {
                stack.push(Component {
                    name: line.value().trim().to_ascii_uppercase(),
                    nodes: Vec::new(),
                });
            } else if line.is("END") {
                let done = stack.pop()?;
                if !done.name.eq_ignore_ascii_case(line.value().trim()) {
                    return None;
                }
                match stack.last_mut() {
                    Some(parent) => parent.nodes.push(Node::Component(done)),
                    None => roots.push(done),
                }
            } else {
                stack.last_mut()?.nodes.push(Node::Line(line));
            }
        }
        (stack.is_empty() && roots.iter().any(|r| r.name == "VCALENDAR"))
            .then_some(Object { roots })
    }

    pub fn to_text(&self) -> String {
        self.roots.iter().map(Component::to_text).collect()
    }

    /// Where each `VEVENT` is, in the order `parse::layout` lists them: by `VCALENDAR`, then
    /// by position in it.
    fn vevents(&self) -> Vec<At> {
        let mut found = Vec::new();
        for (r, root) in self.roots.iter().enumerate() {
            if root.name != "VCALENDAR" {
                continue;
            }
            for (n, node) in root.nodes.iter().enumerate() {
                if matches!(node, Node::Component(c) if c.name == "VEVENT") {
                    found.push((r, n));
                }
            }
        }
        found
    }

    fn at(&mut self, (r, n): At) -> &mut Component {
        match &mut self.roots[r].nodes[n] {
            Node::Component(c) => c,
            Node::Line(_) => unreachable!("positions come from `vevents`"),
        }
    }

    /// Every `VEVENT` with what the parser makes of it. `Unreadable` if the two disagree.
    fn events(&self, text: &str) -> Result<Vec<(At, Layout)>, EditError> {
        let layout = parse::layout(text).ok_or(EditError::Unreadable)?;
        let positions = self.vevents();
        if positions.len() != layout.len() {
            return Err(EditError::Unreadable);
        }
        Ok(positions.into_iter().zip(layout).collect())
    }

    /// What every write ends with: each zone a time names is defined, and no attendee is mailed
    /// by the server (§7.8).
    fn finish(&mut self, year: i32) {
        for root in &mut self.roots {
            for node in &mut root.nodes {
                if let Node::Component(event) = node
                    && event.name == "VEVENT"
                {
                    not_scheduled(event);
                }
            }
            define_zones(root, year);
        }
    }
}

/// A new resource holding `event` as a single event or series, with `uid` as its `UID`.
pub(super) fn create(uid: &str, event: &EventData, now: i64) -> String {
    let stamp = utc_text(now);
    let mut vevent = Component::new(
        "VEVENT",
        [
            format!("UID:{uid}"),
            format!("DTSTAMP:{stamp}"),
            format!("CREATED:{stamp}"),
            format!("LAST-MODIFIED:{stamp}"),
            "SEQUENCE:0".to_owned(),
        ],
    );
    write_fields(&mut vevent, event, &Field::ALL.into_iter().collect());
    let mut calendar = Component::new(
        "VCALENDAR",
        ["VERSION:2.0".to_owned(), format!("PRODID:{PRODID}")],
    );
    calendar.nodes.push(Node::Component(vevent));
    let mut object = Object {
        roots: vec![calendar],
    };
    object.finish(year_of(event.start_utc));
    object.to_text()
}

/// The resource `text` with `desired` written into the series or single event (`occurrence =
/// None`), or into the occurrence whose original start is `occurrence`. Only the `changed`
/// fields are rewritten; an occurrence that was no exception yet becomes one, a copy of the
/// series with every field written.
///
/// New times or a new rule for a series drop its exceptions: they name occurrences of the old
/// series. The app drops its own copies of them too (`service/series.rs`).
pub(super) fn update(
    text: &str,
    occurrence: Option<i64>,
    desired: &EventData,
    changed: &FieldSet,
    now: i64,
) -> Result<String, EditError> {
    let mut object = Object::parse(text).ok_or(EditError::Unreadable)?;
    let events = object.events(text)?;
    let master = events.iter().find(|(_, l)| l.role == Role::Master);

    match occurrence {
        None => {
            let (at, _) = master.ok_or(EditError::NoEvent)?;
            let event = object.at(*at);
            write_fields(event, desired, changed);
            touch(event, now);
            if changed.contains(&Field::Times) || changed.contains(&Field::Recurrence) {
                // Backwards, so the positions still to remove stay valid.
                for ((r, n), _) in events.iter().rev().filter(|(_, l)| l.role != Role::Master) {
                    object.roots[*r].nodes.remove(*n);
                }
            }
        }
        Some(start) => {
            let mut fields = changed.clone();
            fields.remove(&Field::Recurrence);
            match events
                .iter()
                .find(|(_, l)| l.role == Role::Exception(start))
            {
                Some((at, _)) => {
                    let event = object.at(*at);
                    write_fields(event, desired, &fields);
                    touch(event, now);
                }
                None => {
                    let ((r, n), layout) = master.ok_or(EditError::NoEvent)?;
                    let exception = exception_of(object.at((*r, *n)), start, layout);
                    let mut exception = exception;
                    let all = Field::ALL
                        .into_iter()
                        .filter(|&f| f != Field::Recurrence)
                        .collect();
                    write_fields(&mut exception, desired, &all);
                    touch(&mut exception, now);
                    let last = events
                        .iter()
                        .filter(|((root, _), _)| root == r)
                        .map(|((_, n), _)| *n)
                        .max()
                        .unwrap_or(*n);
                    object.roots[*r]
                        .nodes
                        .insert(last + 1, Node::Component(exception));
                }
            }
        }
    }
    object.finish(year_of(desired.start_utc));
    Ok(object.to_text())
}

/// The resource `text` without the occurrence at `original_start`: its exception, if any, goes,
/// and the series gets an `EXDATE` for it. `None` if it was excluded already.
pub(super) fn exclude(
    text: &str,
    original_start: i64,
    now: i64,
) -> Result<Option<String>, EditError> {
    let mut object = Object::parse(text).ok_or(EditError::Unreadable)?;
    let events = object.events(text)?;
    let ((r, n), layout) = events
        .iter()
        .find(|(_, l)| l.role == Role::Master)
        .ok_or(EditError::NoEvent)?;

    let master = object.at((*r, *n));
    let dtstart = master.first("DTSTART").cloned().ok_or(EditError::NoEvent)?;
    let exdate = Line(original_line("EXDATE", original_start, &dtstart, layout));
    let excluded = master.lines().any(|l| {
        l.is("EXDATE")
            && l.param("TZID") == exdate.param("TZID")
            && l.value().split(',').any(|v| v.trim() == exdate.value())
    });
    let exceptions: Vec<At> = events
        .iter()
        .filter(|(_, l)| l.role == Role::Exception(original_start))
        .map(|(at, _)| *at)
        .collect();
    if excluded && exceptions.is_empty() {
        return Ok(None);
    }
    if !excluded {
        master.add_after(&RECURRENCE, exdate.0);
    }
    touch(master, now);
    for (r, n) in exceptions.into_iter().rev() {
        object.roots[r].nodes.remove(n);
    }
    object.finish(year_of(original_start));
    Ok(Some(object.to_text()))
}

/// A copy of `master` as the exception for its occurrence at `original_start`: everything but
/// its times and recurrence, which `write_fields` and the new `RECURRENCE-ID` replace.
fn exception_of(master: &Component, original_start: i64, layout: &Layout) -> Component {
    let mut exception = master.clone();
    let dtstart = master.first("DTSTART").cloned();
    exception.replace(&RECURRENCE, Vec::new());
    exception.replace(&["RECURRENCE-ID"], Vec::new());
    let id = dtstart.map(|d| original_line("RECURRENCE-ID", original_start, &d, layout));
    // Next to the start, where readers look for it.
    exception.replace(&["DTSTART"], id.into_iter().collect());
    exception
}

/// Writes the `fields` of `event` into `vevent`.
fn write_fields(vevent: &mut Component, event: &EventData, fields: &FieldSet) {
    for field in fields {
        match field {
            Field::Title => vevent.replace(&["SUMMARY"], text_line("SUMMARY", Some(&event.title))),
            Field::Description => {
                // An HTML copy of the old text would contradict the new one.
                vevent.replace(&["X-ALT-DESC"], Vec::new());
                vevent.replace(
                    &["DESCRIPTION"],
                    text_line("DESCRIPTION", event.description.as_deref()),
                );
            }
            Field::Location => vevent.replace(
                &["LOCATION"],
                text_line("LOCATION", event.location.as_deref()),
            ),
            Field::Times => vevent.replace(
                &["DTSTART", "DTEND", "DURATION"],
                vec![
                    time_line(
                        "DTSTART",
                        event.start_utc,
                        event.all_day,
                        event.start_tz.as_deref(),
                    ),
                    time_line(
                        "DTEND",
                        event.end_utc,
                        event.all_day,
                        event.end_tz.as_deref(),
                    ),
                ],
            ),
            Field::Recurrence => vevent.replace(
                &RECURRENCE,
                event
                    .recurrence
                    .iter()
                    .flat_map(|r| r.lines())
                    .filter(|l| !l.trim().is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            Field::Reminders => write_alarms(vevent, event),
        }
    }
}

/// Makes the alarms of `vevent` match the event's reminders. An alarm that is still wanted is
/// kept as it is, with whatever else it holds; one a reminder cannot hold is always kept.
fn write_alarms(vevent: &mut Component, event: &EventData) {
    let mut wanted = if event.reminders.use_default {
        Vec::new()
    } else {
        event.reminders.overrides.clone()
    };
    vevent.nodes.retain(|node| {
        let Node::Component(alarm) = node else {
            return true;
        };
        if alarm.name != "VALARM" {
            return true;
        }
        match parse::alarm_text(&alarm.to_text()) {
            None => true,
            Some(reminder) => match wanted.iter().position(|w| *w == reminder) {
                Some(i) => {
                    wanted.remove(i);
                    true
                }
                None => false,
            },
        }
    });
    let label = if event.title.trim().is_empty() {
        "Reminder"
    } else {
        event.title.as_str()
    };
    for reminder in wanted {
        let trigger = format!("TRIGGER:-PT{}M", reminder.minutes_before);
        let lines = match reminder.method {
            ReminderMethod::Popup => vec![
                "ACTION:DISPLAY".to_owned(),
                format!("DESCRIPTION:{}", escape(label)),
                trigger,
            ],
            ReminderMethod::Email => vec![
                "ACTION:EMAIL".to_owned(),
                format!("SUMMARY:{}", escape(label)),
                format!("DESCRIPTION:{}", escape(label)),
                trigger,
            ],
        };
        vevent
            .nodes
            .push(Node::Component(Component::new("VALARM", lines)));
    }
}

/// Marks a change: a higher `SEQUENCE` and new stamps (RFC 5545 §3.8.7).
fn touch(vevent: &mut Component, now: i64) {
    let sequence = vevent
        .first("SEQUENCE")
        .and_then(|l| l.value().trim().parse::<u32>().ok())
        .map_or(0, |s| s + 1);
    vevent.replace(&["SEQUENCE"], vec![format!("SEQUENCE:{sequence}")]);
    vevent.replace(&["DTSTAMP"], vec![format!("DTSTAMP:{}", utc_text(now))]);
    vevent.replace(
        &["LAST-MODIFIED"],
        vec![format!("LAST-MODIFIED:{}", utc_text(now))],
    );
}

/// Asks the server not to mail the attendees (RFC 6638 §7.1), as the app never does (§7.8). An
/// attendee that already says who schedules it is left alone.
fn not_scheduled(vevent: &mut Component) {
    for node in &mut vevent.nodes {
        if let Node::Line(line) = node
            && line.is("ATTENDEE")
            && line.param("SCHEDULE-AGENT").is_none()
        {
            let name = line.name().len();
            line.0.insert_str(name, ";SCHEDULE-AGENT=CLIENT");
        }
    }
}

/// Adds a `VTIMEZONE` for every `TZID` the events name that is an IANA zone and not defined
/// yet (RFC 5545 §3.6.5). Its rules are those of `year`.
fn define_zones(calendar: &mut Component, year: i32) {
    if calendar.name != "VCALENDAR" {
        return;
    }
    let defined: Vec<String> = calendar
        .components("VTIMEZONE")
        .filter_map(|z| z.first("TZID"))
        .map(|l| l.value().trim().to_owned())
        .collect();
    let mut named: Vec<Tz> = Vec::new();
    for event in calendar.components("VEVENT") {
        for line in event.lines() {
            if let Some(tzid) = line.param("TZID")
                && !defined.iter().any(|d| d == tzid)
                && let Ok(tz) = tzid.parse::<Tz>()
                && !named.contains(&tz)
            {
                named.push(tz);
            }
        }
    }
    let at = calendar
        .nodes
        .iter()
        .position(|n| matches!(n, Node::Component(c) if c.name == "VEVENT"))
        .unwrap_or(calendar.nodes.len());
    for (i, tz) in named.into_iter().enumerate() {
        calendar
            .nodes
            .insert(at + i, Node::Component(zone::vtimezone(tz, year)));
    }
}

/// A text property, or none for an empty text.
fn text_line(name: &str, value: Option<&str>) -> Vec<String> {
    value
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("{name}:{}", escape(v)))
        .into_iter()
        .collect()
}

/// Escapes a text value (RFC 5545 §3.3.11).
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// A `DTSTART` or `DTEND` for a stored time (§4): a date for an all-day event, a local time
/// with its IANA `TZID`, a UTC time for UTC or a fixed offset, or a floating time.
fn time_line(name: &str, stored: i64, all_day: bool, zone: Option<&str>) -> String {
    if all_day {
        return format!("{name};VALUE=DATE:{}", naive(stored).format("%Y%m%d"));
    }
    match zone {
        None => format!("{name}:{}", local_text(naive(stored))),
        Some(zone) => match zone.parse::<Tz>() {
            Ok(tz) if tz != Tz::UTC => {
                let local = instant(stored).with_timezone(&tz).naive_local();
                format!("{name};TZID={}:{}", tz.name(), local_text(local))
            }
            _ => format!("{name}:{}", utc_text(stored)),
        },
    }
}

/// A `RECURRENCE-ID` or `EXDATE` for the occurrence at `original_start`, in the form of the
/// series' `DTSTART` (RFC 5545 §3.8.4.4), so the server matches it to that occurrence.
fn original_line(name: &str, original_start: i64, dtstart: &Line, layout: &Layout) -> String {
    if layout.all_day {
        return format!(
            "{name};VALUE=DATE:{}",
            naive(original_start).format("%Y%m%d")
        );
    }
    if dtstart.value().trim().ends_with(['Z', 'z']) {
        return format!("{name}:{}", utc_text(original_start));
    }
    let local = match layout.start_zone.as_deref() {
        None => naive(original_start),
        Some(zone) => match (zone.parse::<Tz>(), zone.parse::<FixedOffset>()) {
            (Ok(tz), _) => instant(original_start).with_timezone(&tz).naive_local(),
            (_, Ok(offset)) => instant(original_start).with_timezone(&offset).naive_local(),
            _ => naive(original_start),
        },
    };
    match dtstart.param("TZID") {
        Some(tzid) if tzid.contains([':', ';', ',']) => {
            format!("{name};TZID=\"{tzid}\":{}", local_text(local))
        }
        Some(tzid) => format!("{name};TZID={tzid}:{}", local_text(local)),
        // Floating, or in the calendar's zone (`X-WR-TIMEZONE`).
        None => format!("{name}:{}", local_text(local)),
    }
}

fn instant(stored: i64) -> DateTime<chrono::Utc> {
    DateTime::from_timestamp(stored, 0).unwrap_or_default()
}

/// A stored wall-clock value (floating, all-day) as the time it names.
fn naive(stored: i64) -> NaiveDateTime {
    instant(stored).naive_utc()
}

fn local_text(local: NaiveDateTime) -> String {
    local.format("%Y%m%dT%H%M%S").to_string()
}

fn utc_text(stored: i64) -> String {
    instant(stored).format("%Y%m%dT%H%M%SZ").to_string()
}

fn year_of(stored: i64) -> i32 {
    instant(stored).year()
}
