//! Trilha de diagnóstico dedicada ao Pulse Shell (`shell-trace.log`) e
//! guarda anti-crash-loop (`shell-starting.lock`).
//!
//! Grava cada etapa crítica (Watchdog, TopBar, Dock, AppBar, TrayProxy e
//! falhas de Direct2D/UpdateLayeredWindow) com `flush()` imediato em
//! `%LOCALAPPDATA%\Pulse\logs\shell-trace.log` e espelha erros na auditoria
//! principal (`crate::debug`) para diagnóstico mesmo após travamento/kill.

use crate::debug::Failure;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_TRACE_BYTES: u64 = 2 * 1024 * 1024;
static TRACE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn trace_lock() -> &'static Mutex<()> {
    TRACE_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn trace_file() -> PathBuf {
    crate::debug::log_dir().join("shell-trace.log")
}

fn starting_lock_file() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join("shell-starting.lock")
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn write_line(level: &str, stage: &str, message: &str) {
    let Ok(_guard) = trace_lock().lock() else {
        return;
    };
    let path = trace_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::metadata(&path).is_ok_and(|meta| meta.len() >= MAX_TRACE_BYTES) {
        let prev = path.with_extension("log.prev");
        let _ = fs::remove_file(&prev);
        let _ = fs::rename(&path, &prev);
    }
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let pid = std::process::id();
    let thread = std::thread::current();
    let tname = thread.name().unwrap_or("unnamed");
    let clean = crate::debug::sanitize_text(message, 500);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            file,
            "[{ts}] [{level}] [pid={pid} {tname}] [{stage}] {clean}"
        );
        let _ = file.flush();
    }
}

/// Registra marco informativo no `shell-trace.log` e, se o modo Debug estiver
/// ativo, também no `debug-AAAA-MM-DD.jsonl`.
pub fn info(stage: &str, message: impl AsRef<str>) {
    let msg = message.as_ref();
    write_line("INFO", stage, msg);
    crate::debug::record_message("pulse_shell", 0, "ok", &format!("[{stage}] {msg}"));
}

/// Registra aviso (não fatal) no `shell-trace.log` e no log de auditoria.
pub fn warn(stage: &str, message: impl AsRef<str>) {
    let msg = message.as_ref();
    write_line("WARN", stage, msg);
    crate::debug::record_message("pulse_shell", 0, "ok", &format!("[{stage}] WARN: {msg}"));
}

/// Registra falha técnica com etapa e detalhe no `shell-trace.log` e na
/// auditoria principal (`record_failure`).
pub fn error(stage: &str, summary: impl AsRef<str>, detail: impl AsRef<str>) {
    let sum = summary.as_ref();
    let det = detail.as_ref();
    write_line("ERROR", stage, &format!("{sum} | detail={det}"));
    crate::debug::record_failure("pulse_shell", 0, Failure::new(sum).stage(stage).detail(det));
}

/// Marca início de inicialização do Pulse Shell antes de mexer em taskbar/janelas.
/// Se o processo travar ou morrer antes de `mark_shell_healthy()`, a próxima
/// abertura do Pulse detecta o arquivo e desativa o shell automaticamente.
pub fn mark_shell_starting() {
    let path = starting_lock_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = format!("pid={} started_ms={}", std::process::id(), now_unix_ms());
    let _ = fs::write(&path, payload);
    info("guard", "shell-starting.lock criado");
}

/// Remove a trava de inicialização depois que Barra Superior e Dock renderizam.
pub fn mark_shell_healthy() {
    let path = starting_lock_file();
    if path.exists() {
        let _ = fs::remove_file(&path);
        info(
            "guard",
            "shell-starting.lock removido (inicialização saudável)",
        );
    }
}

/// Verifica se uma inicialização anterior do Pulse Shell foi interrompida por
/// travamento ou encerramento abrupto e consome a trava.
pub fn consume_interrupted_startup() -> Option<String> {
    let path = starting_lock_file();
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).unwrap_or_else(|_| "sem metadados".to_owned());
    let _ = fs::remove_file(&path);
    error(
        "crash_loop_guard",
        "Inicialização anterior do Pulse Shell não concluiu; desativando automaticamente para proteger o sistema",
        content.trim(),
    );
    Some(content)
}
