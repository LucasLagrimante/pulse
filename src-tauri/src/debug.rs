use serde::Serialize;
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 500;
/// Dias de arquivos `debug-AAAA-MM-DD.jsonl` mantidos em `%LOCALAPPDATA%\Pulse\logs`.
const LOG_RETENTION_DAYS: i64 = 7;
/// Teto por arquivo diário: um laço de erro não pode encher o disco.
const MAX_LOG_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_MESSAGE_CHARS: usize = 320;
const MAX_DETAIL_CHARS: usize = 600;
const MAX_CONTEXT_CHARS: usize = 200;
static LOG: OnceLock<Mutex<Vec<DebugEntry>>> = OnceLock::new();
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
static SESSION_STARTED_UNIX_MS: OnceLock<u64> = OnceLock::new();
/// Serializa as escritas no arquivo e garante a poda de retenção uma vez por execução.
static FILE_WRITER: OnceLock<Mutex<()>> = OnceLock::new();

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
    /// Etapa interna que falhou (ex.: `spawn`, `exit_status`, `parse`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Erro técnico original, sanitizado (sem quebras, caracteres de controle
    /// nem o caminho do perfil do usuário).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Trecho dos dados de entrada ao redor do ponto de falha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Falha estruturada: `message` é o resumo humano; `stage`, `detail` e
/// `context` permitem diagnosticar sem reproduzir o problema à mão.
#[derive(Debug, Clone, Default)]
pub struct Failure {
    pub message: String,
    pub stage: Option<String>,
    pub detail: Option<String>,
    pub context: Option<String>,
}

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Self::default()
        }
    }

    pub fn stage(mut self, stage: impl Into<String>) -> Self {
        self.stage = Some(stage.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
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
    pub persistence: String,
    pub data_residency: &'static str,
    pub log_directory: String,
}

/// Linha gravada no `.jsonl`: o evento mais a versão e a sessão que o
/// produziram, já que um mesmo arquivo diário cruza reinícios e atualizações.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedEntry<'a> {
    #[serde(flatten)]
    entry: &'a DebugEntry,
    app_version: &'static str,
    session_started_unix_ms: u64,
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

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Remove caracteres de controle, colapsa espaços, troca o perfil do usuário
/// por `%USERPROFILE%` e corta no limite de caracteres.
pub fn sanitize_text(raw: &str, max_chars: usize) -> String {
    let mut text: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(profile) = env::var_os("USERPROFILE").and_then(|p| p.into_string().ok()) {
        if profile.len() > 3 {
            text = text.replace(&profile, "%USERPROFILE%");
            text = text.replace(&profile.replace('\\', "\\\\"), "%USERPROFILE%");
        }
    }
    if text.chars().count() > max_chars {
        let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        text
    }
}

/// Janela de texto ao redor de um byte de `payload`, alinhada a limites de
/// caractere, para mostrar exatamente o dado que quebrou um parse.
pub fn context_window(payload: &str, byte_offset: usize, radius: usize) -> String {
    let clamp = |mut i: usize| {
        i = i.min(payload.len());
        while i > 0 && !payload.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let start = clamp(byte_offset.saturating_sub(radius));
    let end = clamp(byte_offset.saturating_add(radius));
    let escaped: String = payload[start..end]
        .chars()
        .map(|c| {
            if c.is_control() {
                format!("\\u{{{:04x}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        escaped,
        if end < payload.len() { "…" } else { "" }
    )
}

fn log_file_for(dir: &Path, date: chrono::NaiveDate) -> PathBuf {
    dir.join(format!("debug-{}.jsonl", date.format("%Y-%m-%d")))
}

/// Apaga `debug-AAAA-MM-DD.jsonl` mais antigos que a retenção. Outros arquivos
/// da pasta nunca são tocados.
fn prune_old_logs(dir: &Path, today: chrono::NaiveDate) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(date) = name
            .strip_prefix("debug-")
            .and_then(|rest| rest.strip_suffix(".jsonl"))
            .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if (today - date).num_days() >= LOG_RETENTION_DAYS {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn persist_entry(entry: &DebugEntry) {
    let lock = FILE_WRITER.get_or_init(|| {
        prune_old_logs(&log_dir(), chrono::Local::now().date_naive());
        Mutex::new(())
    });
    let Ok(_guard) = lock.lock() else { return };
    let dir = log_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = log_file_for(&dir, chrono::Local::now().date_naive());
    if fs::metadata(&path).is_ok_and(|meta| meta.len() >= MAX_LOG_FILE_BYTES) {
        return;
    }
    let line = PersistedEntry {
        entry,
        app_version: env!("CARGO_PKG_VERSION"),
        session_started_unix_ms: *SESSION_STARTED_UNIX_MS.get_or_init(now_unix_ms),
    };
    let Ok(json) = serde_json::to_string(&line) else {
        return;
    };
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{json}");
    }
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
    } else if operation.contains("subscription") || operation.contains("quota") {
        "telemetry"
    } else if operation.contains("interface") {
        "interface"
    } else if operation.contains("performance") || operation.contains("overview") {
        "telemetry"
    } else {
        "application"
    }
}

fn push_entry(operation: &str, duration_ms: u128, outcome: &str, message: &str, source: &str) {
    push_structured(
        operation,
        duration_ms,
        outcome,
        Failure::new(message),
        source,
    );
}

fn push_structured(
    operation: &str,
    duration_ms: u128,
    outcome: &str,
    failure: Failure,
    source: &str,
) {
    let message = failure.message;
    let entry = DebugEntry {
        sequence: SEQUENCE.fetch_add(1, Ordering::Relaxed),
        timestamp_unix_ms: now_unix_ms(),
        operation: operation.into(),
        category: category_for(operation).into(),
        source: source.into(),
        duration_ms,
        outcome: outcome.into(),
        severity: if outcome == "error" { "error" } else { "info" }.into(),
        message: sanitize_text(
            message.lines().next().unwrap_or("Sem detalhes"),
            MAX_MESSAGE_CHARS,
        ),
        stage: failure.stage.map(|s| sanitize_text(&s, 40)),
        detail: failure
            .detail
            .map(|d| sanitize_text(&d, MAX_DETAIL_CHARS))
            .filter(|d| !d.is_empty()),
        context: failure
            .context
            .map(|c| sanitize_text(&c, MAX_CONTEXT_CHARS))
            .filter(|c| !c.is_empty()),
    };

    if get_debug_mode() {
        persist_entry(&entry);
    }

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
    record_message(operation, duration_ms, outcome, message);
}

pub fn record_message(operation: &str, duration_ms: u128, outcome: &str, message: &str) {
    if !get_debug_mode() {
        return;
    }
    let outcome = if outcome == "error" { "error" } else { "ok" };
    push_entry(operation, duration_ms, outcome, message, "rust-backend");
}

/// Registra uma falha com etapa, erro técnico e contexto.
pub fn record_failure(operation: &str, duration_ms: u128, failure: Failure) {
    if !get_debug_mode() {
        return;
    }
    push_structured(operation, duration_ms, "error", failure, "rust-backend");
}

/// Registra o resultado de um comando preservando o erro real como `detail`.
pub fn record_result<T>(
    operation: &str,
    duration_ms: u128,
    result: &Result<T, String>,
    failure_message: &str,
) {
    match result {
        Ok(_) => record(operation, duration_ms, Ok(())),
        Err(error) => record_failure(
            operation,
            duration_ms,
            Failure::new(failure_message)
                .stage("command")
                .detail(error.as_str()),
        ),
    }
}

#[tauri::command]
pub fn record_client_error(message: String, component_stack: String) {
    let summary = message
        .lines()
        .next()
        .unwrap_or("Falha de interface")
        .to_owned();
    let mut failure = Failure::new(summary).stage("render").detail(message);
    // A primeira linha útil da pilha de componentes aponta onde o React quebrou.
    if let Some(frame) = component_stack
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
    {
        failure = failure.context(frame);
    }
    push_structured("interface_render", 0, "error", failure, "react-interface");
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
    let enabled = get_debug_mode();
    DebugAuditInfo {
        enabled,
        session_started_unix_ms: *SESSION_STARTED_UNIX_MS.get_or_init(now_unix_ms),
        entry_count: log_store()
            .lock()
            .map(|entries| entries.len())
            .unwrap_or_default(),
        retention_limit: MAX_ENTRIES,
        application_version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        persistence: if enabled {
            format!(
                "Memória desta execução + arquivo local diário, mantido por {LOG_RETENTION_DAYS} dias"
            )
        } else {
            "Modo Debug desligado: nada é registrado nem gravado em disco".to_owned()
        },
        data_residency: "Processado e mantido localmente; nenhum evento é enviado pela rede",
        log_directory: sanitize_text(&log_dir().to_string_lossy(), 260),
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
    use super::{
        category_for, context_window, log_file_for, prune_old_logs, sanitize_text, DebugEntry,
    };
    use chrono::NaiveDate;

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
            stage: None,
            detail: None,
            context: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(
            !json.contains("stage"),
            "campos vazios não poluem o log: {json}"
        );
        assert_eq!(entry.operation, "overview");
        assert_eq!(entry.duration_ms, 2);
    }

    #[test]
    fn categorizes_audited_operations() {
        assert_eq!(category_for("system_overview"), "telemetry");
        assert_eq!(category_for("recent_files"), "storage");
        assert_eq!(category_for("terminate_process"), "system_action");
        assert_eq!(category_for("subscription_usage_claude"), "telemetry");
    }

    #[test]
    fn sanitizes_control_characters_and_whitespace() {
        assert_eq!(
            sanitize_text("linha 1\nlinha\u{7}2\t fim", 100),
            "linha 1 linha 2 fim"
        );
        assert_eq!(sanitize_text("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn context_window_escapes_the_offending_character() {
        let payload = "{\"displayName\":\"Battle\u{7}net\"}";
        let offset = payload.find('\u{7}').unwrap();
        let context = context_window(payload, offset, 8);
        assert!(context.contains("Battle\\u{0007}net"), "{context}");
    }

    #[test]
    fn context_window_respects_multibyte_boundaries() {
        let payload = "ação çãé";
        for offset in 0..=payload.len() + 3 {
            let _ = context_window(payload, offset, 2);
        }
    }

    #[test]
    fn prunes_only_expired_debug_logs() {
        let dir = std::env::temp_dir().join(format!("pulse-debug-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let old = log_file_for(&dir, NaiveDate::from_ymd_opt(2026, 11, 6).unwrap());
        let recent = log_file_for(&dir, NaiveDate::from_ymd_opt(2026, 11, 7).unwrap());
        let other = dir.join("notas.jsonl");
        for path in [&old, &recent, &other] {
            std::fs::write(path, "{}\n").unwrap();
        }
        prune_old_logs(&dir, today);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
