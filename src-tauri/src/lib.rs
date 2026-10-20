mod commands;

use std::sync::Arc;

use simplecalendar_core::db::Db;
use simplecalendar_core::secrets::{KeychainStore, Secrets};
use tauri::Manager;

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
            app.manage(db);

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
            app.manage(secrets);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::get_status])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
