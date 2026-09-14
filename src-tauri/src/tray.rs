use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewWindow,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TrayPreferences {
    minimize_on_close: bool,
}

fn preferences_file() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
        .join("tray-preferences.json")
}

fn load_preferences() -> TrayPreferences {
    fs::read_to_string(preferences_file())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_preferences(preferences: &TrayPreferences) -> Result<(), String> {
    let file = preferences_file();
    let directory = file.parent().expect("preferences file has a parent");
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let content = serde_json::to_string_pretty(preferences).map_err(|error| error.to_string())?;
    fs::write(file, content).map_err(|error| error.to_string())
}

pub fn should_minimize_on_close() -> bool {
    load_preferences().minimize_on_close
}

pub fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = TrayIconBuilder::new()
        .tooltip("Pulse — Windows Observability")
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            match event {
                // Os dois botões abrem o mesmo painel consolidado, como no
                // fluxo rápido do EarTrumpet. O menu nativo separado não é
                // usado para não dividir controles entre duas superfícies.
                TrayIconEvent::Click {
                    button: MouseButton::Left | MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    let app = tray.app_handle();
                    let _ = toggle_flyout_internal(app);
                }
                // Clique duplo com botão esquerdo: abre a janela principal completa
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    let app = tray.app_handle();
                    let _ = show_main_internal(app);
                }
                _ => {}
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

fn show_main_internal(app: &AppHandle) -> Result<(), String> {
    if let Some(flyout) = app.get_webview_window("tray-flyout") {
        let _ = flyout.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.unminimize().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        let _ = window.emit("pulse://foreground-changed", true);
    }
    Ok(())
}

fn toggle_flyout_internal(app: &AppHandle) -> Result<(), String> {
    if let Some(flyout) = app.get_webview_window("tray-flyout") {
        let is_visible = flyout.is_visible().unwrap_or(false);
        if is_visible {
            let _ = flyout.emit("pulse://foreground-changed", false);
            let _ = flyout.hide();
        } else {
            // Posiciona a janela no canto inferior direito próximo à barra de tarefas
            if let Ok(Some(monitor)) = flyout.current_monitor() {
                let size = monitor.size();
                let scale = monitor.scale_factor();
                let flyout_w = 380.0;
                let flyout_h = 680.0;
                let margin_right = 16.0;
                let margin_bottom = 56.0; // Espaço para a barra de tarefas do Windows

                let screen_w = size.width as f64 / scale;
                let screen_h = size.height as f64 / scale;
                let pos_x = (screen_w - flyout_w - margin_right).max(0.0);
                let pos_y = (screen_h - flyout_h - margin_bottom).max(0.0);

                let _ = flyout.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                    pos_x, pos_y,
                )));
            }
            flyout.show().map_err(|e| e.to_string())?;
            flyout.set_focus().map_err(|e| e.to_string())?;
            let _ = flyout.emit("pulse://foreground-changed", true);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_current_window_label(window: WebviewWindow) -> String {
    window.label().to_string()
}

#[tauri::command]
pub fn is_current_window_foreground(window: WebviewWindow) -> bool {
    window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
}

#[tauri::command]
pub fn minimize_to_tray(window: WebviewWindow) -> Result<(), String> {
    let _ = window.emit("pulse://foreground-changed", false);
    window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn minimize_to_tray_and_remember(window: WebviewWindow) -> Result<(), String> {
    save_preferences(&TrayPreferences {
        minimize_on_close: true,
    })?;
    minimize_to_tray(window)
}

#[tauri::command]
pub fn get_minimize_on_close() -> bool {
    should_minimize_on_close()
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), String> {
    show_main_internal(&app)
}

#[tauri::command]
pub fn toggle_tray_flyout(app: AppHandle) -> Result<(), String> {
    toggle_flyout_internal(&app)
}

#[tauri::command]
pub fn hide_tray_flyout(app: AppHandle) -> Result<(), String> {
    if let Some(flyout) = app.get_webview_window("tray-flyout") {
        let _ = flyout.emit("pulse://foreground-changed", false);
        flyout.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn exit_app(app: AppHandle) {
    app.exit(0);
}
