//! The CalDAV cursor: the calendar's version tags, and every resource seen with its ETag
//! (`docs/architecture.md` §5).
//!
//! A provider never reads the database, so what the last pull saw travels in the cursor. With it
//! a pull knows which resources changed without the server's help, and which events to delete
//! when a resource is gone or lost an exception.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::provider::{RemoteEvent, SeriesRole, SyncCursor};

/// What the last pull saw. Stored as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Cursor {
    /// The collection's sync-token (RFC 6578). With one, only changes are asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The collection's ctag. If it did not change, neither did any resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctag: Option<String>,
    /// Every resource, by its path.
    pub items: BTreeMap<String, Item>,
}

/// One resource: an event, or a series with its exceptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Item {
    pub etag: String,
    /// The original starts of the exceptions in it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<i64>,
}

impl Cursor {
    pub fn store(&self) -> SyncCursor {
        SyncCursor(serde_json::to_string(self).expect("cursor serialises"))
    }

    /// `None` if the stored text is not a CalDAV cursor: then the calendar is read whole.
    pub fn load(cursor: &SyncCursor) -> Option<Cursor> {
        serde_json::from_str(&cursor.0).ok()
    }
}

impl Item {
    /// The remote ids of the events in this resource: its own path for the event or master,
    /// plus one per exception.
    pub fn remote_ids(&self, href: &str) -> Vec<String> {
        std::iter::once(href.to_owned())
            .chain(
                self.exceptions
                    .iter()
                    .map(|&start| exception_id(href, start)),
            )
            .collect()
    }
}

/// An exception's remote id: the resource's path and the occurrence it replaces. A path never
/// contains a bare `#`, so the two parts cannot be confused.
fn exception_id(href: &str, original_start_utc: i64) -> String {
    format!("{href}#{original_start_utc}")
}

/// A listing against what was seen before.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Diff {
    /// New resources, changed ones, and those without an ETag. In path order.
    pub fetch: Vec<String>,
    /// Resources that were seen before and are not listed any more.
    pub gone: Vec<String>,
}

/// Compares a listing (path to ETag) with the resources seen before.
pub(super) fn compare(
    seen: &BTreeMap<String, Item>,
    listed: &BTreeMap<String, Option<String>>,
) -> Diff {
    let fetch = listed
        .iter()
        .filter(|(href, etag)| match (etag, seen.get(*href)) {
            (Some(etag), Some(item)) => *etag != item.etag,
            _ => true,
        })
        .map(|(href, _)| href.clone())
        .collect();
    let gone = seen
        .keys()
        .filter(|href| !listed.contains_key(*href))
        .cloned()
        .collect();
    Diff { fetch, gone }
}

/// One resource, read.
#[derive(Debug)]
pub(super) struct Read {
    pub events: Vec<RemoteEvent>,
    pub item: Item,
    /// Remote ids the resource had before and does not have now.
    pub removed: Vec<String>,
}

/// Names the events parsed from the resource at `href` after it, and compares them with what
/// it held before.
///
/// The parser names events after their UID. Here the master or single event becomes `href`
/// and each exception `href#<original start>`, because writes (#27) and deletions address
/// resources by path, not by UID.
pub(super) fn resource(
    href: &str,
    etag: &str,
    parsed: Vec<RemoteEvent>,
    before: Option<&Item>,
) -> Read {
    let mut events = Vec::new();
    let mut has_master = false;
    let mut exceptions = BTreeSet::new();
    for mut event in parsed {
        match &mut event.series {
            SeriesRole::Exception {
                master_remote_id,
                original_start_utc,
            } => {
                if !exceptions.insert(*original_start_utc) {
                    continue;
                }
                event.remote_id = exception_id(href, *original_start_utc);
                *master_remote_id = href.to_owned();
            }
            SeriesRole::Single | SeriesRole::Master { .. } => {
                if has_master {
                    log::warn!("{href}: more than one event in one resource; keeping the first");
                    continue;
                }
                has_master = true;
                event.remote_id = href.to_owned();
            }
        }
        event.etag = Some(etag.to_owned());
        events.push(event);
    }

    let now: BTreeSet<&str> = events.iter().map(|e| e.remote_id.as_str()).collect();
    let removed = before
        .map(|item| item.remote_ids(href))
        .unwrap_or_default()
        .into_iter()
        .filter(|id| !now.contains(id.as_str()))
        .collect();
    Read {
        events,
        item: Item {
            etag: etag.to_owned(),
            exceptions: exceptions.into_iter().collect(),
        },
        removed,
    }
}
