//! `#[tauri::command]` wrappers around `CalendarService` (`docs/architecture.md` §10). They
//! contain no logic: errors arrive in the frontend as `{kind, message}` objects.
//
// Rust note: Tauri requires an async command that borrows managed state (`State<'_, _>`) to
// return a `Result`. `get_status` cannot fail, hence its `()` error type.

use simplecalendar_core::CalendarService;
use simplecalendar_core::model::Occurrence;
use simplecalendar_core::service::{
    CalendarInfo, CalendarUpdate, EventDetail, EventDraft, GoogleClientInfo, OccurrenceRef, Scope,
    ServiceError, SourceInfo, Status,
};
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

/// Every occurrence on a visible calendar from date `from` up to date `to` (`YYYY-MM-DD`, end
/// exclusive), in the IANA zone `tz`.
#[tauri::command]
pub async fn list_occurrences(
    service: State<'_, CalendarService>,
    from: String,
    to: String,
    tz: String,
) -> Result<Vec<Occurrence>, ServiceError> {
    service.list_occurrences(&from, &to, &tz).await
}

/// Every calendar, grouped by source.
#[tauri::command]
pub async fn list_calendars(
    service: State<'_, CalendarService>,
) -> Result<Vec<CalendarInfo>, ServiceError> {
    service.list_calendars().await
}

/// Shows or hides a calendar, sets its colour (`null` goes back to the provider's) or renames
/// a local calendar. Fields left out of `update` stay as they are.
#[tauri::command]
pub async fn update_calendar(
    service: State<'_, CalendarService>,
    calendar: i64,
    update: CalendarUpdate,
) -> Result<CalendarInfo, ServiceError> {
    service.update_calendar(calendar, update).await
}

/// Creates a calendar on this computer.
#[tauri::command]
pub async fn create_local_calendar(
    service: State<'_, CalendarService>,
    name: String,
) -> Result<CalendarInfo, ServiceError> {
    service.create_local_calendar(&name).await
}

/// Deletes a calendar on this computer with its events.
#[tauri::command]
pub async fn remove_calendar(
    service: State<'_, CalendarService>,
    calendar: i64,
) -> Result<(), ServiceError> {
    service.remove_calendar(calendar).await
}

/// One event or occurrence in full, for the editor and the popover.
#[tauri::command]
pub async fn get_event(
    service: State<'_, CalendarService>,
    target: OccurrenceRef,
) -> Result<EventDetail, ServiceError> {
    service.get_event(target).await
}

/// Creates an event and returns its occurrence, with times in the IANA zone `tz`.
#[tauri::command]
pub async fn create_event(
    service: State<'_, CalendarService>,
    draft: EventDraft,
    tz: String,
) -> Result<Occurrence, ServiceError> {
    service.create_event(draft, &tz).await
}

/// Replaces an event, or one or all occurrences of a series, with `draft`. Returns the changed
/// occurrence with times in the zone `tz`, or `null` if the series no longer has it.
#[tauri::command]
pub async fn update_event(
    service: State<'_, CalendarService>,
    target: OccurrenceRef,
    scope: Scope,
    draft: EventDraft,
    tz: String,
) -> Result<Option<Occurrence>, ServiceError> {
    service.update_event(target, scope, draft, &tz).await
}

/// Deletes an event, or one or all occurrences of a series.
#[tauri::command]
pub async fn delete_event(
    service: State<'_, CalendarService>,
    target: OccurrenceRef,
    scope: Scope,
) -> Result<(), ServiceError> {
    service.delete_event(target, scope).await
}

/// Signs in to a Google account in the system browser. Resolves when the browser step
/// finishes. With `source`, signs that account in again.
#[tauri::command]
pub async fn begin_google_sign_in(
    service: State<'_, CalendarService>,
    source: Option<String>,
) -> Result<SourceInfo, ServiceError> {
    // The browser is opened from Rust, so the frontend needs no permission for it (§10).
    // Rust note: `that_detached` starts the browser as its own process and does not wait for
    // it, so the sign-in goes on waiting for the redirect instead.
    let open = |url: &str| open::that_detached(url).map_err(|e| e.to_string());
    service.begin_google_sign_in(source.as_deref(), open).await
}

/// Stops a Google sign-in that is waiting for the browser.
#[tauri::command]
pub fn cancel_google_sign_in(service: State<'_, CalendarService>) {
    service.cancel_google_sign_in();
}

/// Which OAuth client Google sign-in uses.
#[tauri::command]
pub async fn get_google_client(
    service: State<'_, CalendarService>,
) -> Result<GoogleClientInfo, ServiceError> {
    service.google_client().await
}

/// Sets the user's own OAuth client; an empty `client_id` goes back to the built-in one.
#[tauri::command]
pub async fn set_google_client(
    service: State<'_, CalendarService>,
    client_id: String,
    client_secret: String,
) -> Result<GoogleClientInfo, ServiceError> {
    service.set_google_client(&client_id, &client_secret).await
}
