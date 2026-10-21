mod commands;

use std::sync::Arc;

use serde::Serialize;
use simplecalendar_core::db::Db;
use simplecalendar_core::google::{ClientConfig, Endpoints, GoogleAuth};
use simplecalendar_core::provider::Providers;
use simplecalendar_core::provider::google::GoogleProvider;
use simplecalendar_core::provider::ics::IcsProvider;
use simplecalendar_core::secrets::{KeychainStore, Secrets};
use simplecalendar_core::sync::SyncEngine;
use simplecalendar_core::{CalendarService, CoreEvent};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            log::info!("simplecalendar-core {}", simplecalendar_core::version());

            // The app-data directory comes from the bundle identifier in tauri.conf.json.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = Db::open(&data_dir.join("simplecalendar.db"))?;

            // Keychain entries use the bundle identifier as service name (§8). The probe runs
            // in the background: on Linux it may wait for an unlock prompt, and without a
            // keyring the app still starts, for local calendars.
            let secrets = Secrets::new(Arc::new(KeychainStore::new(
                app.config().identifier.clone(),
            )));
            let probe = secrets.clone();
            tauri::async_runtime::spawn(async move {
                match probe.probe().await {
                    Ok(()) => log::info!("keychain available"),
                    Err(unavailable) => {
                        log::warn!("keychain unavailable: {:?}", unavailable.reason)
                    }
                }
            });

            // Google sign-in and access tokens, shared by the service and the provider (§8).
            let google = Arc::new(GoogleAuth::new(
                db.clone(),
                secrets.clone(),
                compiled_google_client(),
                Endpoints::google(),
            )?);

            // The sync engine (§7.1), with a provider for every kind of remote source.
            let providers = Providers::new()
                .with(Arc::new(IcsProvider::new()?))
                .with(Arc::new(GoogleProvider::new(google.clone())));
            let (events, rx) = mpsc::unbounded_channel();
            let (engine, sync) =
                SyncEngine::new(db.clone(), secrets.clone(), providers, events.clone());
            tauri::async_runtime::spawn(engine.run());
            tauri::async_runtime::spawn(forward_events(app.handle().clone(), rx));
            // The "app start" trigger.
            sync.sync_all();

            // Every command goes through the service (§2).
            app.manage(CalendarService::new(db, secrets, sync, events, google));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::list_sources,
            commands::add_ics_source,
            commands::remove_source,
            commands::sync_now,
            commands::list_occurrences,
            commands::list_calendars,
            commands::update_calendar,
            commands::create_local_calendar,
            commands::remove_calendar,
            commands::begin_google_sign_in,
            commands::cancel_google_sign_in,
            commands::get_google_client,
            commands::set_google_client
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

/// The Google OAuth client compiled into this build, if `build.rs` found one (§8).
//
// Rust note: `option_env!` reads a variable at compile time and gives `None` when it is not
// set, so a build without the variables still compiles.
fn compiled_google_client() -> Option<ClientConfig> {
    let client_id = option_env!("SIMPLECALENDAR_GOOGLE_CLIENT_ID")?;
    Some(ClientConfig {
        client_id: client_id.to_owned(),
        client_secret: option_env!("SIMPLECALENDAR_GOOGLE_CLIENT_SECRET").map(str::to_owned),
    })
}

/// The payload of `calendar:changed`.
#[derive(Clone, Serialize)]
struct CalendarChanged {
    calendar_ids: Vec<i64>,
}

/// Passes what the core reports on to the webview (§2, §10), until the core stops.
async fn forward_events(app: AppHandle, mut rx: mpsc::UnboundedReceiver<CoreEvent>) {
    while let Some(event) = rx.recv().await {
        let sent = match event {
            CoreEvent::CalendarChanged { calendar_ids } => {
                app.emit("calendar:changed", CalendarChanged { calendar_ids })
            }
            CoreEvent::SyncStatus(status) => app.emit("sync:status", status),
        };
        if let Err(e) = sent {
            log::warn!("cannot send an event to the webview: {e}");
        }
    }
}
