//! The reminder scheduler (`docs/architecture.md` §9).
//!
//! One task looks at the reminders of the next 24 hours, shows the ones that are due and sleeps
//! until the next. A change to the database or the display zone wakes it early, through a
//! `ReminderHandle`. Every reminder is recorded in `fired_reminders` before it is reported, so it
//! is shown once, also across a restart. The task does not know whether a window exists: it
//! reports `CoreEvent::ReminderDue`, and the shell decides how to show it.

mod plan;
mod store;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use tokio::sync::{Notify, mpsc};

use crate::CoreEvent;
use crate::db::{Db, DbError};

pub(crate) use store::set_display_zone;

/// How far ahead the scheduler looks (§9).
const HORIZON: i64 = 24 * 3600;

/// The longest the scheduler waits without looking at the clock. A sleep is measured on a clock
/// that stops while the computer is suspended; checking the wall clock this often keeps a
/// reminder from coming late by the length of a suspend.
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// The payload of `CoreEvent::ReminderDue`: one reminder to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderDue {
    /// Together with `original_start`, the occurrence, as `list_occurrences` identifies it.
    pub event_id: i64,
    pub original_start: i64,
    pub title: String,
    /// "Today at 14:00", and the location on a second line if the event has one.
    pub body: String,
    /// The day the occurrence starts in the display zone, to open it in the day view.
    pub date: NaiveDate,
}

/// Wakes the scheduler to look again. Cloning is cheap.
#[derive(Debug, Clone, Default)]
pub struct ReminderHandle {
    wake: Arc<Notify>,
}

impl ReminderHandle {
    /// Events, calendars or the display zone changed: the reminders are computed again. Calls
    /// that come while the scheduler is busy lead to one more look, not one each.
    pub fn changed(&self) {
        // Rust note: `notify_one` stores a permit when nobody is waiting, so a change that comes
        // while the scheduler is working is not lost: its next wait ends at once.
        self.wake.notify_one();
    }
}

/// The current time in epoch seconds. Tests replace it with a clock they move by hand.
type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The scheduler. Create it with `new`, then drive `run` on the async runtime.
pub struct ReminderScheduler {
    db: Db,
    events: mpsc::UnboundedSender<CoreEvent>,
    wake: Arc<Notify>,
    clock: Clock,
}

impl ReminderScheduler {
    /// A scheduler that `handle` wakes and that reports due reminders on `events`.
    pub fn new(
        db: Db,
        events: mpsc::UnboundedSender<CoreEvent>,
        handle: &ReminderHandle,
    ) -> ReminderScheduler {
        let clock: Clock = Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64)
        });
        ReminderScheduler::with_clock(db, events, handle, clock)
    }

    fn with_clock(
        db: Db,
        events: mpsc::UnboundedSender<CoreEvent>,
        handle: &ReminderHandle,
        clock: Clock,
    ) -> ReminderScheduler {
        ReminderScheduler {
            db,
            events,
            wake: Arc::clone(&handle.wake),
            clock,
        }
    }

    /// Runs as long as the app does. Rows of reminders shown more than a week ago are pruned
    /// first.
    pub async fn run(self) {
        let now = (self.clock)();
        match self.db.write(move |c| store::prune(c, now)).await {
            Ok(0) => {}
            Ok(n) => log::info!("reminders: pruned {n} old rows"),
            Err(e) => log::error!("reminders: cannot prune old rows: {e}"),
        }
        loop {
            let now = (self.clock)();
            let next = match self.check(now).await {
                Ok(next) => next,
                Err(e) => {
                    log::error!("reminders: {e}");
                    None
                }
            };
            self.wait_until(next.unwrap_or(now + HORIZON).min(now + HORIZON))
                .await;
        }
    }

    /// Shows the reminders that are due at `now`. When the next one is due, if one is in the
    /// next 24 hours. Nothing happens before the frontend has reported a display zone: without
    /// it, all-day and floating events have no time, and no text can say "today".
    async fn check(&self, now: i64) -> Result<Option<i64>, DbError> {
        let found = self
            .db
            .read(move |c| {
                let Some(zone) = store::display_zone(c)? else {
                    return Ok(None);
                };
                let candidates = store::candidates(c, now, now + HORIZON, zone)?;
                Ok(Some((zone, plan::plan(candidates, &store::fired(c)?, now))))
            })
            .await?;
        let Some((zone, plan)) = found else {
            return Ok(None);
        };
        let today = today(now, zone);
        for due in plan.due {
            let key = due.key;
            if !self.db.write(move |c| store::fire(c, key, now)).await? {
                continue;
            }
            let o = due.occurrence;
            let mut body = plan::when(o.start, today);
            if let Some(location) = o.location.as_deref().filter(|l| !l.is_empty()) {
                body.push('\n');
                body.push_str(location);
            }
            let reminder = ReminderDue {
                event_id: o.event_id,
                original_start: o.original_start,
                title: o.title,
                body,
                date: o.start.date(),
            };
            // Only fails when the shell has stopped listening, and then nobody would see it.
            let _ = self.events.send(CoreEvent::ReminderDue(reminder));
        }
        Ok(plan.next)
    }

    /// Waits until the wall clock reaches `at`, or until `changed` is called.
    async fn wait_until(&self, at: i64) {
        loop {
            let left = at - (self.clock)();
            if left <= 0 {
                return;
            }
            let step = CHECK_EVERY.min(Duration::from_secs(left as u64));
            if tokio::time::timeout(step, self.wake.notified())
                .await
                .is_ok()
            {
                return;
            }
        }
    }
}

/// The date at `now` in `zone`.
fn today(now: i64, zone: Tz) -> NaiveDate {
    DateTime::from_timestamp(now, 0)
        .unwrap_or_default()
        .with_timezone(&zone)
        .date_naive()
}
