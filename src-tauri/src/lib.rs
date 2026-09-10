pub mod domain;
pub mod storage;
pub mod system_overview;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            system_overview::get_system_overview
        ])
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
