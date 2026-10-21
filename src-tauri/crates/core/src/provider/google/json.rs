//! Google Calendar API v3 resources, and how they become the provider's types (§7.5).
//!
//! Pure: no HTTP. Only the fields the app stores are read; serde skips the rest.

use chrono::{DateTime, NaiveDate};
use serde::Deserialize;

use crate::model::{Access, EventStatus};
use crate::provider::{
    Reminder, ReminderMethod, Reminders, RemoteCalendar, RemoteEvent, SeriesRole,
};

/// One page of `calendarList.list`.
//
// Rust note: `rename_all = "camelCase"` maps `next_page_token` to Google's `nextPageToken`.
// `#[serde(default)]` gives an empty `Vec` when Google leaves `items` out.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CalendarListPage {
    #[serde(default)]
    pub items: Vec<CalendarEntry>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CalendarEntry {
    id: String,
    summary: Option<String>,
    /// The name the user gave a calendar someone shared with them.
    summary_override: Option<String>,
    background_color: Option<String>,
    time_zone: Option<String>,
    access_role: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    default_reminders: Vec<GoogleReminder>,
}

impl CalendarEntry {
    /// The calendar as the engine stores it, or `None` for one the app cannot show: with only
    /// free/busy access, Google gives no titles and refuses most event fields. `can_write` is
    /// whether the provider writes at all: a calendar is writable only if both agree (§4).
    pub fn into_remote(self, can_write: bool) -> Option<RemoteCalendar> {
        let access = match self.access_role.as_str() {
            "owner" | "writer" if can_write => Access::Write,
            "owner" | "writer" => Access::Read,
            "reader" => Access::Read,
            _ => return None,
        };
        Some(RemoteCalendar {
            name: self
                .summary_override
                .or(self.summary)
                .unwrap_or_else(|| self.id.clone()),
            remote_id: self.id,
            color: self.background_color.filter(|c| is_hex_colour(c)),
            time_zone: self.time_zone,
            access,
            is_primary: self.primary,
            default_reminders: self
                .default_reminders
                .iter()
                .filter(|r| r.method == "popup")
                .filter_map(|r| u32::try_from(r.minutes).ok())
                .collect(),
        })
    }
}

/// One page of `events.list`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EventsPage {
    #[serde(default)]
    pub items: Vec<GoogleEvent>,
    pub next_page_token: Option<String>,
    pub next_sync_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GoogleEvent {
    id: String,
    status: Option<String>,
    etag: Option<String>,
    #[serde(rename = "iCalUID")]
    ical_uid: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    location: Option<String>,
    color_id: Option<String>,
    start: Option<GoogleTime>,
    end: Option<GoogleTime>,
    recurrence: Option<Vec<String>>,
    recurring_event_id: Option<String>,
    original_start_time: Option<GoogleTime>,
    organizer: Option<Organizer>,
    /// Absent means false, as documented.
    #[serde(default)]
    guests_can_modify: bool,
    #[serde(default)]
    locked: bool,
    event_type: Option<String>,
    reminders: Option<GoogleReminders>,
    updated: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Organizer {
    /// Whether the organizer is the calendar this copy of the event is on. Absent means false.
    #[serde(rename = "self", default)]
    is_self: bool,
}

/// `start`, `end` or `originalStartTime`: a `date` for all-day events, else a `dateTime` with
/// an offset and usually a `timeZone`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleTime {
    date: Option<String>,
    date_time: Option<String>,
    time_zone: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleReminders {
    #[serde(default)]
    use_default: bool,
    #[serde(default)]
    overrides: Vec<GoogleReminder>,
}

#[derive(Debug, Deserialize)]
struct GoogleReminder {
    method: String,
    minutes: i64,
}

/// What one entry of `events.list` means for the local copy.
#[derive(Debug, Clone, PartialEq, Eq)]
// Rust note: the event is boxed (kept on the heap) so a `Change` is small either way; without
// it every `Delete` would take as much room as a whole event.
pub(super) enum Change {
    Upsert(Box<RemoteEvent>),
    /// The event, or the whole series, was deleted.
    Delete(String),
}

/// A time as stored (§4): an instant, or for a date its midnight written as UTC.
#[derive(Clone)]
struct Stored {
    value: i64,
    all_day: bool,
    /// IANA name, or the offset of `dateTime` when Google names no zone. `None` for dates.
    zone: Option<String>,
}

impl GoogleTime {
    /// `fallback_zone` is the calendar's zone: Google gives events without their own zone the
    /// zone of their calendar (§7.5).
    fn stored(&self, fallback_zone: Option<&str>) -> Result<Stored, String> {
        if let Some(date) = &self.date {
            let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| format!("the date {date:?} cannot be read"))?;
            return Ok(Stored {
                value: date.and_time(chrono::NaiveTime::MIN).and_utc().timestamp(),
                all_day: true,
                zone: None,
            });
        }
        let Some(text) = &self.date_time else {
            return Err("a time has neither a date nor a dateTime".into());
        };
        let instant = DateTime::parse_from_rfc3339(text)
            .map_err(|_| format!("the time {text:?} cannot be read"))?;
        let zone = self
            .time_zone
            .clone()
            .or_else(|| fallback_zone.map(str::to_owned))
            // Without any zone the offset still gives the right instant (`TimeKind::Offset`).
            .unwrap_or_else(|| instant.format("%:z").to_string());
        Ok(Stored {
            value: instant.timestamp(),
            all_day: false,
            zone: Some(zone),
        })
    }
}

impl GoogleEvent {
    /// What this entry means. `calendar_zone` is the zone of the calendar it came from.
    pub fn into_change(self, calendar_zone: Option<&str>) -> Result<Change, String> {
        let cancelled = self.status.as_deref() == Some("cancelled");
        // A cancelled entry without a series is a real deletion. One with a series is a
        // cancelled occurrence, stored as an exception (§7.5).
        if cancelled && self.recurring_event_id.is_none() {
            return Ok(Change::Delete(self.id));
        }

        let original = match &self.original_start_time {
            Some(time) => Some(time.stored(calendar_zone)?),
            None => None,
        };
        let series = match (&self.recurring_event_id, &original, &self.recurrence) {
            (Some(master), Some(original), _) => SeriesRole::Exception {
                master_remote_id: master.clone(),
                original_start_utc: original.value,
            },
            (Some(_), None, _) => {
                return Err(format!(
                    "the occurrence {:?} does not say which one it replaces",
                    self.id
                ));
            }
            (None, _, Some(lines)) if !lines.is_empty() => SeriesRole::Master {
                recurrence: lines.join("\n"),
            },
            _ => SeriesRole::Single,
        };

        // A cancelled occurrence often comes without its own times: it then takes the
        // original start, which is all the expansion needs to drop it.
        let (start, end) = match (&self.start, &self.end) {
            (Some(start), Some(end)) => (start.stored(calendar_zone)?, end.stored(calendar_zone)?),
            _ => match original {
                Some(original) if cancelled => (original.clone(), original),
                _ => return Err(format!("the event {:?} has no start or end", self.id)),
            },
        };

        // Google restricts changes to locked copies and to special types such as birthdays or
        // working locations. v0.2 treats them as read-only through the same flags (§13 point 9).
        let restricted = self.locked
            || self
                .event_type
                .as_deref()
                .is_some_and(|kind| kind != "default");
        let reminders = match self.reminders {
            None => Reminders::default(),
            Some(r) => Reminders {
                use_default: r.use_default,
                overrides: r
                    .overrides
                    .iter()
                    .filter_map(|o| {
                        Some(Reminder {
                            method: match o.method.as_str() {
                                "popup" => ReminderMethod::Popup,
                                "email" => ReminderMethod::Email,
                                _ => return None,
                            },
                            minutes_before: u32::try_from(o.minutes).ok()?,
                        })
                    })
                    .collect(),
            },
        };

        Ok(Change::Upsert(Box::new(RemoteEvent {
            remote_id: self.id,
            ical_uid: self.ical_uid,
            status: match self.status.as_deref() {
                Some("cancelled") => EventStatus::Cancelled,
                Some("tentative") => EventStatus::Tentative,
                _ => EventStatus::Confirmed,
            },
            title: self.summary.unwrap_or_default(),
            description: self.description,
            location: self.location,
            color: self
                .color_id
                .as_deref()
                .and_then(event_colour)
                .map(str::to_owned),
            all_day: start.all_day,
            start_utc: start.value,
            // Google never sends an end before the start; this keeps the table's check safe.
            end_utc: end.value.max(start.value),
            start_tz: start.zone,
            end_tz: end.zone,
            series,
            remote_updated_at: self
                .updated
                .as_deref()
                .and_then(|u| DateTime::parse_from_rfc3339(u).ok())
                .map(|u| u.timestamp()),
            etag: self.etag,
            is_organizer: self.organizer.is_some_and(|o| o.is_self) && !restricted,
            guests_can_modify: self.guests_can_modify && !restricted,
            reminders,
        })))
    }
}

/// The colour of an event's `colorId`, as Google Calendar shows it today (Lavender, Sage, ...).
/// The API's own `colors` resource still lists an older, paler palette.
fn event_colour(id: &str) -> Option<&'static str> {
    Some(match id {
        "1" => "#7986cb",
        "2" => "#33b679",
        "3" => "#8e24aa",
        "4" => "#e67c73",
        "5" => "#f6bf26",
        "6" => "#f4511e",
        "7" => "#039be5",
        "8" => "#616161",
        "9" => "#3f51b5",
        "10" => "#0b8043",
        "11" => "#d50000",
        _ => return None,
    })
}

fn is_hex_colour(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
