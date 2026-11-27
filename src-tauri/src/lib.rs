pub mod ai_disk_advisor;
pub mod ai_services_advisor;
pub mod app_icons;
pub mod app_updates;
pub mod audio;
pub mod beta_features;
pub mod cleanup_os;
pub mod custom_dock;
pub mod debug;
pub mod dev_ports;
pub mod disk_analyzer;
pub mod disk_cleanup;
pub mod domain;
pub mod file_explorer;
pub mod hardware_telemetry;
pub mod launcher;
pub mod network_security;
pub mod performance;
pub mod performance_devices;
pub mod peripheral_battery;
pub mod platform_info;
pub mod privileges;
pub mod process;
pub mod process_identity;
pub mod process_usage;
pub mod quota_history;
pub mod recent_files;
pub mod security_audit;
pub mod self_update;
pub mod services;
#[cfg(windows)]
pub mod shell_launch;
pub mod single_instance;
pub mod startup;
pub mod startup_inventory;
pub mod storage;
pub mod subscription_usage;
pub mod system_overview;
pub mod tray;
pub mod uninstall;
pub mod window_shape;

use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // POSITION faltava: a janela reabria sempre centralizada, no
                // tamanho salvo. O filtro cobria só `main`, então o gerenciador
                // de arquivos (`files`) não persistia nada.
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED
                        | tauri_plugin_window_state::StateFlags::FULLSCREEN,
                )
                .with_filter(|label| matches!(label, "main" | "files"))
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
            for label in ["launcher", "tray-flyout"] {
                if let Some(window) = app.get_webview_window(label) {
                    window_shape::apply_to_known_window(&window);
                }
            }
            subscription_usage::init(app.handle().clone());
            disk_analyzer::init(app.handle().clone());
            disk_cleanup::init(app.handle().clone());
            self_update::init(app.handle().clone());
            custom_dock::init(app.handle().clone());
            launcher::init(app.handle().clone());
            {
                let beta = beta_features::load();
                if beta.pulse_runner && beta.pulse_runner_win_key {
                    std::thread::spawn(|| {
                        launcher::apply_win_key_hook(true);
                    });
                }
            }
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
                } else if matches!(window.label(), "tray-flyout" | "launcher" | "files") {
                    api.prevent_close();
                    let _ = window.emit("pulse://foreground-changed", false);
                    let _ = window.hide();
                }
            }
            tauri::WindowEvent::Resized(_) => {
                // O Shell redimensiona `tray-flyout` por modo; o recorte precisa
                // acompanhar, senão os cantos voltam a aparecer no novo tamanho.
                if let Some(webview) = window.app_handle().get_webview_window(window.label()) {
                    window_shape::apply_to_known_window(&webview);
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                let _ = window.emit("pulse://foreground-changed", *focused);
                if !*focused && window.label() == "launcher" {
                    // Fecha o Pulse Runner ao perder o foco e libera a Dock presa
                    // por ele (senão ela ficava visível sozinha, sem o runner).
                    launcher::on_launcher_focus_lost(window.app_handle());
                } else if !*focused && window.label() == "tray-flyout" {
                    // Fecha o painel rápido automaticamente ao clicar fora
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            platform_info::get_platform_info,
            debug::get_debug_mode,
            debug::set_debug_mode,
            debug::get_debug_log,
            debug::get_debug_audit_info,
            debug::clear_debug_log,
            debug::record_client_error,
            dev_ports::get_dev_ports,
            dev_ports::kill_dev_port,
            audio::get_audio_devices,
            audio::set_default_audio_device,
            audio::get_audio_mixer,
            audio::set_audio_mixer_app_volume,
            audio::set_audio_mixer_app_muted,
            audio::set_audio_mixer_master,
            startup::get_startup_registration,
            startup::set_startup_registration,
            self_update::get_self_update_status,
            self_update::set_auto_self_update,
            self_update::check_self_update,
            system_overview::get_system_overview,
            system_overview::terminate_process,
            performance::get_performance_details,
            performance_devices::get_performance_devices,
            hardware_telemetry::get_hardware_telemetry,
            hardware_telemetry::get_telemetry_preferences,
            hardware_telemetry::set_telemetry_preferences,
            startup_inventory::list_startup_items,
            startup_inventory::set_startup_item_enabled,
            recent_files::list_recent_files,
            recent_files::refresh_recent_files,
            recent_files::open_recent_file,
            recent_files::open_recent_file_location,
            recent_files::get_image_preview,
            recent_files::get_custom_scan_folders,
            recent_files::add_custom_scan_folder,
            recent_files::remove_custom_scan_folder,
            tray::get_current_window_label,
            tray::is_current_window_foreground,
            tray::minimize_to_tray,
            tray::minimize_to_tray_and_remember,
            tray::get_minimize_on_close,
            tray::show_main_window,
            tray::show_main_module,
            tray::show_files_window,
            tray::open_windows_date_time_settings,
            tray::open_legacy_audio_panel,
            tray::frontend_ready,
            tray::toggle_tray_flyout,
            tray::resize_tray_flyout,
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
            ai_disk_advisor::detect_ai_cli_providers,
            ai_disk_advisor::get_ai_disk_analysis_preview,
            ai_disk_advisor::launch_ai_disk_terminal,
            ai_disk_advisor::get_ai_cleanup_plan_status,
            ai_disk_advisor::open_ai_cleanup_plan,
            ai_services_advisor::get_ai_services_analysis_preview,
            ai_services_advisor::launch_ai_services_terminal,
            ai_services_advisor::get_ai_services_review_status,
            ai_services_advisor::open_ai_services_review,
            disk_cleanup::analyze_disk_cleanup,
            disk_cleanup::clean_disk_categories,
            disk_cleanup::get_auto_cleanup,
            privileges::get_privilege_status,
            privileges::relaunch_as_admin,
            disk_cleanup::get_cleanup_insights,
            disk_cleanup::set_auto_cleanup,
            services::list_windows_services,
            services::set_windows_service_state,
            network_security::get_network_connections,
            network_security::get_security_posture,
            security_audit::run_remote_access_audit,
            app_updates::list_pending_updates,
            app_updates::upgrade_winget_package,
            app_updates::upgrade_all_winget_packages,
            app_updates::upgrade_winget_package_elevated,
            subscription_usage::get_subscription_usage,
            subscription_usage::get_subscription_settings,
            subscription_usage::set_openrouter_api_key,
            subscription_usage::clear_openrouter_api_key,
            subscription_usage::set_opencode_go_api_key,
            subscription_usage::clear_opencode_go_api_key,
            subscription_usage::set_provider_enabled,
            subscription_usage::set_provider_order,
            quota_history::get_quota_history,
            beta_features::get_beta_features,
            beta_features::set_beta_feature_enabled,
            beta_features::update_dock_settings,
            peripheral_battery::get_peripheral_batteries,
            peripheral_battery::refresh_peripheral_batteries,
            peripheral_battery::export_peripheral_diagnostics,
            peripheral_battery::set_peripheral_alias,
            peripheral_battery::set_peripheral_kind,
            launcher::launcher_query,
            launcher::launcher_execute,
            launcher::launcher_refresh_index,
            launcher::launcher_hide,
            launcher::launcher_toggle,
            launcher::win_key_admin::win_key_admin_status,
            launcher::win_key_admin::win_key_admin_enable,
            launcher::win_key_admin::win_key_admin_disable,
            custom_dock::get_dock_status,
            custom_dock::restore_windows_taskbar,
            custom_dock::list_dock_candidates,
            file_explorer::explorer_get_quick_access,
            file_explorer::explorer_list_directory,
            file_explorer::explorer_search_in_dir,
            file_explorer::explorer_preview_file,
            file_explorer::explorer_open_entry,
            file_explorer::explorer_open_in_terminal,
            file_explorer::explorer_create_folder,
            file_explorer::explorer_rename_entry,
            file_explorer::explorer_trash_entry,
            file_explorer::explorer_toggle_pin,
            // Comandos reexportados em `uninstall::mod` nao podem ser
            // registrados: o `#[tauri::command]` gera itens ocultos
            // (`__cmd__*`) no modulo de origem, e o handler so os encontra no
            // caminho completo.
            uninstall::installed::list_installed_apps,
            uninstall::get_uninstall_capabilities,
            uninstall::footprint::measure_installed_app_sizes,
            uninstall::footprint::measure_installed_app_size_full,
            uninstall::create_system_restore_point,
            uninstall::session::start_uninstall_session,
            uninstall::session::run_app_uninstaller,
            uninstall::session::cleanup_uninstall_residue,
            uninstall::session::cancel_uninstall_session
        ])
        .build(tauri::generate_context!())
        .expect("error while running Pulse")
        .run(|_app, event| {
            // Qualquer saída do app (Sair, reinício como admin, autoatualização)
            // devolve o sistema como era: hook da tecla Windows removido, barra
            // nativa e área útil restauradas.
            if let tauri::RunEvent::Exit = event {
                launcher::shutdown();
                let shell = custom_dock::status();
                if shell.running || shell.native_taskbar_hidden {
                    custom_dock::shutdown();
                }
            }
        });
}
