pub mod debug;
pub mod domain;
pub mod performance;
pub mod recent_files;
pub mod startup;
pub mod startup_inventory;
pub mod storage;
pub mod system_overview;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            debug::get_debug_mode,
            debug::set_debug_mode,
            debug::get_debug_log,
            debug::record_client_error,
            startup::get_startup_registration,
            startup::set_startup_registration,
            system_overview::get_system_overview,
            performance::get_performance_details,
            startup_inventory::list_startup_items,
            startup_inventory::set_startup_item_enabled,
            recent_files::list_recent_files,
            recent_files::refresh_recent_files,
            recent_files::open_recent_file,
            recent_files::open_recent_file_location
        ])
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
