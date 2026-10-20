//! The ICS provider: read-only feeds (`docs/architecture.md` §7.6).
//!
//! A feed is one calendar, fetched whole. The cursor remembers the HTTP validators and a hash
//! of the body, so a feed that did not change costs a `304` or a hash comparison, not a parse.
//! A feed that did change is parsed into one `full` change set.

mod parse;
#[cfg(test)]
mod tests;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, PageToken, ProviderError,
    RemoteCalendar, SourceContext, SyncCursor, http,
};
use crate::model::SourceKind;

/// The `remote_id` of a feed's one calendar.
pub const CALENDAR_ID: &str = "feed";

/// The largest feed body that is read. Real feeds are a few megabytes at most; this keeps a
/// wrong URL from filling memory.
const MAX_FEED_BYTES: usize = 32 * 1024 * 1024;

/// The cursor: JSON with the HTTP validators and a SHA-256 of the body (§5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by `pull`, which is not written yet")
)]
struct Cursor {
    etag: Option<String>,
    last_modified: Option<String>,
    /// Lower-case hex.
    hash: String,
}

/// The URL that is fetched for a subscription URL: `webcal://` and `webcals://` become
/// `https://`. Other URLs are returned as they are.
pub fn feed_url(url: &str) -> String {
    let _ = url;
    todo!()
}

pub struct IcsProvider {
    client: reqwest::Client,
    max_bytes: usize,
}

impl IcsProvider {
    pub fn new() -> reqwest::Result<IcsProvider> {
        Ok(IcsProvider {
            client: http::client()?,
            max_bytes: MAX_FEED_BYTES,
        })
    }
}

#[async_trait]
impl CalendarProvider for IcsProvider {
    fn kind(&self) -> SourceKind {
        SourceKind::Ics
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: false,
            incremental_sync: false,
            reminders: false,
        }
    }

    async fn list_calendars(
        &self,
        cx: &SourceContext,
    ) -> Result<Vec<RemoteCalendar>, ProviderError> {
        let _ = cx;
        todo!()
    }

    async fn pull(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        cursor: Option<&SyncCursor>,
        page: Option<&PageToken>,
    ) -> Result<ChangeSet, ProviderError> {
        let _ = (cx, calendar, cursor, page, &self.client, self.max_bytes);
        todo!()
    }
}
