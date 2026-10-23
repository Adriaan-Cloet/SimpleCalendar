//! One sync cycle for one source (`docs/architecture.md` §7.2).

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use tokio::sync::mpsc;

use super::store::{self, LocalCalendar};
use super::{SyncState, SyncStatus};
use crate::CoreEvent;
use crate::db::{Db, DbError};
use crate::model::SourceKind;
use crate::outbox;
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
    pub(super) fn emit(&self, event: CoreEvent) {
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
pub(super) enum SyncError {
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
        // The local source is never synced. A remote one without a provider, such as a Google
        // account before the Google provider exists, says so: a UI that asked for a sync would
        // otherwise wait for a status forever.
        if source.kind != SourceKind::Local {
            log::warn!("source {source_id}: no provider for {:?}", source.kind);
            let message = format!("{} calendars cannot be synced yet.", kind_name(source.kind));
            let (id, stored) = (source_id.to_owned(), message.clone());
            if let Err(e) = ctx
                .db
                .write(move |c| store::set_status(c, &id, SyncState::Error, Some(&stored)))
                .await
            {
                log::error!("source {source_id}: cannot store its status: {e}");
            }
            ctx.emit_status(source_id, SyncState::Error, Some(message));
        }
        return None;
    };
    // Rust note: `?` on an `Option` returns `None` from this function when there is no
    // interval at all.
    let interval = super::interval(source.kind, source.interval_s)?;
    if source.auth_required && !manual {
        return Some(interval);
    }

    ctx.emit_status(source_id, SyncState::Syncing, None);
    let result = sync_source(
        ctx,
        source_id,
        &source.display_name,
        provider.as_ref(),
        manual,
    )
    .await;
    let state = match &result {
        Ok(()) => SyncState::Ok,
        Err(SyncError::Provider(ProviderError::Auth)) => SyncState::AuthRequired,
        Err(SyncError::Provider(ProviderError::Offline)) => SyncState::Offline,
        Err(_) => SyncState::Error,
    };
    let last_error = result.err().map(|e| {
        log::warn!("source {source_id}: sync failed: {e}");
        e.to_string()
    });

    let (id, message) = (source_id.to_owned(), last_error.clone());
    if let Err(e) = ctx
        .db
        .write(move |c| store::set_status(c, &id, state, message.as_deref()))
        .await
    {
        log::error!("source {source_id}: cannot store its status: {e}");
    }
    ctx.emit_status(source_id, state, last_error);
    // Without a connection, look again soon: the indicator should not say "offline" for an
    // hour after the network is back.
    let interval = match state {
        SyncState::Offline => interval.min(OFFLINE_RETRY),
        _ => interval,
    };
    Some(next_cycle(ctx, source_id, interval).await)
}

/// How soon a source that could not connect is tried again, if its interval is longer.
pub(super) const OFFLINE_RETRY: Duration = Duration::from_secs(60);

/// When the timer should sync `source_id` again: after `interval`, or sooner when an outbox row
/// may be tried again before then (§7.3).
async fn next_cycle(ctx: &Ctx, source_id: &str, interval: Duration) -> Duration {
    let id = source_id.to_owned();
    match ctx
        .db
        .read(move |c| outbox::next_retry(c, &id, now()))
        .await
    {
        // Rust note: `max(1)` makes the difference positive, so the cast to `u64` is exact.
        Ok(Some(at)) => interval.min(Duration::from_secs((at - now()).max(1) as u64)),
        Ok(None) => interval,
        Err(e) => {
            log::error!("source {source_id}: cannot read its outbox: {e}");
            interval
        }
    }
}

/// Steps 1 to 4 of §7.2. Step 5, notifying, happens after every commit.
async fn sync_source(
    ctx: &Ctx,
    source_id: &str,
    display_name: &str,
    provider: &dyn CalendarProvider,
    manual: bool,
) -> Result<(), SyncError> {
    // 1. Authenticate. For the read-only sources of v0.1 that means reading the secret.
    let cx = SourceContext {
        source_id: source_id.to_owned(),
        display_name: display_name.to_owned(),
        secret: ctx.secrets.get(source_id).await?,
    };

    // 2. Push, so the pull below already sees our own writes. A row that cannot be sent now
    // ends the cycle before the pull. A sync the user asked for does not wait for backoff.
    super::push::push(ctx, &cx, provider, manual).await?;

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
        let (returned, applied) = ctx
            .db
            .write(move |c| {
                let tx = c.transaction()?;
                let mut applied =
                    store::apply_page(&tx, calendar_id, &upserts, &deletions, &mut seen)?;
                if let Some((cursor, warnings)) = last {
                    if full {
                        applied.changed |= store::finish_full_pull(&tx, calendar_id, &seen)?;
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
                Ok((seen, applied))
            })
            .await?;
        seen = returned;

        // After the commit, so the re-query it triggers sees the page.
        if applied.changed {
            ctx.emit(CoreEvent::CalendarChanged {
                calendar_ids: vec![calendar_id],
            });
        }
        for conflict in applied.conflicts {
            ctx.emit(CoreEvent::SyncConflict(conflict));
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

/// The name of a kind of source in a message to the user.
fn kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Local => "Local",
        SourceKind::Ics => "Subscribed",
        SourceKind::Google => "Google",
        SourceKind::Caldav => "CalDAV",
    }
}
