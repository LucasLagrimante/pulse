use serde::Serialize;
use std::{
    env, fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 500;
static LOG: OnceLock<Mutex<Vec<DebugEntry>>> = OnceLock::new();
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
static SESSION_STARTED_UNIX_MS: OnceLock<u64> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugEntry {
    pub sequence: u64,
    pub timestamp_unix_ms: u64,
    pub operation: String,
    pub category: String,
    pub source: String,
    pub duration_ms: u128,
    pub outcome: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugAuditInfo {
    pub enabled: bool,
    pub session_started_unix_ms: u64,
    pub entry_count: usize,
    pub retention_limit: usize,
    pub application_version: &'static str,
    pub platform: &'static str,
    pub persistence: &'static str,
    pub data_residency: &'static str,
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn data_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
}

fn mode_file() -> PathBuf {
    data_dir().join("debug-mode")
}

fn log_store() -> &'static Mutex<Vec<DebugEntry>> {
    SESSION_STARTED_UNIX_MS.get_or_init(now_unix_ms);
    LOG.get_or_init(|| Mutex::new(Vec::new()))
}

fn category_for(operation: &str) -> &'static str {
    if operation.contains("startup") || operation.contains("process") {
        "system_action"
    } else if operation.contains("file") || operation.contains("disk") {
        "storage"
    } else if operation.contains("interface") {
        "interface"
    } else if operation.contains("performance") || operation.contains("overview") {
        "telemetry"
    } else {
        "application"
    }
}

fn push_entry(operation: &str, duration_ms: u128, outcome: &str, message: &str, source: &str) {
    let entry = DebugEntry {
        sequence: SEQUENCE.fetch_add(1, Ordering::Relaxed),
        timestamp_unix_ms: now_unix_ms(),
        operation: operation.into(),
        category: category_for(operation).into(),
        source: source.into(),
        duration_ms,
        outcome: outcome.into(),
        severity: if outcome == "error" { "error" } else { "info" }.into(),
        message: message
            .lines()
            .next()
            .unwrap_or("Sem detalhes")
            .chars()
            .take(320)
            .collect(),
    };

    if let Ok(mut entries) = log_store().lock() {
        entries.push(entry);
        let overflow = entries.len().saturating_sub(MAX_ENTRIES);
        if overflow > 0 {
            entries.drain(0..overflow);
        }
    }
}

#[tauri::command]
pub fn get_debug_mode() -> bool {
    fs::read_to_string(mode_file()).is_ok_and(|s| s.trim() == "1")
}

#[tauri::command]
pub fn set_debug_mode(enabled: bool) -> Result<bool, String> {
    fs::create_dir_all(data_dir())
        .map_err(|e| format!("Não foi possível preparar o diagnóstico local: {e}"))?;
    fs::write(mode_file(), if enabled { "1" } else { "0" })
        .map_err(|e| format!("Não foi possível salvar o modo Debug: {e}"))?;
    push_entry(
        "debug_mode_changed",
        0,
        "ok",
        if enabled {
            "Auditoria ativada pelo usuário"
        } else {
            "Auditoria desativada pelo usuário"
        },
        "rust-backend",
    );
    Ok(enabled)
}

pub fn record(operation: &str, duration_ms: u128, result: Result<(), &str>) {
    if !get_debug_mode() {
        return;
    }
    let (outcome, message) = match result {
        Ok(()) => ("ok", "Concluído"),
        Err(message) => ("error", message),
    };
    push_entry(operation, duration_ms, outcome, message, "rust-backend");
}

#[tauri::command]
pub fn record_client_error(message: String, _component_stack: String) {
    let message: String = message
        .lines()
        .next()
        .unwrap_or("Falha de interface")
        .chars()
        .take(240)
        .collect();
    push_entry("interface_render", 0, "error", &message, "react-interface");
}

#[tauri::command]
pub fn get_debug_log() -> Vec<DebugEntry> {
    log_store()
        .lock()
        .map(|entries| entries.clone())
        .unwrap_or_default()
}

#[tauri::command]
pub fn get_debug_audit_info() -> DebugAuditInfo {
    DebugAuditInfo {
        enabled: get_debug_mode(),
        session_started_unix_ms: *SESSION_STARTED_UNIX_MS.get_or_init(now_unix_ms),
        entry_count: log_store()
            .lock()
            .map(|entries| entries.len())
            .unwrap_or_default(),
        retention_limit: MAX_ENTRIES,
        application_version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        persistence:
            "Somente memória desta execução; encerrando o Pulse, os eventos são descartados",
        data_residency: "Processado e mantido localmente; nenhum evento é enviado pela rede",
    }
}

#[tauri::command]
pub fn clear_debug_log() -> Result<(), String> {
    log_store()
        .lock()
        .map_err(|_| "Não foi possível obter acesso exclusivo ao histórico.".to_owned())?
        .clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{category_for, DebugEntry};

    #[test]
    fn debug_entry_is_structured() {
        let entry = DebugEntry {
            sequence: 1,
            timestamp_unix_ms: 1,
            operation: "overview".into(),
            category: "telemetry".into(),
            source: "rust-backend".into(),
            duration_ms: 2,
            outcome: "ok".into(),
            severity: "info".into(),
            message: "Concluído".into(),
        };
        assert_eq!(entry.operation, "overview");
        assert_eq!(entry.duration_ms, 2);
    }

    #[test]
    fn categorizes_audited_operations() {
        assert_eq!(category_for("system_overview"), "telemetry");
        assert_eq!(category_for("recent_files"), "storage");
        assert_eq!(category_for("terminate_process"), "system_action");
    }
}
