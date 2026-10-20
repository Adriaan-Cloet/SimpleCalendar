//! `#[tauri::command]` wrappers around `CalendarService` (`docs/architecture.md` §10). They
//! contain no logic: errors arrive in the frontend as `{kind, message}` objects.
//
// Rust note: Tauri requires an async command that borrows managed state (`State<'_, _>`) to
// return a `Result`. `get_status` cannot fail, hence its `()` error type.

use simplecalendar_core::CalendarService;
use simplecalendar_core::service::{ServiceError, SourceInfo, Status};
use tauri::State;

/// The app's status. If the keychain was unavailable, it is probed again first, so a "Try
/// again" button only calls this.
#[tauri::command]
pub async fn get_status(service: State<'_, CalendarService>) -> Result<Status, ()> {
    Ok(service.status().await)
}

/// Every remote source, oldest first.
#[tauri::command]
pub async fn list_sources(
    service: State<'_, CalendarService>,
) -> Result<Vec<SourceInfo>, ServiceError> {
    service.list_sources().await
}

/// Subscribes to an ICS feed. `name` may be empty.
#[tauri::command]
pub async fn add_ics_source(
    service: State<'_, CalendarService>,
    url: String,
    name: String,
) -> Result<SourceInfo, ServiceError> {
    service.add_ics_source(&url, &name).await
}

/// Removes a source, its calendars and events, and its keychain entry.
#[tauri::command]
pub async fn remove_source(
    service: State<'_, CalendarService>,
    source: String,
) -> Result<(), ServiceError> {
    service.remove_source(&source).await
}

/// Syncs one source now, or every source when `source` is left out. Returns at once; progress
/// arrives as `sync:status` events.
#[tauri::command]
pub fn sync_now(service: State<'_, CalendarService>, source: Option<String>) {
    service.sync_now(source);
}
