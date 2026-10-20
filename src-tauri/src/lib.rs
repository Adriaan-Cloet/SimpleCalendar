mod commands;

use std::sync::Arc;

use serde::Serialize;
use simplecalendar_core::CoreEvent;
use simplecalendar_core::db::Db;
use simplecalendar_core::provider::Providers;
use simplecalendar_core::provider::ics::IcsProvider;
use simplecalendar_core::secrets::{KeychainStore, Secrets};
use simplecalendar_core::sync::SyncEngine;
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
            app.manage(db.clone());

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
            app.manage(secrets.clone());

            // The sync engine (§7.1), with a provider for every kind of remote source.
            let providers = Providers::new().with(Arc::new(IcsProvider::new()?));
            let (events, rx) = mpsc::unbounded_channel();
            let (engine, sync) = SyncEngine::new(db, secrets, providers, events);
            tauri::async_runtime::spawn(engine.run());
            tauri::async_runtime::spawn(forward_events(app.handle().clone(), rx));
            // The "app start" trigger.
            sync.sync_all();
            app.manage(sync);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::sync_now
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
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
