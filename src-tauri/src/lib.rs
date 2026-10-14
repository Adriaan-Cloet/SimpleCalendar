use simplecalendar_core::db::Db;
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
