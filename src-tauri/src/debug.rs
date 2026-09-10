use serde::Serialize;
use std::{
    env, fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 200;
static LOG: OnceLock<Mutex<Vec<DebugEntry>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugEntry {
    pub timestamp_unix_ms: u64,
    pub operation: String,
    pub duration_ms: u128,
    pub outcome: String,
    pub message: String,
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
    LOG.get_or_init(|| Mutex::new(Vec::new()))
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
    let timestamp_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let entry = DebugEntry {
        timestamp_unix_ms,
        operation: operation.into(),
        duration_ms,
        outcome: outcome.into(),
        message: message.into(),
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
pub fn get_debug_log() -> Vec<DebugEntry> {
    log_store()
        .lock()
        .map(|entries| entries.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::DebugEntry;
    #[test]
    fn debug_entry_is_structured() {
        let entry = DebugEntry {
            timestamp_unix_ms: 1,
            operation: "overview".into(),
            duration_ms: 2,
            outcome: "ok".into(),
            message: "Concluído".into(),
        };
        assert_eq!(entry.operation, "overview");
        assert_eq!(entry.duration_ms, 2);
    }
}
