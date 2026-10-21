//! The Google Calendar provider: reads calendars and events (`docs/architecture.md` §7.5).
//!
//! Writing comes with the outbox (#18). Tokens come from `GoogleAuth`, which the sign-in in
//! the service shares (§8).

mod json;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;

use self::json::{CalendarListPage, Change, EventsPage};
use crate::google::GoogleAuth;
use crate::model::SourceKind;
use crate::provider::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, Next, PageToken, ProviderError,
    RemoteCalendar, SourceContext, SyncCursor,
};

/// Entries per page. Google allows up to 250 for the calendar list and 2500 for events; 250
/// keeps one page, and the transaction that stores it, small.
const PAGE_SIZE: &str = "250";

/// Whether this provider writes. Until Google write (#18) it does not, so calendars the
/// account may change are still stored as read-only, and no edit button appears (§4).
const WRITES: bool = false;

pub struct GoogleProvider {
    auth: Arc<GoogleAuth>,
}

impl GoogleProvider {
    pub fn new(auth: Arc<GoogleAuth>) -> GoogleProvider {
        GoogleProvider { auth }
    }

    /// `path` under the Calendar API, with each segment percent-encoded: calendar ids contain
    /// `@` and `#`.
    fn url(&self, path: &[&str], query: &[(&str, &str)]) -> Result<Url, ProviderError> {
        let mut url = self.auth.endpoints().api.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                ProviderError::Permanent("the Google API address is invalid".into())
            })?;
            // The base ends in a slash, which leaves an empty last segment.
            segments.pop_if_empty();
            segments.extend(path);
        }
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    /// GETs `url` with the source's access token. A 401 drops the token and tries once more
    /// with a refreshed one; the second 401 means signing in again.
    async fn get<T: DeserializeOwned>(
        &self,
        cx: &SourceContext,
        url: Url,
    ) -> Result<T, ProviderError> {
        for attempt in 0..2 {
            let token = self.auth.access_token(cx).await?;
            let response = self
                .auth
                .http()
                .get(url.clone())
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| {
                    log::warn!("Google request failed: {e}");
                    ProviderError::Transient { retry_after: None }
                })?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED && attempt == 0 {
                self.auth.forget(&cx.source_id).await;
                continue;
            }
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok())
                .map(Duration::from_secs);
            let body = response
                .bytes()
                .await
                .map_err(|_| ProviderError::Transient { retry_after: None })?;
            if status.is_success() {
                return serde_json::from_slice(&body).map_err(|e| {
                    ProviderError::Permanent(format!(
                        "Google sent a response that cannot be read: {e}"
                    ))
                });
            }
            return Err(classify(status, retry_after, &body));
        }
        Err(ProviderError::Auth)
    }
}

/// An error response as a `ProviderError` (§7.2).
fn classify(status: StatusCode, retry_after: Option<Duration>, body: &[u8]) -> ProviderError {
    #[derive(serde::Deserialize)]
    struct ErrorBody {
        error: ErrorDetail,
    }
    #[derive(serde::Deserialize)]
    struct ErrorDetail {
        message: Option<String>,
        #[serde(default)]
        errors: Vec<ErrorReason>,
    }
    #[derive(serde::Deserialize)]
    struct ErrorReason {
        reason: Option<String>,
    }
    let detail = serde_json::from_slice::<ErrorBody>(body)
        .ok()
        .map(|b| b.error);
    let rate_limited = detail.as_ref().is_some_and(|d| {
        d.errors.iter().any(|e| {
            matches!(
                e.reason.as_deref(),
                Some("rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded")
            )
        })
    });
    match status {
        StatusCode::UNAUTHORIZED => ProviderError::Auth,
        // The sync token expired: start over with a full pull (§7.5).
        StatusCode::GONE => ProviderError::CursorInvalid,
        StatusCode::NOT_FOUND => ProviderError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => ProviderError::Transient { retry_after },
        StatusCode::FORBIDDEN if rate_limited => ProviderError::Transient { retry_after },
        s if s.is_server_error() => ProviderError::Transient { retry_after },
        s => ProviderError::Permanent(
            detail
                .and_then(|d| d.message)
                .map(|m| format!("Google refused the request: {m}"))
                .unwrap_or_else(|| format!("Google answered HTTP {}", s.as_u16())),
        ),
    }
}

#[async_trait]
impl CalendarProvider for GoogleProvider {
    fn kind(&self) -> SourceKind {
        SourceKind::Google
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: WRITES,
            incremental_sync: true,
            reminders: true,
        }
    }

    /// The account's calendar list, in full every cycle (§13 point 5). Calendars with only
    /// free/busy access are left out.
    async fn list_calendars(
        &self,
        cx: &SourceContext,
    ) -> Result<Vec<RemoteCalendar>, ProviderError> {
        let mut calendars = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", PAGE_SIZE)];
            if let Some(token) = &page_token {
                query.push(("pageToken", token));
            }
            let url = self.url(&["users", "me", "calendarList"], &query)?;
            let page: CalendarListPage = self.get(cx, url).await?;
            calendars.extend(page.items.into_iter().filter_map(|c| c.into_remote(WRITES)));
            match page.next_page_token {
                Some(next) => page_token = Some(next),
                None => return Ok(calendars),
            }
        }
    }

    /// One page of `events.list`, unexpanded (`singleEvents=false`). Without a cursor this is
    /// the first, full sync; with one, the changes since. Both use the same parameters, as
    /// Google requires for a sync token (§7.5).
    async fn pull(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        cursor: Option<&SyncCursor>,
        page: Option<&PageToken>,
    ) -> Result<ChangeSet, ProviderError> {
        let mut query = vec![("singleEvents", "false"), ("maxResults", PAGE_SIZE)];
        if let Some(cursor) = cursor {
            query.push(("syncToken", &cursor.0));
        }
        if let Some(page) = page {
            query.push(("pageToken", &page.0));
        }
        let url = self.url(&["calendars", &calendar.remote_id, "events"], &query)?;
        let response: EventsPage = self.get(cx, url).await?;

        let mut changes = ChangeSet {
            upserts: Vec::new(),
            deletions: Vec::new(),
            full: cursor.is_none(),
            next: match (response.next_page_token, response.next_sync_token) {
                (Some(page), _) => Next::Page(PageToken(page)),
                (None, Some(token)) => Next::Done(SyncCursor(token)),
                (None, None) => {
                    return Err(ProviderError::Permanent(
                        "Google sent neither a next page nor a sync token".into(),
                    ));
                }
            },
            warnings: Vec::new(),
        };
        for event in response.items {
            match event.into_change(calendar.time_zone.as_deref()) {
                Ok(Change::Upsert(event)) => changes.upserts.push(*event),
                Ok(Change::Delete(id)) => changes.deletions.push(id),
                // One unreadable event does not stop the rest; the user sees why it is missing.
                Err(why) => changes
                    .warnings
                    .push(format!("An event was skipped: {why}.")),
            }
        }
        Ok(changes)
    }
}
