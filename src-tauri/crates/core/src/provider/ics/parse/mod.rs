//! Turning a feed's text into events (`docs/architecture.md` §7.6, §7.7).
//!
//! Only `icalendar`'s low-level parser is used: it gives every property with its raw value and
//! parameters, and every component with its sub-components, `VTIMEZONE` parts included. The
//! typed API would hide the raw `TZID`s that §7.7 needs (§13 point 3).

#[cfg(test)]
mod tests;

use crate::provider::RemoteEvent;

/// Why a feed could not be read at all. A single broken event is skipped instead.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by `pull`, which is not written yet")
)]
#[error("not an iCalendar feed: {0}")]
pub(super) struct ParseError(String);

/// Every event in the feed. Events without a `UID` or a usable `DTSTART` are logged and left
/// out, so one broken event never hides the rest.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by `pull`, which is not written yet")
)]
pub(super) fn parse_feed(text: &str) -> Result<Vec<RemoteEvent>, ParseError> {
    let _ = text;
    todo!()
}
