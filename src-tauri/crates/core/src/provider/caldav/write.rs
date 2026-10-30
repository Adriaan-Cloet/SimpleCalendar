//! Writing CalDAV events (`docs/architecture.md` §7.3, §7.8).
//!
//! An event is a resource, an iCalendar object that holds a single event, or a series with its
//! exceptions. Every write sends the whole object: a create puts a new one with
//! `If-None-Match: *`, an edit fetches the object, changes the lines of the changed fields and
//! puts it back with `If-Match`, so whatever else it holds stays. A deleted series is a
//! `DELETE`; a deleted occurrence an `EXDATE` in its series.
//!
//! The ETag belongs to the whole object, not to one event in it. After this app wrote one
//! event of a series, the others it has stored still carry the ETag from before, so the next
//! edit of one of them would look like a conflict. The provider therefore remembers which
//! ETag each of its own writes turned into which (`written`), and an edit based on an ETag it
//! replaced itself counts as based on the newer one.

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use reqwest::Url;

use super::ical::{self, EditError};
use super::{
    Account, Answer, CaldavProvider, collection_of, cursor, own_invitations, parse_feed,
    status_error,
};
use crate::provider::{
    CalendarRef, Etag, EventData, EventUpdate, ProviderError, RemoteEvent, SourceContext,
    WritableProvider,
};

/// Attempts at one edit when the object changes between fetching and putting it back.
const ATTEMPTS: usize = 3;

/// ETags remembered per resource; more is never needed between two pulls.
const REMEMBERED: usize = 20;

const CALENDAR: &str = "text/calendar; charset=utf-8";

/// How a put went.
enum Put {
    /// Stored, with the new ETag if the server gave one.
    Stored(Option<String>),
    /// The condition failed (412): the object exists, or it changed.
    Refused,
}

/// A remote id split into its resource's path and, for an exception, the original start.
fn split(remote_id: &str) -> (&str, Option<i64>) {
    match remote_id.rsplit_once('#') {
        Some((href, start)) => match start.parse() {
            Ok(start) => (href, Some(start)),
            Err(_) => (remote_id, None),
        },
        None => (remote_id, None),
    }
}

/// The `UID` for a new resource: its file name without `.ics`.
fn uid_of(href: &str) -> &str {
    let name = href.rsplit('/').next().unwrap_or(href);
    name.strip_suffix(".ics").unwrap_or(name)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn edit_error(e: EditError) -> ProviderError {
    match e {
        EditError::Unreadable => {
            ProviderError::Permanent("the event on the server cannot be read".into())
        }
        EditError::NoEvent => ProviderError::NotFound,
    }
}

impl CaldavProvider {
    /// The login, with the user's addresses if discovery can find them. Writes come before
    /// the calendar list in a cycle (§7.3), so after a restart discovery may not have run yet.
    async fn writer<'a>(&self, cx: &'a SourceContext) -> Result<Account<'a>, ProviderError> {
        let mut account = Account::of(cx)?;
        // Only who organizes an invitation depends on it; the write itself reports any problem.
        if let Ok(discovered) = self.locate(cx, &account).await {
            account.addresses = discovered.addresses;
        }
        Ok(account)
    }

    /// The address of the resource at `href`, on the calendar's server.
    fn resource(collection: &Url, href: &str) -> Result<Url, ProviderError> {
        collection
            .join(href)
            .ok()
            .filter(|url| url.host_str() == collection.host_str())
            .ok_or_else(|| ProviderError::Permanent("the event's address is invalid".into()))
    }

    /// The object at `url` with its ETag, or `None` if it is gone.
    async fn fetch(
        &self,
        account: &Account<'_>,
        url: &Url,
    ) -> Result<Option<(String, String)>, ProviderError> {
        let answer = self.exchange(account, "GET", url.clone(), &[], "").await?;
        match answer.status.as_u16() {
            200 => {}
            404 | 410 => return Ok(None),
            _ => return Err(status_error(answer.status, answer.retry_after)),
        }
        let etag = answer.etag.ok_or_else(|| {
            ProviderError::Permanent("the server sent the event without an ETag".into())
        })?;
        Ok(Some((etag, answer.body)))
    }

    /// Puts `text` at `url`: over the version `etag`, or only if nothing is there (`None`).
    async fn put(
        &self,
        account: &Account<'_>,
        url: &Url,
        text: &str,
        etag: Option<&str>,
    ) -> Result<Put, ProviderError> {
        let condition = match etag {
            Some(etag) => ("If-Match", etag),
            None => ("If-None-Match", "*"),
        };
        let headers = [("Content-Type", CALENDAR), condition];
        let answer = self
            .exchange(account, "PUT", url.clone(), &headers, text)
            .await?;
        match answer.status.as_u16() {
            200..=299 => Ok(Put::Stored(answer.etag)),
            412 => Ok(Put::Refused),
            // A server that checks what it stores says why in the body (RFC 4791 §5.3.2.1).
            403 | 409 | 415 => {
                log::warn!("CalDAV write refused: {}", answer.body.trim());
                Err(ProviderError::Permanent(
                    "the server refused the event".into(),
                ))
            }
            _ => Err(status_error(answer.status, answer.retry_after)),
        }
    }

    /// The event `remote_id` from the object `text` at `href`, as a pull would read it.
    fn event_in(
        &self,
        account: &Account<'_>,
        href: &str,
        etag: &str,
        text: &str,
        remote_id: &str,
    ) -> Result<Option<RemoteEvent>, ProviderError> {
        let mut feed = parse_feed(text).map_err(|e| ProviderError::Permanent(e.to_string()))?;
        own_invitations(&mut feed, &account.addresses);
        Ok(cursor::resource(href, etag, feed.events, None)
            .events
            .into_iter()
            .find(|e| e.remote_id == remote_id))
    }

    /// The stored event after a write: from the text that was sent, if the server named its
    /// ETag (it may not when it changed the object, RFC 4791 §5.3.4), else fetched.
    #[allow(clippy::too_many_arguments)]
    async fn written(
        &self,
        account: &Account<'_>,
        url: &Url,
        href: &str,
        text: &str,
        before: Option<&str>,
        etag: Option<String>,
        remote_id: &str,
    ) -> Result<RemoteEvent, ProviderError> {
        let (etag, text) = match etag {
            Some(etag) => (etag, text.to_owned()),
            None => self
                .fetch(account, url)
                .await?
                .ok_or(ProviderError::NotFound)?,
        };
        if let Some(before) = before {
            self.remember(url, before, &etag);
        }
        self.event_in(account, href, &etag, &text, remote_id)?
            .ok_or_else(|| ProviderError::Permanent("the server did not keep the event".into()))
    }

    /// Records that this provider's write turned `before` into `after`.
    fn remember(&self, url: &Url, before: &str, after: &str) {
        let mut written = self
            .written
            .lock()
            .expect("the ETag memory is never poisoned");
        let steps = written.entry(url.to_string()).or_default();
        steps.push((before.to_owned(), after.to_owned()));
        if steps.len() > REMEMBERED {
            steps.remove(0);
        }
    }

    /// The ETag `etag` became through this provider's own writes since.
    fn latest(&self, url: &Url, etag: &str) -> String {
        let written = self
            .written
            .lock()
            .expect("the ETag memory is never poisoned");
        let mut etag = etag.to_owned();
        if let Some(steps) = written.get(url.as_str()) {
            for (before, after) in steps {
                if *before == etag {
                    etag = after.clone();
                }
            }
        }
        etag
    }

    fn forget_writes(&self, url: &Url) {
        self.written
            .lock()
            .expect("the ETag memory is never poisoned")
            .remove(url.as_str());
    }
}

#[async_trait]
impl WritableProvider for CaldavProvider {
    async fn get_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
    ) -> Result<Option<RemoteEvent>, ProviderError> {
        let account = self.writer(cx).await?;
        let collection = collection_of(calendar)?;
        let (href, _) = split(remote_id);
        let url = Self::resource(&collection, href)?;
        match self.fetch(&account, &url).await? {
            None => Ok(None),
            Some((etag, text)) => self.event_in(&account, href, &etag, &text, remote_id),
        }
    }

    /// `remote_id` is the new resource's path (`new_event_id`). If something is there already,
    /// an earlier attempt got through: that is the event.
    async fn create_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
        event: &EventData,
    ) -> Result<RemoteEvent, ProviderError> {
        let account = self.writer(cx).await?;
        let collection = collection_of(calendar)?;
        if split(remote_id).1.is_some() || !remote_id.starts_with('/') {
            return Err(ProviderError::Permanent(
                "the new event has no CalDAV address".into(),
            ));
        }
        let url = Self::resource(&collection, remote_id)?;
        let text = ical::create(uid_of(remote_id), event, now());
        match self.put(&account, &url, &text, None).await? {
            Put::Stored(etag) => {
                self.written(&account, &url, remote_id, &text, None, etag, remote_id)
                    .await
            }
            Put::Refused => {
                let (etag, text) = self
                    .fetch(&account, &url)
                    .await?
                    .ok_or(ProviderError::Transient { retry_after: None })?;
                self.event_in(&account, remote_id, &etag, &text, remote_id)?
                    .ok_or_else(|| {
                        ProviderError::Permanent("another event has this address".into())
                    })
            }
        }
    }

    /// Writes the whole object with the changed fields of `update` in it. `if_match` must be
    /// the object's ETag, or one this provider's own writes replaced; else the server changed
    /// the object since the edit, which is a conflict (§7.4).
    async fn update_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        update: &EventUpdate,
        if_match: Option<&Etag>,
    ) -> Result<RemoteEvent, ProviderError> {
        let account = self.writer(cx).await?;
        let collection = collection_of(calendar)?;
        let (href, occurrence) = split(&update.remote_id);
        let url = Self::resource(&collection, href)?;
        for _ in 0..ATTEMPTS {
            let (etag, text) = self
                .fetch(&account, &url)
                .await?
                .ok_or(ProviderError::NotFound)?;
            if let Some(base) = if_match
                && self.latest(&url, &base.0) != etag
            {
                return Err(ProviderError::Conflict);
            }
            let changed = ical::update(&text, occurrence, &update.desired, &update.changed, now())
                .map_err(edit_error)?;
            // Refused: the object changed after it was fetched. Fetch it again.
            if let Put::Stored(new) = self.put(&account, &url, &changed, Some(&etag)).await? {
                return self
                    .written(
                        &account,
                        &url,
                        href,
                        &changed,
                        Some(&etag),
                        new,
                        &update.remote_id,
                    )
                    .await;
            }
        }
        Err(ProviderError::Transient { retry_after: None })
    }

    /// A series or single event is deleted with its resource. An occurrence is excluded from
    /// its series, whatever else changed in the object: that touches no other event.
    async fn delete_event(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        remote_id: &str,
        if_match: Option<&Etag>,
    ) -> Result<(), ProviderError> {
        let account = self.writer(cx).await?;
        let collection = collection_of(calendar)?;
        let (href, occurrence) = split(remote_id);
        let url = Self::resource(&collection, href)?;

        let Some(original_start) = occurrence else {
            let latest = if_match.map(|etag| self.latest(&url, &etag.0));
            let headers: Vec<(&str, &str)> = latest
                .iter()
                .map(|etag| ("If-Match", etag.as_str()))
                .collect();
            let Answer {
                status,
                retry_after,
                ..
            } = self
                .exchange(&account, "DELETE", url.clone(), &headers, "")
                .await?;
            return match status.as_u16() {
                200..=299 | 404 | 410 => {
                    self.forget_writes(&url);
                    Ok(())
                }
                412 => Err(ProviderError::Conflict),
                _ => Err(status_error(status, retry_after)),
            };
        };

        for _ in 0..ATTEMPTS {
            let Some((etag, text)) = self.fetch(&account, &url).await? else {
                return Ok(());
            };
            let changed = match ical::exclude(&text, original_start, now()) {
                Ok(Some(changed)) => changed,
                // Already excluded, or no series left to exclude it from.
                Ok(None) | Err(EditError::NoEvent) => return Ok(()),
                Err(e) => return Err(edit_error(e)),
            };
            match self.put(&account, &url, &changed, Some(&etag)).await? {
                Put::Stored(Some(new)) => {
                    self.remember(&url, &etag, &new);
                    return Ok(());
                }
                Put::Stored(None) => {
                    if let Some((new, _)) = self.fetch(&account, &url).await? {
                        self.remember(&url, &etag, &new);
                    }
                    return Ok(());
                }
                Put::Refused => {}
            }
        }
        Err(ProviderError::Transient { retry_after: None })
    }
}
