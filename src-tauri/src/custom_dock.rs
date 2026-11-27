//! Pulse Shell nativo em Win32 + Direct2D (Beta — somente Windows):
//! Barra Superior estilo macOS + Dock inferior + Reserva de WorkArea (`SHAppBarMessage`) +
//! System Tray Proxy + Watchdog reversível da barra nativa do Windows.

use crate::beta_features::BetaFeaturesConfig;
use serde::Serialize;
use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use tauri::Manager;
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
pub mod appbar;
#[cfg(windows)]
pub mod dock_window;
#[cfg(windows)]
pub mod icons;
#[cfg(windows)]
pub mod power;
#[cfg(windows)]
pub mod shell_log;
#[cfg(windows)]
pub mod telemetry_feed;
#[cfg(windows)]
pub mod topbar;
#[cfg(windows)]
pub mod tray_proxy;
#[cfg(windows)]
pub mod watchdog;
#[cfg(windows)]
mod windows;

pub const DOCK_WATCHDOG_ARG: &str = "--dock-watchdog";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockStatus {
    pub supported: bool,
    pub running: bool,
    pub native_taskbar_hidden: bool,
    pub watchdog_active: bool,
    pub item_count: usize,
    pub engine: &'static str,
    pub unavailable_reason: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockAppCandidate {
    pub name: String,
    pub exe_path: String,
    pub is_pinned: bool,
    pub is_running: bool,
}

static APP_HANDLE: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();

fn app_handle_slot() -> &'static Mutex<Option<AppHandle>> {
    APP_HANDLE.get_or_init(|| Mutex::new(None))
}

fn clone_app_handle() -> Option<AppHandle> {
    app_handle_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned())
}

pub fn is_supported() -> bool {
    cfg!(windows)
}

/// Se o binário foi iniciado no modo sentinela (`--dock-watchdog <pid>`),
/// executa o watchdog e retorna `true` para encerrar antes de abrir a GUI.
pub fn run_watchdog_if_requested() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let Some(pos) = args.iter().position(|arg| arg == DOCK_WATCHDOG_ARG) else {
        return false;
    };
    let Some(pid) = args.get(pos + 1).and_then(|raw| raw.parse::<u32>().ok()) else {
        return true;
    };
    #[cfg(windows)]
    {
        watchdog::run_watchdog_loop(pid, &args[pos + 2..]);
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
    }
    true
}

pub fn init(app: AppHandle) {
    if let Ok(mut slot) = app_handle_slot().lock() {
        *slot = Some(app);
    }

    // Proteção contra loop de travamento: se a sessão anterior morreu antes de
    // concluir a inicialização do Pulse Shell, desativa automaticamente e devolve
    // a barra do Windows sem bloquear a thread principal do Tauri.
    #[cfg(windows)]
    if shell_log::consume_interrupted_startup().is_some() {
        if let Ok(updated) = crate::beta_features::disable_custom_dock_flag() {
            notify_config_changed(&updated);
        }
        let _ = std::thread::Builder::new()
            .name("pulse-shell-recover".to_owned())
            .spawn(windows::recover_orphaned_session_if_needed);
        return;
    }

    let config = crate::beta_features::load();
    if config.custom_dock && is_supported() {
        // Inicia em thread dedicada após a janela principal do Tauri subir o loop
        // de mensagens (evita deadlock com WM_SETTINGCHANGE / ABM_SETPOS no boot).
        let _ = std::thread::Builder::new()
            .name("pulse-shell-init".to_owned())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(350));
                if let Err(err) = apply_config(&config) {
                    crate::debug::record("custom_dock_init", 0, Err(&err));
                }
            });
    } else {
        #[cfg(windows)]
        {
            let _ = std::thread::Builder::new()
                .name("pulse-shell-recover".to_owned())
                .spawn(windows::recover_orphaned_session_if_needed);
        }
    }
}

pub fn notify_config_changed(config: &BetaFeaturesConfig) {
    if let Some(app) = clone_app_handle() {
        let _ = app.emit("pulse://beta-features-changed", config);
    }
}

pub fn open_main_window() {
    if let Some(app) = clone_app_handle() {
        tauri::async_runtime::spawn(async move {
            let _ = crate::tray::show_main_window(app);
        });
    }
}

/// Encerra o Pulse a partir da Barra Superior. Roda fora da thread da barra:
/// `shutdown` derruba as janelas do Shell e não pode esperar a própria thread.
pub fn exit_pulse_from_shell() {
    if let Some(app) = clone_app_handle() {
        tauri::async_runtime::spawn(async move {
            crate::tray::exit_app(app);
        });
    }
}

pub fn open_main_module(module: &str) {
    if let Some(app) = clone_app_handle() {
        let target = module.to_owned();
        tauri::async_runtime::spawn(async move {
            let _ = crate::tray::show_main_module(app, target);
        });
    }
}

pub fn open_beta_settings_in_main() {
    open_main_module("Recursos Beta");
}

/// Abre ou foca a janela standalone do Explorador de Arquivos do Pulse (`"files"`),
/// substituindo o Windows Explorer sem precisar abrir a janela principal inteira.
pub fn open_standalone_files_window() {
    if let Some(app) = clone_app_handle() {
        tauri::async_runtime::spawn(async move {
            let _ = crate::tray::show_files_window(app, None);
        });
    }
}

/// Revela a Dock quando o Pulse Runner é acionado pela tecla Windows.
/// No-op fora do Windows ou quando o Pulse Shell está desligado.
pub fn reveal_dock_from_shortcut() {
    #[cfg(windows)]
    {
        if is_supported() {
            dock_window::reveal_from_shortcut();
        }
    }
}

/// Mantém a Dock visível enquanto o Pulse Runner estiver aberto (inclusive em
/// tela cheia), liberando-a quando ele fecha.
pub fn set_dock_hold(hold: bool) {
    #[cfg(windows)]
    {
        if is_supported() {
            dock_window::hold_from_shortcut(hold);
        }
    }
}

/// Abre o Pulse Runner (atalho a partir da Barra Superior).
pub fn open_launcher_from_shell() {
    if let Some(app) = clone_app_handle() {
        tauri::async_runtime::spawn(async move {
            crate::launcher::toggle_launcher_window(&app);
        });
    }
}

#[cfg(windows)]
pub fn trigger_emergency_restore_from_shell() {
    std::thread::spawn(|| {
        windows::restore_taskbar_and_stop();
        if let Ok(updated) = crate::beta_features::disable_custom_dock_flag() {
            notify_config_changed(&updated);
        }
        crate::debug::record("custom_dock_restore_taskbar", 0, Ok(()));
    });
}

#[cfg(windows)]
pub fn toggle_quick_panel_at(
    anchor_center_x: i32,
    bar_bottom_y: i32,
    monitor: ::windows::Win32::Foundation::RECT,
    scale: f32,
) {
    toggle_shell_popup_at("full", anchor_center_x, bar_bottom_y, monitor, scale);
}

/// Abre (ou fecha se já estiver no mesmo modo) o popup modular da Barra Superior
/// reutilizando a única instância WebView2 (`tray-flyout`) para manter RAM mínima (< 80 MB).
#[cfg(windows)]
pub fn toggle_shell_popup_at(
    mode: &'static str,
    anchor_center_x: i32,
    bar_bottom_y: i32,
    monitor: ::windows::Win32::Foundation::RECT,
    scale: f32,
) {
    static LAST_POPUP_MODE: OnceLock<Mutex<String>> = OnceLock::new();
    let mode_slot = LAST_POPUP_MODE.get_or_init(|| Mutex::new(String::new()));

    let Some(app) = clone_app_handle() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let Some(flyout) = app.get_webview_window("tray-flyout") else {
            return;
        };
        let was_visible = flyout.is_visible().unwrap_or(false);
        let same_mode = mode_slot
            .lock()
            .map(|g| g.as_str() == mode)
            .unwrap_or(false);

        if was_visible && same_mode {
            let _ = flyout.emit("pulse://foreground-changed", false);
            let _ = flyout.hide();
            return;
        }
        if let Ok(mut g) = mode_slot.lock() {
            *g = mode.to_owned();
        }

        let (logical_w, logical_h) = match mode {
            "audio" => (360.0, 380.0),
            "quotas" => (360.0, 350.0),
            "clock" => (340.0, 420.0),
            "peripherals" => (360.0, 320.0),
            _ => (390.0, 620.0),
        };
        let _ = flyout.set_size(tauri::Size::Logical(tauri::LogicalSize::new(
            logical_w, logical_h,
        )));

        let s = if scale.is_finite() && scale > 0.0 {
            f64::from(scale)
        } else {
            1.0
        };
        let flyout_w = (logical_w * s).round() as i32;
        let flyout_h = (logical_h * s).round() as i32;
        let margin = (8.0 * s).round() as i32;
        let min_x = monitor.left + margin;
        let max_x = (monitor.right - flyout_w - margin).max(min_x);
        let pos_x = (anchor_center_x - flyout_w / 2).clamp(min_x, max_x);
        let max_y = (monitor.bottom - flyout_h - margin).max(bar_bottom_y + margin);
        let pos_y = (bar_bottom_y + margin).min(max_y);

        let _ = flyout.emit("pulse://flyout-mode", mode);
        let _ = flyout.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
            pos_x, pos_y,
        )));
        let _ = flyout.show();
        let _ = flyout.set_focus();
        let _ = flyout.emit("pulse://foreground-changed", true);
    });
}

pub fn apply_config(config: &BetaFeaturesConfig) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::apply_config(config)
    }
    #[cfg(not(windows))]
    {
        if config.custom_dock {
            Err(
                "A Dock nativa em Win32 + Direct2D está disponível apenas no Windows 10/11."
                    .to_owned(),
            )
        } else {
            Ok(())
        }
    }
}

pub fn shutdown() {
    #[cfg(windows)]
    {
        windows::shutdown();
    }
}

pub fn status() -> DockStatus {
    #[cfg(windows)]
    {
        windows::status()
    }
    #[cfg(not(windows))]
    {
        DockStatus {
            supported: false,
            running: false,
            native_taskbar_hidden: false,
            watchdog_active: false,
            item_count: 0,
            engine: "Win32 + Direct2D (Shell)",
            unavailable_reason: Some(
                "A Dock nativa em Win32 + Direct2D está disponível apenas no Windows 10/11."
                    .to_owned(),
            ),
            last_error: None,
        }
    }
}

#[tauri::command]
pub fn get_dock_status() -> DockStatus {
    status()
}

#[tauri::command]
pub async fn restore_windows_taskbar(app: AppHandle) -> Result<DockStatus, String> {
    #[cfg(windows)]
    {
        let _ = tauri::async_runtime::spawn_blocking(windows::restore_taskbar_and_stop).await;
    }
    let updated = crate::beta_features::disable_custom_dock_flag()?;
    let _ = app.emit("pulse://beta-features-changed", &updated);
    crate::debug::record("custom_dock_restore_taskbar", 0, Ok(()));
    Ok(status())
}

#[tauri::command]
pub async fn list_dock_candidates() -> Vec<DockAppCandidate> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(windows::list_candidates)
            .await
            .unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
