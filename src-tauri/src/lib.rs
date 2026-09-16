pub mod app_icons;
pub mod audio;
pub mod debug;
pub mod disk_analyzer;
pub mod disk_cleanup;
pub mod domain;
pub mod performance;
pub mod performance_devices;
pub mod process;
pub mod process_usage;
pub mod recent_files;
pub mod single_instance;
pub mod startup;
pub mod startup_inventory;
pub mod storage;
pub mod subscription_usage;
pub mod system_overview;
pub mod tray;

use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .with_filter(|label| label == "main")
                .build(),
        )
        .setup(|app| {
            #[cfg(desktop)]
            {
                if let Err(e) = tray::setup_tray(app) {
                    eprintln!("Erro ao configurar o tray icon do Pulse: {e}");
                }
                if startup::should_start_minimized(&std::env::args().collect::<Vec<_>>()) {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.hide();
                    }
                }
            }
            subscription_usage::init(app.handle().clone());
            disk_analyzer::init(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "main" {
                    api.prevent_close();
                    if tray::should_minimize_on_close() {
                        let _ = window.emit("pulse://foreground-changed", false);
                        let _ = window.hide();
                    } else {
                        let _ = window.emit("pulse://close-requested", ());
                    }
                } else if window.label() == "tray-flyout" {
                    api.prevent_close();
                    let _ = window.emit("pulse://foreground-changed", false);
                    let _ = window.hide();
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                let _ = window.emit("pulse://foreground-changed", *focused);
                if !*focused && window.label() == "tray-flyout" {
                    // Fecha a barra flutuante automaticamente quando o usuário clica fora
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            debug::get_debug_mode,
            debug::set_debug_mode,
            debug::get_debug_log,
            debug::get_debug_audit_info,
            debug::clear_debug_log,
            debug::record_client_error,
            audio::get_audio_devices,
            audio::set_default_audio_device,
            startup::get_startup_registration,
            startup::set_startup_registration,
            system_overview::get_system_overview,
            system_overview::terminate_process,
            performance::get_performance_details,
            performance_devices::get_performance_devices,
            startup_inventory::list_startup_items,
            startup_inventory::set_startup_item_enabled,
            recent_files::list_recent_files,
            recent_files::refresh_recent_files,
            recent_files::open_recent_file,
            recent_files::open_recent_file_location,
            recent_files::get_custom_scan_folders,
            recent_files::add_custom_scan_folder,
            recent_files::remove_custom_scan_folder,
            tray::get_current_window_label,
            tray::is_current_window_foreground,
            tray::minimize_to_tray,
            tray::minimize_to_tray_and_remember,
            tray::get_minimize_on_close,
            tray::show_main_window,
            tray::toggle_tray_flyout,
            tray::hide_tray_flyout,
            tray::set_tray_icon_theme,
            tray::exit_app,
            disk_analyzer::get_system_drives,
            disk_analyzer::start_disk_scan,
            disk_analyzer::cancel_disk_scan,
            disk_analyzer::is_disk_scanning,
            disk_analyzer::get_disk_scan_state,
            disk_analyzer::get_cached_disk_scan,
            disk_analyzer::open_disk_item,
            disk_cleanup::analyze_disk_cleanup,
            disk_cleanup::clean_disk_categories,
            subscription_usage::get_subscription_usage,
            subscription_usage::get_subscription_settings,
            subscription_usage::set_openrouter_api_key,
            subscription_usage::clear_openrouter_api_key,
            subscription_usage::set_opencode_go_api_key,
            subscription_usage::clear_opencode_go_api_key,
            subscription_usage::set_provider_enabled
        ])
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
