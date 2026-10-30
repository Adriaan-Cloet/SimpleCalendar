//! The CalDAV provider: finding an account's calendars, reading them and writing their events
//! (`docs/architecture.md` §5, §7.8).
//!
//! Discovery starts at the address the user gave: its `/.well-known/caldav` (RFC 6764), then
//! the address itself. From there it follows `current-user-principal` to the
//! `calendar-home-set` (RFC 4791 §6.2.1), whose members are the calendars. An address that is
//! a calendar is used as the account's only calendar. What was found is kept in memory per
//! source, so later syncs go straight to the calendar list.
//!
//! A pull asks for changes in the best way the calendar offers: a sync-token report
//! (RFC 6578), else the ctag and the ETag of every resource, compared with the cursor. The
//! changed resources are fetched with `calendar-multiget` and parsed by the ICS parser.
//!
//! A write fetches the event's resource, changes it (`ical`) and puts it back with `If-Match`
//! (`write.rs`).

mod cursor;
mod ical;
#[cfg(test)]
mod live;
#[cfg(test)]
mod tests;
mod write;
mod xml;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, ETAG, LOCATION};
use reqwest::{Method, StatusCode, Url};

use self::cursor::{Cursor, Item, compare};
use self::xml::{CALDAV, CS, DAV, ICAL, Multistatus, Response};
use super::ics::parse::{Feed, address, calendar_zone, parse_feed};
use super::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, Next, PageToken, ProviderError,
    RemoteCalendar, RemoteEvent, SourceContext, SyncCursor, WritableProvider, http,
};
use crate::model::{Access, SourceKind};
use crate::secrets::Secret;

/// The largest answer that is read. A multiget of `MULTIGET_BATCH` events is far below it.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// Resources per `calendar-multiget`.
const MULTIGET_BATCH: usize = 100;

/// Redirects followed for one request.
const MAX_REDIRECTS: usize = 5;

/// Sync reports one pull asks for when the server cuts its answers short (RFC 6578 §3.6).
const MAX_SYNC_ROUNDS: usize = 50;

/// Where an account's calendars are.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Found {
    /// A calendar home: its members are the calendars.
    Home(Url),
    /// The address is one calendar.
    Calendar(Url),
}

/// What discovery found for one source.
#[derive(Debug, Clone)]
struct Discovered {
    /// The address it started from.
    given: String,
    found: Found,
    /// The user's own addresses (`calendar-user-address-set`), as `parse::address` writes them.
    addresses: Vec<String>,
}

pub struct CaldavProvider {
    client: reqwest::Client,
    max_bytes: usize,
    /// What discovery found, by source id.
    found: Mutex<HashMap<String, Discovered>>,
    /// The ETags this provider's own writes went from and to, by resource URL (`write.rs`).
    written: Mutex<HashMap<String, Vec<(String, String)>>>,
}

impl CaldavProvider {
    pub fn new() -> reqwest::Result<CaldavProvider> {
        Ok(CaldavProvider {
            client: http::client_without_redirects()?,
            max_bytes: MAX_BYTES,
            found: Mutex::new(HashMap::new()),
            written: Mutex::new(HashMap::new()),
        })
    }
}

/// The login of one source, from its secret.
struct Account<'a> {
    /// The address as the user gave it, the key of the discovery cache.
    given: &'a str,
    url: Url,
    username: &'a str,
    password: &'a str,
    /// The user's own addresses, once discovery found them: who organizes an invitation.
    addresses: Vec<String>,
}

impl<'a> Account<'a> {
    fn of(cx: &'a SourceContext) -> Result<Account<'a>, ProviderError> {
        let Some(Secret::Caldav {
            url,
            username,
            password,
        }) = &cx.secret
        else {
            return Err(ProviderError::Permanent(
                "the CalDAV login is missing from the keychain".into(),
            ));
        };
        Ok(Account {
            given: url,
            url: server_url(url)?,
            username,
            password,
            addresses: Vec::new(),
        })
    }
}

/// The address requests start from, for an address as the user typed it. Without a scheme it
/// is `https://`. Plain `http://` is only allowed for the user's own computer: the password
/// travels with every request.
pub fn server_url(given: &str) -> Result<Url, ProviderError> {
    let given = given.trim();
    let url = if given.contains("://") {
        Url::parse(given)
    } else {
        Url::parse(&format!("https://{given}"))
    }
    .map_err(|_| ProviderError::Permanent("the CalDAV address is not a valid URL".into()))?;
    if !allowed(&url) {
        return Err(ProviderError::Permanent(
            "the CalDAV address must start with https://".into(),
        ));
    }
    Ok(url)
}

/// `https`, or `http` to this computer.
fn allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => {
            // An IPv6 host comes in brackets: `[::1]`.
            let host = url.host_str().unwrap_or_default();
            let host = host.trim_start_matches('[').trim_end_matches(']');
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        _ => false,
    }
}

/// An answer: the address it finally came from, its status, `Retry-After`, ETag and body.
struct Answer {
    url: Url,
    status: StatusCode,
    retry_after: Option<Duration>,
    etag: Option<String>,
    body: String,
}

impl CaldavProvider {
    /// Sends one WebDAV request with an XML body.
    async fn send(
        &self,
        account: &Account<'_>,
        method: &str,
        url: Url,
        depth: &str,
        body: &str,
    ) -> Result<Answer, ProviderError> {
        let headers = [
            ("Depth", depth),
            (CONTENT_TYPE.as_str(), "application/xml; charset=utf-8"),
        ];
        self.exchange(account, method, url, &headers, body).await
    }

    /// Sends one request with the account's login and follows redirects on the same host,
    /// keeping the method, headers and body. A redirect to another host is refused, so the
    /// password never goes anywhere but the server the user named or the addresses it lists.
    async fn exchange(
        &self,
        account: &Account<'_>,
        method: &str,
        mut url: Url,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Result<Answer, ProviderError> {
        let method = Method::from_bytes(method.as_bytes()).expect("a valid method name");
        // The server names the addresses after the first, such as its calendar home. None of
        // them may take the password over plain HTTP.
        if !allowed(&url) {
            return Err(ProviderError::Permanent(
                "the server points to an address without https".into(),
            ));
        }
        for _ in 0..=MAX_REDIRECTS {
            let mut request = self
                .client
                .request(method.clone(), url.clone())
                .basic_auth(account.username, Some(account.password));
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            let response = request
                .body(body.to_owned())
                .send()
                .await
                .map_err(request_error)?;
            let status = response.status();
            if status.is_redirection() && status != StatusCode::NOT_MODIFIED {
                url = redirect(&url, &response)?;
                continue;
            }
            let retry_after = http::retry_after(&response);
            let etag = response
                .headers()
                .get(ETAG)
                .and_then(|e| e.to_str().ok())
                .filter(|e| !e.is_empty())
                .map(str::to_owned);
            let body = http::read_body(response, self.max_bytes)
                .await
                .map_err(|e| match e {
                    http::BodyError::TooLarge => ProviderError::Permanent(format!(
                        "the server's answer is larger than {} bytes",
                        self.max_bytes
                    )),
                    http::BodyError::Request(e) => request_error(e),
                })?;
            return Ok(Answer {
                url,
                status,
                retry_after,
                etag,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        Err(ProviderError::Permanent(
            "the server redirected too many times".into(),
        ))
    }

    /// A request whose answer must be a multistatus.
    async fn dav(
        &self,
        account: &Account<'_>,
        method: &str,
        url: Url,
        depth: &str,
        body: &str,
    ) -> Result<(Url, Multistatus), ProviderError> {
        let answer = self.send(account, method, url, depth, body).await?;
        if answer.status != StatusCode::MULTI_STATUS {
            return Err(status_error(answer.status, answer.retry_after));
        }
        let multistatus = Multistatus::parse(&answer.body)
            .map_err(|e| ProviderError::Permanent(e.to_string()))?;
        Ok((answer.url, multistatus))
    }
}

/// Where a redirect points, if it stays on the same host and is allowed.
fn redirect(from: &Url, response: &reqwest::Response) -> Result<Url, ProviderError> {
    let to = response
        .headers()
        .get(LOCATION)
        .and_then(|l| l.to_str().ok())
        .and_then(|l| from.join(l).ok())
        .ok_or_else(|| ProviderError::Permanent("the server sent a broken redirect".into()))?;
    if to.host_str() != from.host_str() || !allowed(&to) {
        return Err(ProviderError::Permanent(format!(
            "the server redirected to another address ({}); enter that address instead",
            to.host_str().unwrap_or("unknown")
        )));
    }
    Ok(to)
}

// ---- discovery -----------------------------------------------------------------------------

impl CaldavProvider {
    /// Where the account's calendars are: remembered, or found now.
    async fn locate(
        &self,
        cx: &SourceContext,
        account: &Account<'_>,
    ) -> Result<Discovered, ProviderError> {
        if let Some(discovered) = self.remembered(cx, account) {
            return Ok(discovered);
        }
        let (found, addresses) = self.discover(account).await?;
        let discovered = Discovered {
            given: account.given.to_owned(),
            found,
            addresses,
        };
        self.found
            .lock()
            .expect("the discovery cache is never poisoned")
            .insert(cx.source_id.clone(), discovered.clone());
        Ok(discovered)
    }

    fn remembered(&self, cx: &SourceContext, account: &Account<'_>) -> Option<Discovered> {
        let found = self
            .found
            .lock()
            .expect("the discovery cache is never poisoned");
        found
            .get(&cx.source_id)
            .filter(|d| d.given == account.given)
            .cloned()
    }

    fn forget(&self, cx: &SourceContext) {
        self.found
            .lock()
            .expect("the discovery cache is never poisoned")
            .remove(&cx.source_id);
    }

    async fn discover(&self, account: &Account<'_>) -> Result<(Found, Vec<String>), ProviderError> {
        let mut starts = Vec::new();
        // An address without a path is a server: it says where CalDAV lives.
        if account.url.path() == "/" {
            starts.extend(account.url.join("/.well-known/caldav").ok());
        }
        starts.push(account.url.clone());
        for start in starts {
            if let Some((found, mut addresses)) = self.discover_from(account, start).await? {
                // Servers often list only the principal's path. A login that is an e-mail
                // address is the user's address as well.
                if account.username.contains('@') {
                    addresses.push(address(account.username));
                }
                return Ok((found, addresses));
            }
        }
        Err(ProviderError::Permanent(
            "no CalDAV calendars were found at this address".into(),
        ))
    }

    /// What `start` leads to, with the user's addresses, or `None` if it is not a way in.
    async fn discover_from(
        &self,
        account: &Account<'_>,
        start: Url,
    ) -> Result<Option<(Found, Vec<String>)>, ProviderError> {
        let Some((url, here)) = self.describe(account, start).await? else {
            return Ok(None);
        };
        if !is_calendar(&here)
            && let Some(home) = home_of(&url, &here)
        {
            return Ok(Some((Found::Home(home), addresses_of(&here))));
        }
        let principal = here
            .prop(DAV, "current-user-principal")
            .and_then(|p| p.href())
            .and_then(|p| url.join(&p).ok())
            .filter(|p| *p != url);
        let principal = match principal {
            Some(principal) => self.describe(account, principal).await?,
            None => None,
        };
        let addresses = principal
            .as_ref()
            .map(|(_, p)| addresses_of(p))
            .unwrap_or_default();
        if is_calendar(&here) {
            return Ok(Some((Found::Calendar(url), addresses)));
        }
        Ok(principal
            .and_then(|(url, principal)| home_of(&url, &principal))
            .map(|home| (Found::Home(home), addresses)))
    }

    /// `PROPFIND` for the discovery properties of one address. `None` if the address does not
    /// answer as WebDAV; a rejected login or a server in trouble is an error.
    async fn describe(
        &self,
        account: &Account<'_>,
        url: Url,
    ) -> Result<Option<(Url, Response)>, ProviderError> {
        let answer = self
            .send(account, "PROPFIND", url, "0", xml::PROPFIND_DISCOVER)
            .await?;
        match answer.status.as_u16() {
            207 => {}
            401 => return Err(ProviderError::Auth),
            408 | 429 | 500..=599 => {
                return Err(status_error(answer.status, answer.retry_after));
            }
            _ => return Ok(None),
        }
        let Ok(multistatus) = Multistatus::parse(&answer.body) else {
            return Ok(None);
        };
        Ok(multistatus
            .responses
            .into_iter()
            .next()
            .map(|response| (answer.url, response)))
    }
}

fn is_calendar(response: &Response) -> bool {
    response
        .prop(DAV, "resourcetype")
        .is_some_and(|t| t.has(CALDAV, "calendar"))
}

fn home_of(base: &Url, response: &Response) -> Option<Url> {
    let home = response.prop(CALDAV, "calendar-home-set")?.href()?;
    base.join(&home).ok()
}

/// The user's own addresses, as a principal lists them (RFC 6638 §2.4.1), to know which
/// invitations they organize.
fn addresses_of(principal: &Response) -> Vec<String> {
    principal
        .prop(CALDAV, "calendar-user-address-set")
        .map(|set| {
            set.children(DAV, "href")
                .map(|h| address(h.text()))
                .filter(|a| !a.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Marks the invitations the user organizes as theirs: the parser cannot know their address.
fn own_invitations(feed: &mut Feed, addresses: &[String]) {
    for event in &mut feed.events {
        if feed
            .organizers
            .get(&event.remote_id)
            .is_some_and(|organizer| addresses.contains(organizer))
        {
            event.is_organizer = true;
        }
    }
}

// ---- calendars -----------------------------------------------------------------------------

impl CaldavProvider {
    async fn calendars(
        &self,
        account: &Account<'_>,
        found: &Found,
    ) -> Result<Vec<RemoteCalendar>, ProviderError> {
        let (url, depth) = match found {
            Found::Home(url) => (url, "1"),
            Found::Calendar(url) => (url, "0"),
        };
        let (base, multistatus) = self
            .dav(
                account,
                "PROPFIND",
                url.clone(),
                depth,
                xml::PROPFIND_CALENDARS,
            )
            .await?;
        Ok(multistatus
            .responses
            .iter()
            .filter_map(|r| calendar(&base, r))
            .collect())
    }
}

/// A calendar collection that holds events, as a `RemoteCalendar`. Task lists, the scheduling
/// inbox and outbox and anything else are `None`.
fn calendar(base: &Url, response: &Response) -> Option<RemoteCalendar> {
    if !is_calendar(response) {
        return None;
    }
    // Without the property a calendar takes every kind of component (RFC 4791 §5.2.3).
    if let Some(set) = response.prop(CALDAV, "supported-calendar-component-set")
        && !set.children(CALDAV, "comp").any(|c| {
            c.attr("name")
                .is_some_and(|n| n.eq_ignore_ascii_case("VEVENT"))
        })
    {
        return None;
    }
    let url = base.join(&response.href).ok()?;
    let name = response
        .prop(DAV, "displayname")
        .map(|n| n.text().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            url.path_segments()
                .and_then(|mut s| s.rfind(|s| !s.is_empty()))
                .unwrap_or("Calendar")
                .to_owned()
        });
    Some(RemoteCalendar {
        remote_id: url.to_string(),
        name,
        color: response
            .prop(ICAL, "calendar-color")
            .and_then(|c| color(c.text())),
        time_zone: response
            .prop(CALDAV, "calendar-timezone")
            .and_then(|z| calendar_zone(z.raw_text())),
        access: if can_write(response) {
            Access::Write
        } else {
            Access::Read
        },
        is_primary: false,
        default_reminders: Vec::new(),
    })
}

/// `#RRGGBB` or `#RRGGBBAA` (Apple's form) as `#rrggbb`.
fn color(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#')?;
    let rgb = hex.get(..6).filter(|_| hex.len() == 6 || hex.len() == 8)?;
    rgb.chars()
        .all(|c| c.is_ascii_hexdigit())
        .then(|| format!("#{}", rgb.to_ascii_lowercase()))
}

/// Whether the user may change the calendar's events (RFC 3744 §5.4). A server that does not
/// say is taken at its word when it refuses a write.
fn can_write(response: &Response) -> bool {
    response
        .prop(DAV, "current-user-privilege-set")
        .is_none_or(|set| {
            set.children(DAV, "privilege")
                .any(|p| p.has(DAV, "all") || p.has(DAV, "write") || p.has(DAV, "write-content"))
        })
}

// ---- pulling -------------------------------------------------------------------------------

/// A collection's version tags and members, from a `PROPFIND` of `PROPFIND_VERSIONS`.
#[derive(Debug, Default)]
struct Versions {
    token: Option<String>,
    ctag: Option<String>,
    /// Each member resource by path, with its ETag if it has one.
    members: BTreeMap<String, Option<String>>,
}

impl Versions {
    fn of(collection: &Url, multistatus: &Multistatus) -> Versions {
        let mut versions = Versions::default();
        for r in &multistatus.responses {
            let Some(path) = path_of(collection, &r.href) else {
                continue;
            };
            let text = |ns, name| {
                r.prop(ns, name)
                    .map(|p| p.text().to_owned())
                    .filter(|t| !t.is_empty())
            };
            if same_path(&path, collection.path()) {
                versions.token = text(DAV, "sync-token");
                versions.ctag = text(CS, "getctag");
            } else if !r
                .prop(DAV, "resourcetype")
                .is_some_and(|t| t.has(DAV, "collection"))
            {
                versions.members.insert(path, text(DAV, "getetag"));
            }
        }
        versions
    }
}

/// The path of `href` as the server wrote it, which may be a full URL.
fn path_of(base: &Url, href: &str) -> Option<String> {
    base.join(href).ok().map(|url| url.path().to_owned())
}

/// Whether two collection paths are the same, with or without the final `/`.
fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// What a pull found, before it becomes a change set.
#[derive(Default)]
struct Pulled {
    upserts: Vec<RemoteEvent>,
    deletions: Vec<String>,
    warnings: BTreeSet<String>,
}

/// One fetched resource.
struct Fetched {
    href: String,
    etag: String,
    /// `None` if the server no longer has it.
    data: Option<String>,
}

/// What a sync report says changed.
struct SyncReport {
    /// The token to ask from next time.
    token: String,
    /// Changed and new resources by path, with their ETags.
    changed: BTreeMap<String, Option<String>>,
    deleted: BTreeSet<String>,
}

/// Why a sync report did not give changes.
enum SyncFailure {
    /// The server does not do sync reports after all: compare ETags instead.
    Unsupported,
    Provider(ProviderError),
}

impl CaldavProvider {
    /// The whole calendar: every resource, and the version tags to pull changes from later.
    async fn pull_all(
        &self,
        account: &Account<'_>,
        collection: &Url,
    ) -> Result<ChangeSet, ProviderError> {
        let versions = self.versions(account, collection, "1").await?;
        let hrefs: Vec<String> = versions.members.keys().cloned().collect();
        let mut items = BTreeMap::new();
        let mut pulled = Pulled::default();
        self.read(
            account,
            collection,
            hrefs,
            &BTreeMap::new(),
            &mut items,
            &mut pulled,
        )
        .await?;
        let cursor = Cursor {
            token: versions.token,
            ctag: versions.ctag,
            items,
        };
        Ok(change_set(pulled, true, &cursor))
    }

    /// What changed since `previous`.
    async fn pull_changes(
        &self,
        account: &Account<'_>,
        collection: &Url,
        previous: Cursor,
    ) -> Result<ChangeSet, ProviderError> {
        if let Some(token) = &previous.token {
            match self.sync_report(account, collection, token).await {
                Ok(report) => {
                    return self
                        .apply_changes(account, collection, previous, report)
                        .await;
                }
                Err(SyncFailure::Unsupported) => {}
                Err(SyncFailure::Provider(e)) => return Err(e),
            }
        }

        if let Some(ctag) = &previous.ctag {
            let now = self.versions(account, collection, "0").await?;
            if now.ctag.as_ref() == Some(ctag) {
                return Ok(change_set(Pulled::default(), false, &previous));
            }
        }
        let versions = self.versions(account, collection, "1").await?;
        // Without any ETag nothing can be compared: the calendar is read whole.
        if !versions.members.is_empty() && versions.members.values().all(Option::is_none) {
            return self.pull_all(account, collection).await;
        }
        let diff = compare(&previous.items, &versions.members);
        let mut items = previous.items;
        let mut pulled = Pulled::default();
        for href in diff.gone {
            if let Some(item) = items.remove(&href) {
                pulled.deletions.extend(item.remote_ids(&href));
            }
        }
        let before = items.clone();
        self.read(
            account,
            collection,
            diff.fetch,
            &before,
            &mut items,
            &mut pulled,
        )
        .await?;
        let cursor = Cursor {
            token: versions.token,
            ctag: versions.ctag,
            items,
        };
        Ok(change_set(pulled, false, &cursor))
    }

    /// Applies a sync report's changes to the cursor's items.
    async fn apply_changes(
        &self,
        account: &Account<'_>,
        collection: &Url,
        previous: Cursor,
        report: SyncReport,
    ) -> Result<ChangeSet, ProviderError> {
        let mut items = previous.items;
        let mut pulled = Pulled::default();
        for href in report.deleted {
            if let Some(item) = items.remove(&href) {
                pulled.deletions.extend(item.remote_ids(&href));
            }
        }
        // A resource whose ETag is the one already seen is not fetched again, such as one this
        // app wrote itself (#27).
        let fetch = report
            .changed
            .into_iter()
            .filter(|(href, etag)| match (etag, items.get(href)) {
                (Some(etag), Some(item)) => *etag != item.etag,
                _ => true,
            })
            .map(|(href, _)| href)
            .collect();
        let before = items.clone();
        self.read(account, collection, fetch, &before, &mut items, &mut pulled)
            .await?;
        let cursor = Cursor {
            token: Some(report.token),
            ctag: previous.ctag,
            items,
        };
        Ok(change_set(pulled, false, &cursor))
    }

    /// `PROPFIND` for the version tags, of the collection alone (`Depth: 0`) or with its
    /// members (`Depth: 1`).
    async fn versions(
        &self,
        account: &Account<'_>,
        collection: &Url,
        depth: &str,
    ) -> Result<Versions, ProviderError> {
        let (url, multistatus) = self
            .dav(
                account,
                "PROPFIND",
                collection.clone(),
                depth,
                xml::PROPFIND_VERSIONS,
            )
            .await?;
        Ok(Versions::of(&url, &multistatus))
    }

    /// The changes since `token`. A cut-short answer (507 on the collection) is continued from
    /// the token it gave.
    async fn sync_report(
        &self,
        account: &Account<'_>,
        collection: &Url,
        token: &str,
    ) -> Result<SyncReport, SyncFailure> {
        let mut token = token.to_owned();
        let mut changed = BTreeMap::new();
        let mut deleted = BTreeSet::new();
        for _ in 0..MAX_SYNC_ROUNDS {
            let answer = self
                .send(
                    account,
                    "REPORT",
                    collection.clone(),
                    "1",
                    &xml::sync_collection(&token),
                )
                .await
                .map_err(SyncFailure::Provider)?;
            match answer.status.as_u16() {
                207 => {}
                // RFC 6578 §3.2: a token the server no longer knows fails the
                // `valid-sync-token` precondition, as 403; some servers answer 409 or 410.
                403 | 409 | 410 => {
                    return Err(SyncFailure::Provider(ProviderError::CursorInvalid));
                }
                400 | 405 | 415 | 501 => return Err(SyncFailure::Unsupported),
                _ => {
                    return Err(SyncFailure::Provider(status_error(
                        answer.status,
                        answer.retry_after,
                    )));
                }
            }
            let Ok(multistatus) = Multistatus::parse(&answer.body) else {
                return Err(SyncFailure::Unsupported);
            };
            let Some(next) = multistatus.sync_token.clone() else {
                return Err(SyncFailure::Unsupported);
            };
            let mut truncated = false;
            for r in &multistatus.responses {
                let Some(path) = path_of(&answer.url, &r.href) else {
                    continue;
                };
                if same_path(&path, answer.url.path()) {
                    truncated |= r.status == Some(507);
                } else if r.status == Some(404) {
                    changed.remove(&path);
                    deleted.insert(path);
                } else {
                    let etag = r.prop(DAV, "getetag").map(|e| e.text().to_owned());
                    deleted.remove(&path);
                    changed.insert(path, etag.filter(|e| !e.is_empty()));
                }
            }
            token = next;
            if !truncated {
                return Ok(SyncReport {
                    token,
                    changed,
                    deleted,
                });
            }
        }
        Err(SyncFailure::Provider(ProviderError::Transient {
            retry_after: None,
        }))
    }

    /// Fetches `hrefs`, parses them, and records the result in `items` and `pulled`. `before`
    /// holds what each resource had at the last pull.
    async fn read(
        &self,
        account: &Account<'_>,
        collection: &Url,
        hrefs: Vec<String>,
        before: &BTreeMap<String, Item>,
        items: &mut BTreeMap<String, Item>,
        pulled: &mut Pulled,
    ) -> Result<(), ProviderError> {
        for batch in hrefs.chunks(MULTIGET_BATCH) {
            let fetched = self.multiget(account, collection, batch).await?;
            // Parsing a few hundred events takes a while; it runs on a blocking thread so it
            // does not hold up the async runtime that other tasks share.
            let parsed = tokio::task::spawn_blocking(move || {
                fetched
                    .into_iter()
                    .map(|f| {
                        let feed = f.data.as_deref().map(parse_feed);
                        (f, feed)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .map_err(|e| ProviderError::Permanent(format!("parsing events failed: {e}")))?;

            for (fetched, feed) in parsed {
                let previous = before.get(&fetched.href);
                match feed {
                    None => {
                        if let Some(item) = items.remove(&fetched.href) {
                            pulled.deletions.extend(item.remote_ids(&fetched.href));
                        }
                    }
                    Some(Err(e)) => {
                        // Its events stay as they were. The old ETag stays too, so the next
                        // pull tries again.
                        log::warn!("{}: {e}", fetched.href);
                    }
                    Some(Ok(mut feed)) => {
                        own_invitations(&mut feed, &account.addresses);
                        let read =
                            cursor::resource(&fetched.href, &fetched.etag, feed.events, previous);
                        pulled.upserts.extend(read.events);
                        pulled.deletions.extend(read.removed);
                        pulled.warnings.extend(feed.warnings);
                        items.insert(fetched.href, read.item);
                    }
                }
            }
        }
        Ok(())
    }

    async fn multiget(
        &self,
        account: &Account<'_>,
        collection: &Url,
        hrefs: &[String],
    ) -> Result<Vec<Fetched>, ProviderError> {
        let (url, multistatus) = self
            .dav(
                account,
                "REPORT",
                collection.clone(),
                "1",
                &xml::multiget(hrefs),
            )
            .await?;
        let mut fetched: Vec<Fetched> = multistatus
            .responses
            .iter()
            .filter_map(|r| {
                let href = path_of(&url, &r.href)?;
                let data = r
                    .prop(CALDAV, "calendar-data")
                    .map(|d| d.raw_text().to_owned())
                    .filter(|d| !d.trim().is_empty());
                let etag = r
                    .prop(DAV, "getetag")
                    .map(|e| e.text().to_owned())
                    .unwrap_or_default();
                Some(Fetched { href, etag, data })
            })
            .collect();
        // A resource the server left out of its answer is gone as well.
        let answered: BTreeSet<String> = fetched.iter().map(|f| f.href.clone()).collect();
        fetched.extend(
            hrefs
                .iter()
                .filter(|href| !answered.contains(*href))
                .map(|href| Fetched {
                    href: href.clone(),
                    etag: String::new(),
                    data: None,
                }),
        );
        Ok(fetched)
    }
}

fn change_set(pulled: Pulled, full: bool, cursor: &Cursor) -> ChangeSet {
    ChangeSet {
        upserts: pulled.upserts,
        deletions: pulled.deletions,
        full,
        next: Next::Done(cursor.store()),
        warnings: pulled.warnings.into_iter().collect(),
    }
}

#[async_trait]
impl CalendarProvider for CaldavProvider {
    fn kind(&self) -> SourceKind {
        SourceKind::Caldav
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: true,
            incremental_sync: true,
            reminders: true,
        }
    }

    async fn list_calendars(
        &self,
        cx: &SourceContext,
    ) -> Result<Vec<RemoteCalendar>, ProviderError> {
        let account = Account::of(cx)?;
        let remembered = self.remembered(cx, &account).is_some();
        let found = self.locate(cx, &account).await?.found;
        match self.calendars(&account, &found).await {
            // The server moved the calendars since they were found: look again, once.
            Err(ProviderError::NotFound) if remembered => {
                self.forget(cx);
                let found = self.locate(cx, &account).await?.found;
                self.calendars(&account, &found).await
            }
            result => result,
        }
    }

    async fn pull(
        &self,
        cx: &SourceContext,
        calendar: &CalendarRef,
        cursor: Option<&SyncCursor>,
        _page: Option<&PageToken>,
    ) -> Result<ChangeSet, ProviderError> {
        let mut account = Account::of(cx)?;
        // The engine lists the calendars first, which runs discovery.
        if let Some(discovered) = self.remembered(cx, &account) {
            account.addresses = discovered.addresses;
        }
        let collection = collection_of(calendar)?;
        // A cursor this provider cannot read is treated as none: the calendar is read whole.
        match cursor.and_then(Cursor::load) {
            None => self.pull_all(&account, &collection).await,
            Some(previous) => self.pull_changes(&account, &collection, previous).await,
        }
    }

    fn as_writable(&self) -> Option<&dyn WritableProvider> {
        Some(self)
    }
}

/// The calendar's collection, from its remote id.
fn collection_of(calendar: &CalendarRef) -> Result<Url, ProviderError> {
    Url::parse(&calendar.remote_id)
        .ok()
        .filter(allowed)
        .ok_or_else(|| ProviderError::Permanent("the calendar address is invalid".into()))
}

/// The remote id for a new event in the calendar with remote id `calendar`: the path of a
/// resource named after `name`, a UUID in hex (§7.3). Writes and pulls address events by path
/// (§7.8), so the app stores this from the start.
pub fn new_event_id(calendar: &str, name: &str) -> String {
    let path = Url::parse(calendar)
        .map(|u| u.path().to_owned())
        .unwrap_or_default();
    format!("{}/{name}.ics", path.trim_end_matches('/'))
}

/// The remote id of the occurrence of the series `master` at `original_start_utc`, as a pull
/// names its exception.
pub fn occurrence_id(master: &str, original_start_utc: i64) -> String {
    cursor::exception_id(master, original_start_utc)
}

/// Classifies a failed request. The URL is left out of the log: it names the account.
fn request_error(e: reqwest::Error) -> ProviderError {
    if e.is_builder() {
        return ProviderError::Permanent("the CalDAV address is not a valid URL".into());
    }
    let error = ProviderError::unanswered(&e);
    log::info!("CalDAV request failed: {}", e.without_url());
    error
}

/// Classifies an HTTP status that is not the expected one.
fn status_error(status: StatusCode, retry_after: Option<Duration>) -> ProviderError {
    match status.as_u16() {
        401 => ProviderError::Auth,
        403 => ProviderError::Permanent("the server refused access to this calendar".into()),
        404 | 410 => ProviderError::NotFound,
        408 | 429 | 500..=599 => ProviderError::Transient { retry_after },
        _ => ProviderError::Permanent(format!("the server answered HTTP {status}")),
    }
}
