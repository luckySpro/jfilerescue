//! Tauri backend: thin IPC layer over recovery-core / recovery-platform.
//! All disk access is read-only; destination checks are enforced here, not
//! only in the UI.

mod commands;
mod state;

use tauri::Manager;

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(state::AppState::new(data_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::privilege_state,
            commands::list_disks,
            commands::busy_writers,
            commands::relaunch_elevated,
            commands::open_privacy_settings,
            commands::start_scan,
            commands::scan_progress,
            commands::pause_scan,
            commands::cancel_scan,
            commands::list_sessions,
            commands::load_session,
            commands::query_session,
            commands::session_tree,
            commands::session_types,
            commands::delete_session,
            commands::export_csv,
            commands::preview_file,
            commands::check_destination,
            commands::start_recover,
            commands::recover_progress,
            commands::create_image,
            commands::list_signatures,
            commands::upsert_signature,
            commands::set_signature_enabled,
            commands::delete_signature,
            commands::restore_signature_defaults,
            commands::export_signatures,
            commands::import_signatures,
            commands::test_signature,
            commands::diagnostics,
            commands::data_dir,
            commands::sessions_bytes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running JFileRescue");
}
