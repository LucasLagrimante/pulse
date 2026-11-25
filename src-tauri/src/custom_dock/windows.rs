//! Orquestração Win32 do Pulse Shell (Barra Superior + Dock + AppBarSpace +
//! Tray Proxy + Telemetria + Watchdog da Barra Nativa).

use super::{
    dock_window, icons, power, shell_log, telemetry_feed, topbar, tray_proxy, watchdog,
    DockAppCandidate, DockStatus,
};
use crate::beta_features::BetaFeaturesConfig;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};

struct ShellController {
    config: BetaFeaturesConfig,
    last_error: Option<String>,
}

static CONTROLLER: OnceLock<Mutex<ShellController>> = OnceLock::new();
static LIFECYCLE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static NATIVE_HIDDEN: AtomicBool = AtomicBool::new(false);

fn controller() -> &'static Mutex<ShellController> {
    CONTROLLER.get_or_init(|| {
        Mutex::new(ShellController {
            config: BetaFeaturesConfig::default(),
            last_error: None,
        })
    })
}

fn lifecycle_lock() -> &'static Mutex<()> {
    LIFECYCLE_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn is_native_hidden() -> bool {
    NATIVE_HIDDEN.load(Ordering::Relaxed)
}

pub fn apply_config(config: &BetaFeaturesConfig) -> Result<(), String> {
    let _lifecycle = lifecycle_lock().lock().unwrap_or_else(|e| e.into_inner());

    if !config.custom_dock {
        shutdown_locked();
        return Ok(());
    }

    let was_running = dock_window::is_running() && topbar::is_running();
    if !was_running {
        shell_log::mark_shell_starting();
        shell_log::info("lifecycle", "Iniciando Pulse Shell completo");
    } else {
        shell_log::info("lifecycle", "Atualizando configuração do Pulse Shell ativo");
    }

    // 1. Watchdog externo sempre ativo antes de ocultar a taskbar nativa.
    let wd_ok = watchdog::ensure_watchdog_running();
    shell_log::info("watchdog", format!("watchdog ativo={wd_ok}"));

    // 2. Inicia Barra Superior e Dock ANTES de ocultar taskbar/AppBar/TrayProxy:
    // ambas renderizam o primeiro frame e entram no loop GetMessageW em <50ms,
    // evitando qualquer deadlock com broadcasts WM_SETTINGCHANGE do Explorer.
    if let Err(err) = topbar::start(config) {
        shell_log::error("topbar", "Falha ao iniciar Barra Superior", &err);
        record_error(&err);
        shutdown_locked();
        return Err(err);
    }

    if let Err(err) = dock_window::start(config) {
        shell_log::error("dock", "Falha ao iniciar Dock", &err);
        record_error(&err);
        shutdown_locked();
        return Err(err);
    }

    // 3. Oculta a barra do Windows com Tauri Main Thread + TopBar + Dock
    // todos livres bombeando mensagens (zero deadlock em ABM_SETSTATE).
    if !NATIVE_HIDDEN.load(Ordering::SeqCst) {
        NATIVE_HIDDEN.store(true, Ordering::SeqCst);
        watchdog::set_native_taskbar_hidden(true);
        // Re-alinha a AppBar inferior depois da barra nativa sair da borda inferior
        // (elimina o deslocamento residual de 48px na 1ª ativação).
        dock_window::recheck_appbar();
    }

    // 4. Inicia Tray Proxy (após taskbar ocultada e TopBar/Dock prontas) e Telemetria.
    tray_proxy::start();
    telemetry_feed::start(config);

    if let Ok(mut guard) = controller().lock() {
        guard.config = config.clone();
        guard.last_error = None;
    }

    shell_log::mark_shell_healthy();
    shell_log::info("lifecycle", "Pulse Shell iniciado com sucesso");
    Ok(())
}

pub fn shutdown() {
    let _lifecycle = lifecycle_lock().lock().unwrap_or_else(|e| e.into_inner());
    shutdown_locked();
}

fn shutdown_locked() {
    shell_log::info("lifecycle", "Encerrando Pulse Shell");
    NATIVE_HIDDEN.store(false, Ordering::SeqCst);
    telemetry_feed::stop();
    // Remove o proxy Shell_TrayWnd ANTES de restaurar taskbar/AppBar para que
    // o Explorer volte a receber tudo diretamente.
    tray_proxy::stop();
    dock_window::stop();
    topbar::stop();
    watchdog::restore_native_taskbar_and_workarea();
    watchdog::stop_watchdog();
    shell_log::mark_shell_healthy();

    if let Ok(mut guard) = controller().lock() {
        guard.config.custom_dock = false;
        guard.config.dock_hide_native_taskbar = false;
    }
    shell_log::info(
        "lifecycle",
        "Pulse Shell encerrado e barra nativa restaurada",
    );
}

pub fn restore_taskbar_and_stop() {
    shutdown();
}

pub fn recover_orphaned_session_if_needed() {
    watchdog::recover_orphaned_session_if_needed();
}

pub fn status() -> DockStatus {
    let last_error = controller().lock().ok().and_then(|g| g.last_error.clone());
    DockStatus {
        supported: true,
        running: dock_window::is_running(),
        native_taskbar_hidden: NATIVE_HIDDEN.load(Ordering::SeqCst),
        watchdog_active: watchdog::is_watchdog_active(),
        item_count: dock_window::item_count(),
        engine: "Win32 + Direct2D (Shell)",
        unavailable_reason: None,
        last_error,
    }
}

pub fn list_candidates() -> Vec<DockAppCandidate> {
    dock_window::list_candidates()
}

fn record_error(err: &str) {
    if let Ok(mut guard) = controller().lock() {
        guard.last_error = Some(err.to_owned());
    }
}

// Re-exporta submódulos apenas para manter uso claro de todas as peças.
#[allow(unused_imports)]
use {icons as _, power as _};
