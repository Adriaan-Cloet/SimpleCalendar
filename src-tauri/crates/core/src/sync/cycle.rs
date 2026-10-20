//! One sync cycle for one source (`docs/architecture.md` §7.2).

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use tokio::sync::mpsc;

use super::store::{self, LocalCalendar};
use super::{SyncState, SyncStatus, default_interval};
use crate::CoreEvent;
use crate::db::{Db, DbError};
use crate::provider::{
    CalendarProvider, ChangeSet, Next, PageToken, ProviderError, Providers, SourceContext,
    SyncCursor,
};
use crate::secrets::{SecretError, Secrets};

/// What every cycle needs, shared by all of them.
pub(super) struct Ctx {
    pub db: Db,
    pub secrets: Secrets,
    pub providers: Providers,
    pub events: mpsc::UnboundedSender<CoreEvent>,
}

impl Ctx {
    fn emit(&self, event: CoreEvent) {
        // Only fails when the shell stopped listening, which happens when the app exits.
        let _ = self.events.send(event);
    }

    fn emit_status(&self, source_id: &str, state: SyncState, last_error: Option<String>) {
        self.emit(CoreEvent::SyncStatus(SyncStatus {
            source_id: source_id.to_owned(),
            state,
            last_error,
        }));
    }
}

/// Why a cycle failed.
#[derive(Debug, thiserror::Error)]
enum SyncError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Includes a missing or locked keychain, with the message that says what to do.
    #[error(transparent)]
    Secrets(#[from] SecretError),
    #[error("database: {0}")]
    Db(#[from] DbError),
}

/// Runs one cycle of `source_id` and returns when the timer should run the next one: `None`
/// for a source that was removed or is never synced.
///
/// `manual` is a user request. Only that syncs a source that needs a new sign-in.
pub(super) async fn run(ctx: &Ctx, source_id: &str, manual: bool) -> Option<Duration> {
    let id = source_id.to_owned();
    let source = match ctx.db.read(move |c| store::load_source(c, &id)).await {
        Ok(Some(source)) => source,
        Ok(None) => return None,
        Err(e) => {
            log::error!("source {source_id}: cannot read it: {e}");
            return None;
        }
    };
    let Some(provider) = ctx.providers.get(source.kind) else {
        log::warn!("source {source_id}: no provider for {:?}", source.kind);
        return None;
    };
    // Rust note: `try_from` refuses a negative number instead of wrapping it around, and `?`
    // on an `Option` returns `None` from this function when there is no interval at all.
    let interval = source
        .interval_s
        .and_then(|s| u64::try_from(s).ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
        .or_else(|| default_interval(source.kind))?;
    if source.auth_required && !manual {
        return Some(interval);
    }

    ctx.emit_status(source_id, SyncState::Syncing, None);
    let result = sync_source(ctx, source_id, &source.display_name, provider.as_ref()).await;
    let (state, status) = match &result {
        Ok(()) => (SyncState::Ok, "ok"),
        Err(SyncError::Provider(ProviderError::Auth)) => (SyncState::AuthRequired, "auth_required"),
        Err(_) => (SyncState::Error, "error"),
    };
    let last_error = result.err().map(|e| {
        log::warn!("source {source_id}: sync failed: {e}");
        e.to_string()
    });

    let (id, message) = (source_id.to_owned(), last_error.clone());
    if let Err(e) = ctx
        .db
        .write(move |c| store::set_status(c, &id, status, message.as_deref()))
        .await
    {
        log::error!("source {source_id}: cannot store its status: {e}");
    }
    ctx.emit_status(source_id, state, last_error);
    Some(interval)
}

/// Steps 1 to 4 of §7.2. Step 2, pushing the outbox, comes with writable providers (v0.2), and
/// step 5, notifying, happens after every commit.
async fn sync_source(
    ctx: &Ctx,
    source_id: &str,
    display_name: &str,
    provider: &dyn CalendarProvider,
) -> Result<(), SyncError> {
    // 1. Authenticate. For the read-only sources of v0.1 that means reading the secret.
    let cx = SourceContext {
        source_id: source_id.to_owned(),
        display_name: display_name.to_owned(),
        secret: ctx.secrets.get(source_id).await?,
    };

    // 3. List calendars.
    let remote = provider.list_calendars(&cx).await?;
    let id = source_id.to_owned();
    let (calendars, changed) = ctx
        .db
        .write(move |c| {
            let tx = c.transaction()?;
            let result = store::apply_calendars(&tx, &id, &remote)?;
            tx.commit()?;
            Ok(result)
        })
        .await?;
    if !changed.is_empty() {
        ctx.emit(CoreEvent::CalendarChanged {
            calendar_ids: changed,
        });
    }

    // 4. Pull, calendar by calendar.
    for calendar in &calendars {
        pull_calendar(ctx, &cx, provider, calendar).await?;
    }
    Ok(())
}

/// Pulls one calendar from its cursor. If the provider no longer accepts the cursor, starts
/// over with a full pull.
async fn pull_calendar(
    ctx: &Ctx,
    cx: &SourceContext,
    provider: &dyn CalendarProvider,
    calendar: &LocalCalendar,
) -> Result<(), SyncError> {
    match pull_pages(ctx, cx, provider, calendar, calendar.cursor.as_ref()).await {
        Err(SyncError::Provider(ProviderError::CursorInvalid)) if calendar.cursor.is_some() => {
            log::info!(
                "calendar {}: cursor no longer valid; pulling everything",
                calendar.id
            );
            pull_pages(ctx, cx, provider, calendar, None).await
        }
        result => result,
    }
}

/// Pages through one pull. Each page is its own transaction; the last one also stores the
/// cursor, so a cursor never claims more than what was applied.
async fn pull_pages(
    ctx: &Ctx,
    cx: &SourceContext,
    provider: &dyn CalendarProvider,
    calendar: &LocalCalendar,
    cursor: Option<&SyncCursor>,
) -> Result<(), SyncError> {
    let calendar_id = calendar.id;
    let mut page: Option<PageToken> = None;
    // Every remote id of a full pull, for deleting what was not seen after the last page.
    let mut seen = HashSet::new();
    let mut full = false;
    // The warnings of every page, stored with the last one. A `BTreeSet` keeps them sorted and
    // drops the duplicates that consecutive pages report.
    let mut warnings = BTreeSet::new();

    loop {
        let ChangeSet {
            upserts,
            deletions,
            full: page_full,
            next,
            warnings: page_warnings,
        } = provider
            .pull(cx, &calendar.reference, cursor, page.as_ref())
            .await?;
        full |= page_full;
        warnings.extend(page_warnings);
        let last = match &next {
            Next::Done(cursor) => Some((cursor.clone(), std::mem::take(&mut warnings))),
            Next::Page(_) => None,
        };

        // Rust note: the closure runs on another thread, so it must own what it uses. `seen`
        // is moved into it and handed back as part of the result, instead of being shared
        // behind a lock.
        let (returned, changed) = ctx
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let mut changed =
                    store::apply_page(&tx, calendar_id, &upserts, &deletions, &mut seen)?;
                if let Some((cursor, warnings)) = last {
                    if full {
                        changed |= store::finish_full_pull(&tx, calendar_id, &seen)?;
                    } else {
                        // Only inconsistent server data leaves these. They stay invisible,
                        // and the next full pull removes them.
                        let orphans = store::unlinked_exceptions(&tx, calendar_id)?;
                        if orphans > 0 {
                            log::warn!(
                                "calendar {calendar_id}: {orphans} exceptions without a master"
                            );
                        }
                    }
                    store::store_warnings(&tx, calendar_id, warnings, full)?;
                    store::store_cursor(&tx, calendar_id, &cursor, now())?;
                }
                tx.commit()?;
                Ok((seen, changed))
            })
            .await?;
        seen = returned;

        // After the commit, so the re-query it triggers sees the page.
        if changed {
            ctx.emit(CoreEvent::CalendarChanged {
                calendar_ids: vec![calendar_id],
            });
        }
        match next {
            Next::Page(token) => page = Some(token),
            Next::Done(_) => return Ok(()),
        }
    }
}

/// The current time in epoch seconds.
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
