mod commands;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use simplecalendar_core::db::Db;
use simplecalendar_core::google::{ClientConfig, Endpoints, GoogleAuth};
use simplecalendar_core::provider::Providers;
use simplecalendar_core::provider::caldav::CaldavProvider;
use simplecalendar_core::provider::google::GoogleProvider;
use simplecalendar_core::provider::ics::IcsProvider;
use simplecalendar_core::reminders::{ReminderDue, ReminderHandle, ReminderScheduler};
use simplecalendar_core::secrets::{KeychainStore, Secrets};
use simplecalendar_core::sync::SyncEngine;
use simplecalendar_core::{CalendarService, CoreEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::mpsc;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
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
            // The service shares the CalDAV provider, to check a login before adding it.
            let caldav = Arc::new(CaldavProvider::new()?);
            let providers = Providers::new()
                .with(Arc::new(IcsProvider::new()?))
                .with(Arc::new(GoogleProvider::new(google.clone())))
                .with(caldav.clone());
            let (events, rx) = mpsc::unbounded_channel();
            let (engine, sync) =
                SyncEngine::new(db.clone(), secrets.clone(), providers, events.clone());
            tauri::async_runtime::spawn(engine.run());
            // The reminder scheduler (§9), woken by every change the core reports.
            let reminders = ReminderHandle::default();
            let scheduler = ReminderScheduler::new(db.clone(), events.clone(), &reminders);
            tauri::async_runtime::spawn(scheduler.run());
            tauri::async_runtime::spawn(forward_events(
                app.handle().clone(),
                rx,
                reminders.clone(),
            ));
            // A click on a reminder opens its event.
            let handle = app.handle().clone();
            app.notification().on_action(move |action| {
                if action.action_id() != "tap" {
                    return;
                }
                let open = action
                    .notification()
                    .and_then(|n| n.extra().get(OPEN_KEY))
                    .and_then(|v| serde_json::from_value::<ReminderOpen>(v.clone()).ok());
                if let Some(open) = open {
                    open_reminder(&handle, open);
                }
            })?;
            // The "app start" trigger.
            sync.sync_all();

            // Every command goes through the service (§2).
            app.manage(CalendarService::new(
                db, secrets, sync, events, google, caldav, reminders,
            ));
            Ok(())
        })
        // The "window gains focus" trigger (§7.1); the engine keeps it to once a minute.
        // `try_state`, because the service only exists once `setup` has run.
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(true) = event
                && let Some(service) = window.try_state::<CalendarService>()
            {
                service.window_focused();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::list_sources,
            commands::add_ics_source,
            commands::add_caldav_source,
            commands::update_caldav_password,
            commands::remove_source,
            commands::sync_now,
            commands::set_sync_interval,
            commands::list_occurrences,
            commands::list_calendars,
            commands::update_calendar,
            commands::create_local_calendar,
            commands::remove_calendar,
            commands::get_event,
            commands::create_event,
            commands::update_event,
            commands::delete_event,
            commands::list_sync_issues,
            commands::dismiss_sync_issue,
            commands::redo_sync_issue,
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

/// The payload of `reminder:open`: the occurrence whose reminder was clicked, and its day in the
/// display zone. It travels inside the notification, which hands it back on a click.
#[derive(Clone, Serialize, Deserialize)]
struct ReminderOpen {
    event_id: i64,
    original_start: i64,
    /// `YYYY-MM-DD`
    date: String,
}

/// The key of `ReminderOpen` in a notification's extra data.
const OPEN_KEY: &str = "open";

/// Passes what the core reports on to the webview (§2, §10) and wakes the reminder scheduler
/// after a change, until the core stops.
async fn forward_events(
    app: AppHandle,
    mut rx: mpsc::UnboundedReceiver<CoreEvent>,
    reminders: ReminderHandle,
) {
    while let Some(event) = rx.recv().await {
        let sent = match event {
            CoreEvent::CalendarChanged { calendar_ids } => {
                reminders.changed();
                app.emit("calendar:changed", CalendarChanged { calendar_ids })
            }
            CoreEvent::SyncStatus(status) => app.emit("sync:status", status),
            CoreEvent::SyncConflict(conflict) => app.emit("sync:conflict", conflict),
            CoreEvent::ReminderDue(reminder) => {
                notify(&app, reminder);
                Ok(())
            }
        };
        if let Err(e) = sent {
            log::warn!("cannot send an event to the webview: {e}");
        }
    }
}

/// Shows a reminder as a desktop notification: the title, then when it starts and where.
fn notify(app: &AppHandle, reminder: ReminderDue) {
    let open = ReminderOpen {
        event_id: reminder.event_id,
        original_start: reminder.original_start,
        date: reminder.date.format("%Y-%m-%d").to_string(),
    };
    let shown = app
        .notification()
        .builder()
        .title(reminder.title)
        .body(reminder.body)
        .extra(OPEN_KEY, open)
        .show();
    if let Err(e) = shown {
        log::warn!("cannot show a reminder: {e}");
    }
}

/// Brings the window to the front and asks the frontend to open the occurrence in the day view.
fn open_reminder(app: &AppHandle, open: ReminderOpen) {
    if let Some(window) = app.get_webview_window("main") {
        // Each may fail on a platform that does not support it; the event is still opened.
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    if let Err(e) = app.emit("reminder:open", open) {
        log::warn!("cannot open a reminder's event: {e}");
    }
}
