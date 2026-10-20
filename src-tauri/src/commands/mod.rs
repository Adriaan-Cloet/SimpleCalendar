//! `#[tauri::command]` wrappers around the core (`docs/architecture.md` §10).

use serde::Serialize;
use simplecalendar_core::secrets::{Secrets, SecretsUnavailable};
use simplecalendar_core::sync::SyncHandle;
use tauri::State;

/// What `get_status` reports. Sources needing sign-in and open conflicts join this in later
/// versions (§10).
#[derive(Debug, Serialize)]
pub struct Status {
    /// `None` while the keychain is usable. Otherwise the structured `secrets_unavailable`
    /// error, with the message to show.
    pub secrets_unavailable: Option<SecretsUnavailable>,
}

/// The app's status. If the keychain was unavailable, it is probed again first, so a "Try
/// again" button only calls this.
//
// Rust note: Tauri requires an async command that borrows managed state (`State<'_, _>`) to
// return a `Result`. This one cannot fail, hence the `()` error type.
#[tauri::command]
pub async fn get_status(secrets: State<'_, Secrets>) -> Result<Status, ()> {
    Ok(Status {
        secrets_unavailable: secrets.status().await,
    })
}

/// Syncs one source now, or every source when `source` is left out. Returns at once; progress
/// arrives as `sync:status` events.
#[tauri::command]
pub fn sync_now(sync: State<'_, SyncHandle>, source: Option<String>) {
    sync.sync_now(source);
}
