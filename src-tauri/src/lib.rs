pub mod commands;
pub mod index;
pub mod search;
pub mod state;

use tauri::Manager;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // All indexes live under <app-data>/indexes.
            let root = app.path().app_data_dir()?.join("indexes");
            app.manage(AppState::new(root));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_indexes,
            commands::create_index,
            commands::build_index,
            commands::delete_index,
            commands::search,
            commands::open_path,
            commands::reveal_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
