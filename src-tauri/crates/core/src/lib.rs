//! Calendar logic for SimpleCalendar: model, storage, recurrence and sync.
//!
//! This crate must never depend on `tauri`. The app crate in `src-tauri/` wraps it in
//! Tauri commands; see `docs/architecture.md` §3.

pub mod db;
pub mod google;
pub mod model;
pub mod outbox;
pub mod provider;
pub mod recurrence;
pub mod reminders;
pub mod secrets;
pub mod service;
pub mod sync;

pub use service::CalendarService;

/// What the core reports to the shell. The app crate forwards these to the webview (§2, §10).
///
/// They are sent after the change they describe is committed, so a re-query sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEvent {
    /// Events or settings of these calendars changed (`calendar:changed`).
    CalendarChanged { calendar_ids: Vec<i64> },
    /// A source started or finished a sync cycle (`sync:status`).
    SyncStatus(sync::SyncStatus),
    /// A change could not be saved because the event changed elsewhere (`sync:conflict`).
    SyncConflict(sync::SyncConflict),
    /// A reminder is due: the shell shows it as a desktop notification (§9).
    ReminderDue(reminders::ReminderDue),
}

/// Version of this crate, as set in its `Cargo.toml`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!version().is_empty());
    }
}
