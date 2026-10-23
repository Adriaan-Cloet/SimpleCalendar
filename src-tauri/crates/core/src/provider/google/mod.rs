//! The Google Calendar provider: reads and writes calendars and events
//! (`docs/architecture.md` §7.5).
//!
//! Writes come from the outbox (§7.3): `events.insert` with an id generated here, `events.patch`
//! with only the changed fields, and `events.delete`, each with `sendUpdates=none` and, where
//! there is an ETag, `If-Match`. Tokens come from `GoogleAuth`, which the sign-in in the service
//! shares (§8).

mod json;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub use self::json::instance_id;
use self::json::{CalendarListPage, Change, EventsPage, GoogleEvent, event_body};
use crate::google::GoogleAuth;
use crate::model::SourceKind;
use crate::provider::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, Etag, EventData, EventUpdate, Field,
    Next, PageToken, ProviderError, RemoteCalendar, RemoteEvent, SourceContext, SyncCursor,
    WritableProvider,
};

/// Entries per page. Google allows up to 250 for the calendar list and 2500 for events; 250
/// keeps one page, and the transaction that stores it, small.
const PAGE_SIZE: &str = "250";

/// Whether this provider writes. It does since #18, so calendars the account may change are
/// stored as writable (§4).
const WRITES: bool = true;

/// No email to guests: the app has no guest list to ask about (§7.5).
const SEND_UPDATES: (&str, &str) = ("sendUpdates", "none");

/// A response: its status, `Retry-After` and body.
type Answer = (StatusCode, Option<Duration>, Vec<u8>);

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

    /// Sends one request with the source's access token and returns the answer, whatever its
    /// status. A 401 drops the token and tries once more with a refreshed one; the second 401
    /// means signing in again.
    async fn send(
        &self,
        cx: &SourceContext,
        method: Method,
        url: Url,
        body: Option<&Value>,
        if_match: Option<&Etag>,
    ) -> Result<Answer, ProviderError> {
        for attempt in 0..2 {
            let token = self.auth.access_token(cx).await?;
            let mut request = self
                .auth
                .http()
                .request(method.clone(), url.clone())
                .bearer_auth(&token);
            if let Some(body) = body {
                // reqwest's `json` needs a feature this crate does not build; this is all it does.
                request = request
                    .header("Content-Type", "application/json")
                    .body(body.to_string());
            }
            if let Some(etag) = if_match {
                request = request.header("If-Match", &etag.0);
            }
            let response = request.send().await.map_err(|e| {
                log::warn!("Google request failed: {e}");
                ProviderError::unanswered(&e)
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
            return Ok((status, retry_after, body.to_vec()));
        }
        Err(ProviderError::Auth)
    }

    /// GETs `url` and reads the answer as `T`.
    async fn get<T: DeserializeOwned>(
        &self,
        cx: &SourceContext,
        url: Url,
    ) -> Result<T, ProviderError> {
        let answer = self.send(cx, Method::GET, url, None, None).await?;
        parse(answer)
    }

    /// The URL of one event, or of the calendar's events for `None`.
    fn event_url(
        &self,
        calendar: &CalendarRef,
        remote_id: Option<&str>,
        query: &[(&str, &str)],
    ) -> Result<Url, ProviderError> {
        match remote_id {
            Some(id) => self.url(&["calendars", &calendar.remote_id, "events", id], query),
            None => self.url(&["calendars", &calendar.remote_id, "events"], query),
        }
    }
}

/// A successful answer read as `T`, or the error it stands for.
fn parse<T: DeserializeOwned>((status, retry_after, body): Answer) -> Result<T, ProviderError> {
    if !status.is_success() {
        return Err(classify(status, retry_after, &body));
    }
    serde_json::from_slice(&body).map_err(|e| {
        ProviderError::Permanent(format!("Google sent a response that cannot be read: {e}"))
    })
}

/// An event Google sent back after a write, as the engine stores it.
fn written(event: GoogleEvent, calendar: &CalendarRef) -> Result<RemoteEvent, ProviderError> {
    match event.into_change(calendar.time_zone.as_deref()) {
        Ok(Change::Upsert(event)) => Ok(*event),
        Ok(Change::Delete(_)) => Err(ProviderError::NotFound),
        Err(why) => Err(ProviderError::Permanent(format!(
            "Google sent an event that cannot be read: {why}"
        ))),
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
        // `If-Match` no longer matches: someone changed the event (§7.4).
        StatusCode::PRECONDITION_FAILED => ProviderError::Conflict,
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

    fn as_writable(&self) -> Option<&dyn WritableProvider> {
        Some(self)
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
        let url = self.event_url(calendar, None, &query)?;
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

#[async_trait]
impl WritableProvider for GoogleProvider {
    /// `events.get`. `None` if the event is gone, also when Google still lists it as cancelled.
    async fn get_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
    ) -> Result<Option<RemoteEvent>, ProviderError> {
        let url = self.event_url(calendar, Some(remote_id), &[])?;
        match self.get::<GoogleEvent>(cx, url).await {
            Ok(event) => match written(event, calendar) {
                Ok(event) => Ok(Some(event)),
                Err(ProviderError::NotFound) => Ok(None),
                Err(e) => Err(e),
            },
            Err(ProviderError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `events.insert` with the whole event and the id generated here. Google answers 409 when
    /// that id exists: an earlier insert got through and only its answer was lost (§7.3).
    async fn create_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
        event: &EventData,
    ) -> Result<RemoteEvent, ProviderError> {
        let body = event_body(event, &Field::ALL.into_iter().collect(), Some(remote_id));
        let url = self.event_url(calendar, None, &[SEND_UPDATES])?;
        let answer = self.send(cx, Method::POST, url, Some(&body), None).await?;
        if answer.0 == StatusCode::CONFLICT {
            log::info!("event {remote_id} exists already; taking it as created");
            return self
                .get_event(cx, calendar, remote_id)
                .await?
                .ok_or(ProviderError::NotFound);
        }
        written(parse(answer)?, calendar)
    }

    /// `events.patch` with only the changed fields, never `events.update`, which would erase
    /// the guests, conferencing and everything else the app does not store (§7.5).
    async fn update_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        update: &EventUpdate,
        if_match: Option<&Etag>,
    ) -> Result<RemoteEvent, ProviderError> {
        let body = event_body(&update.desired, &update.changed, None);
        let url = self.event_url(calendar, Some(&update.remote_id), &[SEND_UPDATES])?;
        let answer = self
            .send(cx, Method::PATCH, url, Some(&body), if_match)
            .await?;
        written(parse(answer)?, calendar)
    }

    /// `events.delete`. Google answers 410 for an event deleted before, and 404 for one it
    /// never had: both are gone, which is what was asked.
    async fn delete_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
        if_match: Option<&Etag>,
    ) -> Result<(), ProviderError> {
        let url = self.event_url(calendar, Some(remote_id), &[SEND_UPDATES])?;
        let (status, retry_after, body) =
            self.send(cx, Method::DELETE, url, None, if_match).await?;
        match status {
            s if s.is_success() => Ok(()),
            StatusCode::NOT_FOUND | StatusCode::GONE => Ok(()),
            s => Err(classify(s, retry_after, &body)),
        }
    }
}
