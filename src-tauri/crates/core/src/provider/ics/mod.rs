//! The ICS provider: read-only feeds (`docs/architecture.md` §7.6).
//!
//! A feed is one calendar, fetched whole. The cursor remembers the HTTP validators and a hash
//! of the body, so a feed that did not change costs a `304` or a hash comparison, not a parse.
//! A feed that did change is parsed into one `full` change set.

pub(super) mod parse;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tz_cases;
mod windows_zones;

use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, Next, PageToken, ProviderError,
    Reminders, RemoteCalendar, RemoteEvent, SourceContext, SyncCursor, http,
};
use crate::model::{Access, SourceKind};
use crate::secrets::Secret;

/// The `remote_id` of a feed's one calendar.
pub const CALENDAR_ID: &str = "feed";

/// The largest feed body that is read. Real feeds are a few megabytes at most; this keeps a
/// wrong URL from filling memory.
const MAX_FEED_BYTES: usize = 32 * 1024 * 1024;

/// The cursor: JSON with the HTTP validators and a SHA-256 of the body (§5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Cursor {
    etag: Option<String>,
    last_modified: Option<String>,
    /// Lower-case hex.
    hash: String,
}

/// The URL that is fetched for a subscription URL: `webcal://` and `webcals://` become
/// `https://`. Other URLs are returned as they are.
pub fn feed_url(url: &str) -> String {
    let url = url.trim();
    for scheme in ["webcals://", "webcal://"] {
        // Rust note: `get` instead of slicing with `[..n]`, which would panic if byte `n` fell
        // inside a multi-byte character.
        if url
            .get(..scheme.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
        {
            return format!("https://{}", &url[scheme.len()..]);
        }
    }
    url.to_owned()
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

    /// One read-only calendar, named after the source. Nothing is fetched for it.
    async fn list_calendars(
        &self,
        cx: &SourceContext,
    ) -> Result<Vec<RemoteCalendar>, ProviderError> {
        Ok(vec![RemoteCalendar {
            remote_id: CALENDAR_ID.to_owned(),
            name: cx.display_name.clone(),
            color: None,
            time_zone: None,
            access: Access::Read,
            is_primary: true,
            default_reminders: Vec::new(),
        }])
    }

    async fn pull(
        &self,
        cx: &SourceContext,
        _calendar: &CalendarRef,
        cursor: Option<&SyncCursor>,
        _page: Option<&PageToken>,
    ) -> Result<ChangeSet, ProviderError> {
        let Some(Secret::IcsUrl { url }) = &cx.secret else {
            return Err(ProviderError::Permanent(
                "the feed address is missing from the keychain".into(),
            ));
        };
        // A cursor this provider cannot read is treated as none: the feed is fetched whole.
        let previous: Option<Cursor> = cursor.and_then(|c| serde_json::from_str(&c.0).ok());

        let mut request = self.client.get(feed_url(url));
        if let Some(previous) = &previous {
            if let Some(etag) = &previous.etag {
                request = request.header(IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = &previous.last_modified {
                request = request.header(IF_MODIFIED_SINCE, last_modified);
            }
        }
        let response = request.send().await.map_err(request_error)?;

        let status = response.status();
        // Rust note: a "let chain" (Rust 2024) joins a condition and a pattern match in one
        // `if`. A 304 can only answer the conditional headers, which only a cursor adds.
        if status == StatusCode::NOT_MODIFIED
            && let Some(cursor) = cursor
        {
            return Ok(unchanged(cursor.clone()));
        }
        if !status.is_success() {
            return Err(status_error(status, http::retry_after(&response)));
        }

        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let (etag, last_modified) = (header(ETAG), header(LAST_MODIFIED));
        let body = read_body(response, self.max_bytes).await?;
        let next = Cursor {
            etag,
            last_modified,
            hash: content_hash(&body),
        };
        // Many servers ignore conditional requests; the same body is "no change" as well. The
        // new cursor is still returned, for validators the server may have started sending.
        let same = previous.is_some_and(|p| p.hash == next.hash);
        let next_cursor = SyncCursor(serde_json::to_string(&next).expect("cursor serialises"));
        if same {
            return Ok(unchanged(next_cursor));
        }

        // Parsing a large feed takes a while; it runs on a blocking thread so it does not hold
        // up the async runtime that other tasks share.
        let feed =
            tokio::task::spawn_blocking(move || parse::parse_feed(&String::from_utf8_lossy(&body)))
                .await
                .map_err(|e| ProviderError::Permanent(format!("parsing the feed failed: {e}")))?
                .map_err(|e| ProviderError::Permanent(e.to_string()))?;

        // A feed's alarms are the publisher's, not the user's: they stay off (§9).
        let upserts = feed
            .events
            .into_iter()
            .map(|event| RemoteEvent {
                reminders: Reminders::default(),
                ..event
            })
            .collect();
        Ok(ChangeSet {
            upserts,
            deletions: Vec::new(),
            full: true,
            next: Next::Done(next_cursor),
            warnings: feed.warnings,
        })
    }
}

/// SHA-256 of the body in hex, without its `DTSTAMP` lines.
///
/// `DTSTAMP` is when the file was generated, and Google sets it to the time of the request on
/// every event, so its feeds would never hash the same twice. The app stores nothing from it.
fn content_hash(body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    for line in body.split_inclusive(|&b| b == b'\n') {
        let is_dtstamp = line.get(..8).is_some_and(|start| {
            start.eq_ignore_ascii_case(b"DTSTAMP:") || start.eq_ignore_ascii_case(b"DTSTAMP;")
        });
        if !is_dtstamp {
            hasher.update(line);
        }
    }
    format!("{:x}", hasher.finalize())
}

/// "Nothing changed": no events, not full, so nothing is deleted.
fn unchanged(cursor: SyncCursor) -> ChangeSet {
    ChangeSet {
        upserts: Vec::new(),
        deletions: Vec::new(),
        full: false,
        next: Next::Done(cursor),
        warnings: Vec::new(),
    }
}

/// Reads the body, refusing it once it grows past `max` bytes.
async fn read_body(response: reqwest::Response, max: usize) -> Result<Vec<u8>, ProviderError> {
    http::read_body(response, max).await.map_err(|e| match e {
        http::BodyError::TooLarge => {
            ProviderError::Permanent(format!("the feed is larger than {max} bytes"))
        }
        http::BodyError::Request(e) => request_error(e),
    })
}

/// Classifies a failed request. The URL is never part of the message: it is a secret (§8).
fn request_error(e: reqwest::Error) -> ProviderError {
    if e.is_builder() {
        return ProviderError::Permanent("the feed address is not a valid URL".into());
    }
    let error = ProviderError::unanswered(&e);
    log::info!("feed request failed: {}", e.without_url());
    error
}

/// Classifies an HTTP status that is not a success.
fn status_error(status: StatusCode, retry_after: Option<Duration>) -> ProviderError {
    match status.as_u16() {
        // A private feed whose token was revoked answers like this.
        401 | 403 => ProviderError::Auth,
        408 | 429 | 500..=599 => ProviderError::Transient { retry_after },
        _ => ProviderError::Permanent(format!("the server answered HTTP {status}")),
    }
}
