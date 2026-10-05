//! Fast, local-first subscription quota collection.
//!
//! The UI never starts a terminal. A command returns a local snapshot immediately and a single
//! background worker refreshes official HTTP sources. Tokens stay in the OS credential store (or
//! the provider's own credential file) and are never serialized into the report.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::DateTime;
use regex::Regex;
use reqwest::{blocking::Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Reverse,
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CACHE_TTL: Duration = Duration::from_secs(60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(4);
const WSL_CLI_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DEPTH: usize = 3;
const MAX_FILES_TOTAL: usize = 2500;
const MAX_SCAN_DURATION: Duration = Duration::from_millis(1500);
const FILES_TO_INSPECT: usize = 24;
const OPENROUTER_SERVICE: &str = "Pulse";
const OPENROUTER_ACCOUNT: &str = "openrouter-api-key";
const OPENCODE_GO_SERVICE: &str = "Pulse";
const OPENCODE_GO_ACCOUNT: &str = "opencode-go-api-key";
const PROVIDER_IDS: [&str; 5] = [
    "claude",
    "codex",
    "antigravity",
    "openrouter",
    "opencode-go",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub window_minutes: Option<u64>,
    pub resets_at_unix_seconds: Option<i64>,
    pub reset_description: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub id: String,
    pub provider: String,
    pub subtitle: String,
    pub plan: Option<String>,
    pub environment: Option<String>,
    pub status: String,
    pub windows: Vec<QuotaWindow>,
    pub credits_remaining: Option<f64>,
    pub credits_total: Option<f64>,
    pub credits_used: Option<f64>,
    pub reset_credits_available: Option<u64>,
    pub sampled_at_unix_ms: Option<i64>,
    pub source: String,
    pub unavailable_reason: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUsageReport {
    pub providers: Vec<ProviderUsage>,
    pub sampled_at_unix_ms: i64,
    pub duration_ms: u64,
    pub privacy_note: String,
    pub scan_status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSettings {
    pub openrouter_configured: bool,
    pub openrouter_masked: Option<String>,
    pub opencode_go_configured: bool,
    pub opencode_go_masked: Option<String>,
    pub disabled_providers: Vec<String>,
    pub provider_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SubscriptionPreferences {
    #[serde(default)]
    disabled_providers: Vec<String>,
    #[serde(default)]
    provider_order: Vec<String>,
}

#[derive(Debug, Clone)]
struct CachedReport {
    stored_at: Instant,
    report: SubscriptionUsageReport,
}

#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    modified: SystemTime,
}

#[derive(Debug, Clone)]
struct OAuthCredential {
    access_token: String,
    refresh_token: Option<String>,
    plan: Option<String>,
    expires_at_unix_ms: Option<i64>,
}

static CACHE: OnceLock<Mutex<Option<CachedReport>>> = OnceLock::new();
static SCAN_RUNNING: AtomicBool = AtomicBool::new(false);

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn unavailable_provider(
    id: &str,
    provider: &str,
    subtitle: &str,
    source: &str,
    reason: impl Into<String>,
) -> ProviderUsage {
    ProviderUsage {
        id: id.into(),
        provider: provider.into(),
        subtitle: subtitle.into(),
        plan: None,
        environment: None,
        status: "unavailable".into(),
        windows: Vec::new(),
        credits_remaining: None,
        credits_total: None,
        credits_used: None,
        reset_credits_available: None,
        sampled_at_unix_ms: None,
        source: source.into(),
        unavailable_reason: Some(reason.into()),
        disabled: false,
    }
}

fn pick<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key))
}

fn number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .filter(|value| value.is_finite())
}

fn percent(value: Option<&Value>) -> Option<f64> {
    number(value).map(|value| value.clamp(0.0, 100.0))
}

fn parse_reset(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(raw) = number(Some(value)) {
        return Some(if raw.abs() > 10_000_000_000.0 {
            (raw / 1000.0) as i64
        } else {
            raw as i64
        });
    }
    value.as_str().and_then(|text| {
        DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|date| date.timestamp())
    })
}

fn parse_expiry_ms(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(raw) = number(Some(value)) {
        return Some(if raw.abs() > 10_000_000_000.0 {
            raw as i64
        } else {
            (raw * 1000.0) as i64
        });
    }
    value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text.trim()).ok())
        .map(|date| date.timestamp_millis())
}

fn credential_expired(expires_at_unix_ms: Option<i64>) -> bool {
    expires_at_unix_ms.is_some_and(|expires_at| expires_at <= now_unix_ms() + 30_000)
}

fn reset_from_window(value: &Value) -> Option<i64> {
    if let Some(absolute) = pick(value, &["resets_at", "resetsAt", "reset_at", "resetAt"])
        .and_then(|reset| parse_reset(Some(reset)))
    {
        return Some(absolute);
    }
    let relative = number(pick(
        value,
        &[
            "reset_after_seconds",
            "resetAfterSeconds",
            "resets_in_seconds",
            "resetsInSeconds",
        ],
    ))?;
    Some(now_unix_ms() / 1000 + relative.max(0.0) as i64)
}

fn window_minutes_from(value: &Value) -> Option<u64> {
    number(pick(
        value,
        &[
            "window_minutes",
            "windowMinutes",
            "window_duration_mins",
            "windowDurationMins",
        ],
    ))
    .map(|minutes| minutes.max(0.0).round() as u64)
    .filter(|minutes| *minutes > 0)
    .or_else(|| {
        number(pick(
            value,
            &[
                "limit_window_seconds",
                "limitWindowSeconds",
                "window_seconds",
                "windowSeconds",
            ],
        ))
        .map(|seconds| (seconds.max(0.0) / 60.0).round() as u64)
        .filter(|minutes| *minutes > 0)
    })
}

fn quota_from_used(id: &str, label: &str, value: Option<&Value>) -> Option<QuotaWindow> {
    let value = value?;
    let used_percent = percent(pick(value, &["used_percent", "usedPercent"]))?;
    let resets_at = reset_from_window(value);
    let expired = resets_at.is_some_and(|reset| reset * 1000 <= now_unix_ms());
    let (used, remaining, desc) = if expired {
        (Some(0.0), Some(100.0), Some("Ciclo renovado".into()))
    } else {
        (
            Some(used_percent),
            Some((100.0 - used_percent).clamp(0.0, 100.0)),
            None,
        )
    };
    Some(QuotaWindow {
        id: id.into(),
        label: label.into(),
        used_percent: used,
        remaining_percent: remaining,
        window_minutes: window_minutes_from(value),
        resets_at_unix_seconds: resets_at,
        reset_description: desc,
        unavailable_reason: None,
    })
}

fn quota_from_remaining(id: &str, label: &str, remaining: f64, reset: Option<i64>) -> QuotaWindow {
    let remaining = remaining.clamp(0.0, 100.0);
    QuotaWindow {
        id: id.into(),
        label: label.into(),
        used_percent: Some(100.0 - remaining),
        remaining_percent: Some(remaining),
        window_minutes: None,
        resets_at_unix_seconds: reset,
        reset_description: None,
        unavailable_reason: None,
    }
}

fn credits(value: Option<&Value>) -> Option<f64> {
    let value = value?.as_object()?;
    if value
        .get("has_credits")
        .or_else(|| value.get("hasCredits"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Some(0.0);
    }
    number(value.get("balance")).filter(|balance| *balance >= 0.0)
}

fn command_path(name: &str) -> Option<PathBuf> {
    let output = crate::process::hidden_command("where.exe")
        .arg(name)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn decode_command_output(bytes: &[u8]) -> String {
    let looks_like_utf16le = bytes.len() >= 2
        && bytes.len().is_multiple_of(2)
        && bytes.chunks_exact(2).filter(|pair| pair[1] == 0).count() >= bytes.len() / 4;
    if looks_like_utf16le {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        return String::from_utf16_lossy(&units.collect::<Vec<_>>());
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn wsl_output(args: &[&str]) -> Option<String> {
    let output = crate::process::hidden_command("wsl.exe")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(decode_command_output(&output.stdout))
}

fn wsl_distros() -> Vec<String> {
    let mut distros = wsl_output(&["-l", "-q"])
        .unwrap_or_default()
        .lines()
        .map(|line| {
            line.trim_matches(|character| character == '\u{feff}' || character == '\0')
                .trim()
                .to_owned()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    distros.sort_unstable();
    distros.dedup();
    distros
}

fn wsl_home(distro: &str) -> Option<String> {
    let output = crate::process::hidden_command("wsl.exe")
        .args(["-d", distro, "--", "sh", "-lc", "printf %s \"$HOME\""])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let home = decode_command_output(&output.stdout).trim().to_owned();
    home.starts_with('/').then_some(home)
}

fn wsl_cli_output(command: &str) -> Option<(String, String)> {
    for distro in wsl_distros() {
        let mut child = crate::process::hidden_command("wsl.exe")
            .args(["-d", &distro, "--", "sh", "-lc", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        let Some(mut child) = child.take() else {
            continue;
        };
        let deadline = Instant::now() + WSL_CLI_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        break;
                    }
                    let output = child.wait_with_output().ok()?;
                    return Some((distro, decode_command_output(&output.stdout)));
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Err(_) => break,
            }
        }
    }
    None
}

fn wsl_file_candidates(relative_path: &str) -> Vec<PathBuf> {
    let relative_path = relative_path.trim_start_matches(['/', '\\']);
    let mut candidates = Vec::new();
    for distro in wsl_distros() {
        let Some(home) = wsl_home(&distro) else {
            continue;
        };
        let home = home.replace('/', "\\");
        let home = home.trim_start_matches(['/', '\\']);
        let relative_path = relative_path.replace('/', "\\");
        for prefix in [r"\\wsl.localhost", r"\\wsl$"] {
            let path = PathBuf::from(format!(r"{}\{}\{}\{}", prefix, distro, home, relative_path));
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }
    candidates
}

fn wsl_has_file(relative_path: &str) -> bool {
    wsl_file_candidates(relative_path)
        .iter()
        .any(|path| path.is_file())
}

fn codex_executable() -> Option<PathBuf> {
    let packaged = env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|path| {
        path.join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("codex.exe")
    });
    if packaged.as_ref().is_some_and(|path| path.is_file()) {
        return packaged;
    }
    command_path("codex")
}

fn read_json_response(
    receiver: &mpsc::Receiver<Value>,
    request_id: i64,
    timeout: Duration,
) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(message) if message.get("id").and_then(Value::as_i64) == Some(request_id) => {
                if message.get("error").is_some() {
                    return Err("o app-server recusou a consulta".into());
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "resposta sem resultado".into());
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("o app-server encerrou antes de responder".into())
            }
        }
    }
    Err("tempo limite ao consultar o app-server".into())
}

fn query_codex_app_server() -> Result<Value, String> {
    let executable = codex_executable().ok_or_else(|| "CLI não encontrada".to_string())?;
    let mut child = crate::process::hidden_command(executable.to_string_lossy().as_ref())
        .args(["-c", "service_tier=\"flex\"", "app-server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "não foi possível iniciar o app-server".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "saída indisponível".to_string())?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(message) = serde_json::from_str::<Value>(&line) {
                let _ = sender.send(message);
            }
        }
    });
    let result = (|| {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "entrada indisponível".to_string())?;
        writeln!(stdin, "{}", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"pulse","version":env!("CARGO_PKG_VERSION")}}})).map_err(|_| "falha ao inicializar".to_string())?;
        stdin.flush().ok();
        read_json_response(&receiver, 1, Duration::from_secs(2))?;
        writeln!(
            stdin,
            "{}",
            json!({"id":2,"method":"account/rateLimits/read","params":null})
        )
        .map_err(|_| "falha ao solicitar".to_string())?;
        stdin.flush().ok();
        read_json_response(&receiver, 2, Duration::from_secs(3))
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn codex_from_rate_limits(data: &Value, source: &str) -> Option<ProviderUsage> {
    let rate_limits = pick(
        data,
        &["rate_limits", "rateLimits", "rate_limit", "rateLimit"],
    )
    .unwrap_or(data);
    let primary = pick(rate_limits, &["primary", "primary_window", "primaryWindow"]);
    let secondary = pick(
        rate_limits,
        &["secondary", "secondary_window", "secondaryWindow"],
    );
    let mut windows = Vec::new();
    if let Some(window) = quota_from_used("session", "Sessão", primary) {
        windows.push(window);
    }
    if let Some(window) = quota_from_used("weekly", "Semanal", secondary) {
        windows.push(window);
    }
    if let Some(groups) = pick(
        data,
        &[
            "rate_limits_by_limit_id",
            "rateLimitsByLimitId",
            "additional_rate_limits",
            "additionalRateLimits",
        ],
    )
    .and_then(Value::as_object)
    {
        for (limit_id, value) in groups {
            if limit_id.eq_ignore_ascii_case("codex") {
                continue;
            }
            let name = pick(value, &["limit_name", "limitName"])
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(limit_id);
            if let Some(window) = quota_from_used(
                &format!("{limit_id}-session"),
                &format!("{name} · sessão"),
                pick(value, &["primary", "primary_window", "primaryWindow"]),
            ) {
                windows.push(window);
            }
            if let Some(window) = quota_from_used(
                &format!("{limit_id}-weekly"),
                &format!("{name} · semanal"),
                pick(value, &["secondary", "secondary_window", "secondaryWindow"]),
            ) {
                windows.push(window);
            }
        }
    }
    if windows.is_empty() {
        return None;
    }
    Some(ProviderUsage {
        id: "codex".into(),
        provider: "Codex".into(),
        subtitle: "ChatGPT".into(),
        plan: pick(data, &["plan_type", "planType"])
            .or_else(|| pick(rate_limits, &["plan_type", "planType"]))
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned),
        environment: Some("Windows · assinatura única".into()),
        status: "available".into(),
        windows,
        credits_remaining: pick(rate_limits, &["credits"])
            .or_else(|| pick(data, &["credits"]))
            .and_then(|value| credits(Some(value))),
        credits_total: None,
        credits_used: None,
        reset_credits_available: pick(data, &["rate_limit_reset_credits", "rateLimitResetCredits"])
            .or_else(|| {
                pick(
                    rate_limits,
                    &["rate_limit_reset_credits", "rateLimitResetCredits"],
                )
            })
            .and_then(|value| pick(value, &["available_count", "availableCount"]))
            .and_then(Value::as_u64),
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
        disabled: false,
    })
}

fn codex_home() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
}

fn collect_jsonl_files(
    root: &Path,
    depth: usize,
    started: Instant,
    checked: &mut usize,
    output: &mut Vec<Candidate>,
) {
    if depth > MAX_DEPTH || *checked >= MAX_FILES_TOTAL || started.elapsed() >= MAX_SCAN_DURATION {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        Reverse(
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });
    for entry in entries {
        if *checked >= MAX_FILES_TOTAL || started.elapsed() >= MAX_SCAN_DURATION {
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_jsonl_files(&entry.path(), depth + 1, started, checked, output);
            continue;
        }
        *checked += 1;
        if (*checked).is_multiple_of(15) {
            thread::yield_now();
        }
        if entry.path().extension().and_then(|v| v.to_str()) != Some("jsonl") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        output.push(Candidate {
            path: entry.path(),
            modified,
        });
    }
}

fn codex_from_local_logs() -> Option<ProviderUsage> {
    let home = codex_home()?;
    let started = Instant::now();
    let mut checked = 0;
    let mut candidates = Vec::new();
    for root in [home.join("sessions"), home.join("archived_sessions")] {
        collect_jsonl_files(&root, 0, started, &mut checked, &mut candidates);
    }
    candidates.sort_by_key(|candidate| Reverse(candidate.modified));
    for candidate in candidates.iter().take(FILES_TO_INSPECT) {
        let reader = BufReader::new(File::open(&candidate.path).ok()?);
        let mut latest = None;
        for line in reader.lines().map_while(Result::ok) {
            if !line.contains("\"rate_limits\"") {
                continue;
            }
            let Ok(document) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(rate_limits) = document.pointer("/payload/rate_limits") {
                latest = codex_from_rate_limits(rate_limits, "Último registro local do Codex");
            }
        }
        if let Some(mut usage) = latest {
            usage.sampled_at_unix_ms = candidate
                .modified
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis().min(i64::MAX as u128) as i64);
            return Some(usage);
        }
    }
    None
}

fn codex_auth_credentials() -> Option<(String, Option<String>)> {
    let home = codex_home()?;
    let path = home.join("auth.json");
    let bytes = fs::read(path).ok()?;
    let value = serde_json::from_slice::<Value>(&bytes).ok()?;
    let tokens = value.get("tokens").unwrap_or(&value);
    let token = tokens
        .get("access_token")
        .or_else(|| tokens.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    let account_id = tokens
        .get("account_id")
        .or_else(|| tokens.get("accountId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    Some((token.to_owned(), account_id))
}

fn query_codex_official_api() -> Result<Value, String> {
    let (token, account_id) =
        codex_auth_credentials().ok_or_else(|| "auth.json não encontrada".to_string())?;
    let client = http_client().map_err(|e| format!("HTTP indisponível: {e}"))?;
    let mut request = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(&token)
        .header("User-Agent", "codex")
        .header("Accept", "application/json");
    if let Some(ref acc_id) = account_id {
        request = request.header("ChatGPT-Account-Id", acc_id);
    }
    let response = request
        .send()
        .map_err(|e| format!("Erro de requisição: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response
        .json::<Value>()
        .map_err(|e| format!("JSON inválido: {e}"))
}

fn query_codex() -> ProviderUsage {
    // 1. Tentar primeiro o endpoint oficial ChatGPT wham/usage se houver auth.json válido (dados mais recentes)
    if let Ok(data) = query_codex_official_api() {
        if let Some(usage) =
            codex_from_rate_limits(&data, "ChatGPT oficial · /backend-api/wham/usage")
        {
            return usage;
        }
    }
    // 2. Tentar logs locais de sessões se houverem leituras ativas
    if let Some(local) = codex_from_local_logs() {
        return local;
    }
    // 3. Tentar codex app-server local
    match query_codex_app_server() {
        Ok(data) => codex_from_rate_limits(&data, "Codex app-server · account/rateLimits/read")
            .unwrap_or_else(|| {
                unavailable_provider(
                    "codex",
                    "Codex",
                    "ChatGPT",
                    "Codex app-server",
                    "A conta respondeu, mas não expôs cotas de assinatura.",
                )
            }),
        Err(error) => unavailable_provider(
            "codex",
            "Codex",
            "ChatGPT",
            "Codex app-server",
            format!("Leitura não confirmada: {error}."),
        ),
    }
}

fn parse_claude_credential(value: &Value) -> Option<OAuthCredential> {
    let oauth = value
        .get("claudeAiOauth")
        .or_else(|| value.get("claude_ai_oauth"))
        .unwrap_or(value);
    let access_token = oauth
        .get("accessToken")
        .or_else(|| oauth.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    let plan = oauth
        .get("subscriptionType")
        .or_else(|| oauth.get("subscription_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(OAuthCredential {
        access_token,
        refresh_token: oauth
            .get("refreshToken")
            .or_else(|| oauth.get("refresh_token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        plan,
        expires_at_unix_ms: parse_expiry_ms(
            oauth.get("expiresAt").or_else(|| oauth.get("expires_at")),
        ),
    })
}

fn load_json_file(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

fn choose_oauth_credential(mut credentials: Vec<OAuthCredential>) -> Option<OAuthCredential> {
    if let Some(credential) = credentials
        .iter()
        .find(|credential| !credential_expired(credential.expires_at_unix_ms))
    {
        return Some(credential.clone());
    }
    credentials.sort_by_key(|credential| credential.expires_at_unix_ms.unwrap_or(0));
    credentials.pop()
}

fn claude_credentials() -> Option<OAuthCredential> {
    let mut paths = Vec::new();
    if let Some(home) = env::var_os("USERPROFILE") {
        paths.push(
            PathBuf::from(home)
                .join(".claude")
                .join(".credentials.json"),
        );
    }
    if let Some(home) = env::var_os("HOME") {
        paths.push(
            PathBuf::from(home)
                .join(".claude")
                .join(".credentials.json"),
        );
    }
    paths.extend(wsl_file_candidates(".claude/.credentials.json"));
    if let Some(token) = env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        return Some(OAuthCredential {
            access_token: token,
            refresh_token: None,
            plan: None,
            expires_at_unix_ms: None,
        });
    }
    let credentials = paths
        .iter()
        .filter_map(|path| load_json_file(path).and_then(|value| parse_claude_credential(&value)))
        .collect::<Vec<_>>();
    choose_oauth_credential(credentials)
}

fn claude_unavailable_reason(credential: &OAuthCredential, status: StatusCode) -> String {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        || credential_expired(credential.expires_at_unix_ms)
    {
        if credential.refresh_token.is_some() {
            return "O access token local expirou; abra o Claude Code para renovar a sessão e atualize as cotas.".into();
        }
        return "A credencial local expirou; autentique novamente no Claude Code.".into();
    }
    format!(
        "A fonte oficial recusou a consulta (HTTP {}).",
        status.as_u16()
    )
}

fn cli_percent(line: &str) -> Option<f64> {
    let captures = Regex::new(r"(?i)(\d+(?:[.,]\d+)?)\s*%")
        .ok()?
        .captures(line)?;
    captures
        .get(1)
        .and_then(|value| value.as_str().replace(',', ".").parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
}

fn cli_reset_description(line: &str) -> Option<String> {
    line.split_once('·')
        .map(|(_, reset)| reset.trim().to_owned())
        .filter(|reset| !reset.is_empty())
}

fn parse_claude_cli_usage(raw: &str) -> Vec<QuotaWindow> {
    let mut windows = Vec::new();
    for line in strip_terminal(raw).lines() {
        let normalized = line.to_ascii_lowercase();
        let (id, label, minutes) = if normalized.contains("current session:") {
            ("session", "Sessão", 300)
        } else if normalized.contains("current week") {
            ("weekly", "Semanal", 10080)
        } else {
            continue;
        };
        let Some(used) = cli_percent(line) else {
            continue;
        };
        if windows.iter().any(|window: &QuotaWindow| window.id == id) {
            continue;
        }
        windows.push(QuotaWindow {
            id: id.into(),
            label: label.into(),
            used_percent: Some(used),
            remaining_percent: Some((100.0 - used).clamp(0.0, 100.0)),
            window_minutes: Some(minutes),
            resets_at_unix_seconds: None,
            reset_description: cli_reset_description(line),
            unavailable_reason: None,
        });
    }
    windows
}

fn parse_antigravity_cli_usage(raw: &str) -> Vec<QuotaWindow> {
    let mut windows = Vec::new();
    for line in strip_terminal(raw).lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(remaining_index) = fields
            .iter()
            .position(|field| field.eq_ignore_ascii_case("remaining"))
        else {
            continue;
        };
        let (model_end, label, minutes) =
            if remaining_index >= 2 && fields[remaining_index - 2].eq_ignore_ascii_case("weekly") {
                (remaining_index - 2, "Semanal", 10080)
            } else if remaining_index >= 3
                && fields[remaining_index - 3].eq_ignore_ascii_case("five")
                && fields[remaining_index - 2].eq_ignore_ascii_case("hour")
            {
                (remaining_index - 3, "5 horas", 300)
            } else {
                continue;
            };
        let model = fields[..model_end].join(" ");
        let Some(remaining) = fields
            .get(remaining_index + 1)
            .and_then(|value| value.strip_suffix('%'))
            .and_then(|value| value.replace(',', ".").parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .map(|value| value.clamp(0.0, 100.0))
        else {
            continue;
        };
        let reset = fields
            .get(remaining_index + 2)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|date| date.timestamp());
        let expired = reset.is_some_and(|reset| reset * 1000 <= now_unix_ms());
        let (used, rem, desc) = if expired {
            (Some(0.0), Some(100.0), Some("Ciclo renovado".into()))
        } else {
            (Some(100.0 - remaining), Some(remaining), None)
        };
        windows.push(QuotaWindow {
            id: format!("agy-{}", windows.len()),
            label: format!("{model} · {label}"),
            used_percent: used,
            remaining_percent: rem,
            window_minutes: Some(minutes),
            resets_at_unix_seconds: reset,
            reset_description: desc,
            unavailable_reason: None,
        });
    }
    windows
}

fn query_claude_from_wsl_cli() -> Option<ProviderUsage> {
    if !wsl_has_file(".claude/.credentials.json") {
        return None;
    }
    let (distro, output) = wsl_cli_output("claude -p '/usage' --output-format text")?;
    let windows = parse_claude_cli_usage(&output);
    if windows.is_empty() {
        return None;
    }
    Some(ProviderUsage {
        id: "claude".into(),
        provider: "Claude Code".into(),
        subtitle: "Assinatura Anthropic".into(),
        plan: claude_credentials().and_then(|credential| credential.plan),
        environment: Some(format!("WSL {distro} · Claude Code local")),
        status: "available".into(),
        windows,
        credits_remaining: None,
        credits_total: None,
        credits_used: None,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: "Claude Code CLI · /usage".into(),
        unavailable_reason: None,
        disabled: false,
    })
}

fn query_antigravity_from_wsl_cli() -> Option<ProviderUsage> {
    if !wsl_has_file(".gemini/antigravity-cli/antigravity-oauth-token") {
        return None;
    }
    let (distro, output) = wsl_cli_output("agy -p '/usage' --output-format text")?;
    let windows = parse_antigravity_cli_usage(&output);
    if windows.is_empty() {
        return None;
    }
    Some(ProviderUsage {
        id: "antigravity".into(),
        provider: "Antigravity".into(),
        subtitle: "Google AI Pro".into(),
        plan: None,
        environment: Some(format!("WSL {distro} · agy local")),
        status: "available".into(),
        windows,
        credits_remaining: None,
        credits_total: None,
        credits_used: None,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: "Antigravity CLI · /usage".into(),
        unavailable_reason: None,
        disabled: false,
    })
}

fn http_client() -> Result<Client, String> {
    // Single-client: reqwest::Client já é um pool Arc internamente; construir
    // um por chamada recriava pools de conexão e TLS a cada scan. O clone aqui
    // é barato (Arc) e preserva os mesmos timeouts em todas as chamadas.
    static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| "cliente HTTP indisponível".to_owned())?;
    let _ = HTTP_CLIENT.set(client.clone());
    Ok(client)
}

fn query_claude() -> ProviderUsage {
    if let Some(usage) = query_claude_from_wsl_cli() {
        return usage;
    }
    let source = "Anthropic OAuth · /api/oauth/usage";
    let Some(credentials) = claude_credentials() else {
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            "Credencial local não encontrada no Windows nem nas distros WSL.",
        );
    };
    let Ok(client) = http_client() else {
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            "Cliente HTTP indisponível.",
        );
    };
    let response = client
        .get("https://console.anthropic.com/api/oauth/usage")
        .bearer_auth(&credentials.access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Accept", "application/json")
        .send();
    let Ok(response) = response else {
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            "A fonte oficial não respondeu dentro do limite de 4s.",
        );
    };
    if !response.status().is_success() {
        let status = response.status();
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            claude_unavailable_reason(&credentials, status),
        );
    }
    let Ok(body) = response.json::<Value>() else {
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            "Resposta oficial inválida.",
        );
    };
    let mut windows = Vec::new();
    for (id, label, key, minutes) in [
        ("session", "Sessão", "five_hour", 300),
        ("weekly", "Semanal", "seven_day", 10080),
        ("sonnet", "Sonnet", "seven_day_sonnet", 10080),
    ] {
        if let Some(value) = body.get(key).and_then(Value::as_object) {
            if let Some(used) = number(value.get("utilization")) {
                let resets_at = parse_reset(value.get("resets_at"));
                let expired = resets_at.is_some_and(|reset| reset * 1000 <= now_unix_ms());
                let (used, rem, desc) = if expired {
                    (Some(0.0), Some(100.0), Some("Ciclo renovado".into()))
                } else {
                    (
                        Some(used.clamp(0.0, 100.0)),
                        Some((100.0 - used).clamp(0.0, 100.0)),
                        None,
                    )
                };
                windows.push(QuotaWindow {
                    id: id.into(),
                    label: label.into(),
                    used_percent: used,
                    remaining_percent: rem,
                    window_minutes: Some(minutes),
                    resets_at_unix_seconds: resets_at,
                    reset_description: desc,
                    unavailable_reason: None,
                });
            }
        }
    }
    if windows.is_empty() {
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            "A resposta não expôs janelas de assinatura.",
        );
    }
    let extra_usage = body.get("extra_usage").and_then(Value::as_object);
    let credits_used = extra_usage.and_then(|usage| {
        number(
            usage
                .get("used_credits")
                .or_else(|| usage.get("usedCredits")),
        )
    });
    let credits_total = extra_usage.and_then(|usage| {
        number(
            usage
                .get("total_credits")
                .or_else(|| usage.get("totalCredits")),
        )
    });
    let credits_remaining = match (credits_total, credits_used) {
        (Some(total), Some(used)) if total >= used => Some(total - used),
        (Some(total), None) => Some(total),
        _ => None,
    };
    ProviderUsage {
        id: "claude".into(),
        provider: "Claude Code".into(),
        subtitle: "Assinatura Anthropic".into(),
        plan: credentials.plan,
        environment: Some("Windows/WSL · OAuth local".into()),
        status: "available".into(),
        windows,
        credits_remaining,
        credits_total,
        credits_used,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
        disabled: false,
    }
}

fn nested_string(value: &Value, keys: &[&str]) -> Option<String> {
    if let Some(object) = value.as_object() {
        for key in keys {
            if let Some(text) = object
                .get(*key)
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty())
            {
                return Some(text.trim().to_owned());
            }
        }
        for child in object.values() {
            if let Some(found) = nested_string(child, keys) {
                return Some(found);
            }
        }
    } else if let Some(items) = value.as_array() {
        for child in items {
            if let Some(found) = nested_string(child, keys) {
                return Some(found);
            }
        }
    }
    None
}

fn nested_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    if let Some(object) = value.as_object() {
        for key in keys {
            if let Some(found) = object.get(*key) {
                return Some(found);
            }
        }
        for child in object.values() {
            if let Some(found) = nested_value(child, keys) {
                return Some(found);
            }
        }
    } else if let Some(items) = value.as_array() {
        for child in items {
            if let Some(found) = nested_value(child, keys) {
                return Some(found);
            }
        }
    }
    None
}

fn parse_antigravity_credential(value: &Value) -> Option<OAuthCredential> {
    let access_token = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            nested_string(
                value,
                &["access_token", "accessToken", "token", "bearerToken"],
            )
        })
        .or_else(|| nested_string(value, &["id_token", "idToken"]))?;
    Some(OAuthCredential {
        access_token,
        refresh_token: nested_string(value, &["refresh_token", "refreshToken"]),
        plan: None,
        expires_at_unix_ms: nested_value(
            value,
            &["expiry", "expiry_date", "expiresAt", "expires_at"],
        )
        .and_then(|value| parse_expiry_ms(Some(value))),
    })
}

fn antigravity_credentials() -> Option<OAuthCredential> {
    let mut values = Vec::new();
    if let Some(value) = keyring_secret("gemini", "antigravity") {
        values.push(value);
    }
    let mut paths = Vec::new();
    if let Some(home) = env::var_os("USERPROFILE") {
        paths.push(
            PathBuf::from(home)
                .join(".gemini")
                .join("antigravity-cli")
                .join("antigravity-oauth-token"),
        );
    }
    if let Some(home) = env::var_os("HOME") {
        paths.push(
            PathBuf::from(home)
                .join(".gemini")
                .join("antigravity-cli")
                .join("antigravity-oauth-token"),
        );
    }
    paths.extend(wsl_file_candidates(
        ".gemini/antigravity-cli/antigravity-oauth-token",
    ));
    values.extend(paths.iter().filter_map(|path| load_json_file(path)));
    choose_oauth_credential(
        values
            .iter()
            .filter_map(parse_antigravity_credential)
            .collect(),
    )
}

fn collect_antigravity_windows(value: &Value, output: &mut Vec<QuotaWindow>, path: &str) {
    if let Some(object) = value.as_object() {
        if let Some(fraction) = number(object.get("remainingFraction")) {
            let label = object
                .get("modelId")
                .or_else(|| object.get("label"))
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty())
                .unwrap_or(path);
            output.push(quota_from_remaining(
                &format!("agy-{}", output.len()),
                label,
                fraction * 100.0,
                parse_reset(object.get("resetTime")),
            ));
        }
        for (key, child) in object {
            collect_antigravity_windows(child, output, key);
        }
    } else if let Some(items) = value.as_array() {
        for child in items {
            collect_antigravity_windows(child, output, path);
        }
    }
}

#[allow(clippy::possible_missing_else)]
fn query_antigravity() -> ProviderUsage {
    if let Some(usage) = query_antigravity_from_wsl_cli() {
        return usage;
    }
    let source = "Google Cloud Code · retrieveUserQuotaSummary";
    let Some(credentials) = antigravity_credentials() else {
        return unavailable_provider(
            "antigravity",
            "Antigravity",
            "Google AI Pro",
            source,
            "Credencial local não encontrada no Credential Manager nem nas distros WSL.",
        );
    };
    let Ok(client) = http_client() else {
        return unavailable_provider(
            "antigravity",
            "Antigravity",
            "Google AI Pro",
            source,
            "Cliente HTTP indisponível.",
        );
    };
    let mut token = credentials.access_token.clone();
    let mut refreshed = false;
    let mut body = None;
    'bases: for base in [
        "https://daily-cloudcode-pa.googleapis.com",
        "https://cloudcode-pa.googleapis.com",
    ] {
        let response = client
            .post(format!("{base}/v1internal:retrieveUserQuotaSummary"))
            .bearer_auth(&token)
            .header("User-Agent", "antigravity")
            .json(&json!({}))
            .send();
        let Ok(response) = response else {
            continue;
        };
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            if !refreshed {
                if let Some(refresh) = credentials
                    .refresh_token
                    .as_deref()
                    .and_then(|value| refresh_google_token(&client, value))
                {
                    token = refresh;
                    refreshed = true;
                    continue 'bases;
                }
            }
            return unavailable_provider(
                "antigravity",
                "Antigravity",
                "Google AI Pro",
                source,
                if credentials.refresh_token.is_some() && !google_refresh_configured() {
                    "O login local foi encontrado, mas o access token expirou; abra o Antigravity/agy no WSL para renovar a sessão e atualize as cotas."
                } else {
                    "A credencial local expirou; abra o Antigravity/agy para autenticar novamente."
                },
            );
        }
        if response.status().is_success() {
            if let Ok(value) = response.json::<Value>() {
                body = Some(value);
                break;
            }
        }
    }
    let Some(body) = body else {
        return unavailable_provider(
            "antigravity",
            "Antigravity",
            "Google AI Pro",
            source,
            "A fonte oficial não respondeu dentro do limite de 4s.",
        );
    };
    let mut windows = Vec::new();
    collect_antigravity_windows(&body, &mut windows, "Modelo");
    windows.sort_by(|a, b| {
        a.remaining_percent
            .unwrap_or(100.0)
            .partial_cmp(&b.remaining_percent.unwrap_or(100.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    windows.truncate(6);
    if windows.is_empty() {
        return unavailable_provider(
            "antigravity",
            "Antigravity",
            "Google AI Pro",
            source,
            "A resposta não expôs uma cota confirmada.",
        );
    }
    ProviderUsage {
        id: "antigravity".into(),
        provider: "Antigravity".into(),
        subtitle: "Google AI Pro".into(),
        plan: nested_string(&body, &["name", "displayName"]),
        environment: Some("Windows/WSL · OAuth local".into()),
        status: "available".into(),
        windows,
        credits_remaining: None,
        credits_total: None,
        credits_used: None,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
        disabled: false,
    }
}

fn refresh_google_token(client: &Client, refresh_token: &str) -> Option<String> {
    let client_secret = env::var("GOOGLE_CLIENT_SECRET").ok()?;
    let client_id = env::var("GOOGLE_CLIENT_ID").ok()?;
    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .json::<Value>()
        .ok()?
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn google_refresh_configured() -> bool {
    env::var("GOOGLE_CLIENT_ID")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
        && env::var("GOOGLE_CLIENT_SECRET")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
}

fn keyring_secret(service: &str, account: &str) -> Option<Value> {
    let entry = keyring::Entry::new(service, account).ok()?;
    let raw = entry.get_password().ok()?;
    let raw = raw.trim();
    let decoded = raw.strip_prefix("go-keyring-base64:").and_then(|encoded| {
        BASE64
            .decode(encoded)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    });
    let text = decoded.as_deref().unwrap_or(raw);
    serde_json::from_str(text)
        .ok()
        .or_else(|| Some(Value::String(text.to_owned())))
}
fn openrouter_key() -> Option<String> {
    keyring::Entry::new(OPENROUTER_SERVICE, OPENROUTER_ACCOUNT)
        .ok()?
        .get_password()
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

fn opencode_go_key() -> Option<String> {
    keyring::Entry::new(OPENCODE_GO_SERVICE, OPENCODE_GO_ACCOUNT)
        .ok()?
        .get_password()
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

fn query_opencode_go() -> ProviderUsage {
    let source = "OpenCode Go API · /zen/go/v1/usage";
    let Some(key) = opencode_go_key() else {
        return unavailable_provider(
            "opencode-go",
            "OpenCode Go",
            "Assinatura de modelos Open (USD)",
            source,
            "Chave não configurada. Informe uma chave nas configurações de Cotas de IA para consultar a cota do OpenCode Go.",
        );
    };
    let Ok(client) = http_client() else {
        return unavailable_provider(
            "opencode-go",
            "OpenCode Go",
            "Assinatura de modelos Open (USD)",
            source,
            "Cliente HTTP indisponível.",
        );
    };
    let response = client
        .get("https://opencode.ai/zen/go/v1/usage")
        .bearer_auth(key)
        .header("Accept", "application/json")
        .header("x-opencode-session", "pulse")
        .send();
    let Ok(response) = response else {
        return unavailable_provider(
            "opencode-go",
            "OpenCode Go",
            "Assinatura de modelos Open (USD)",
            source,
            "A fonte oficial não respondeu dentro do limite de 4s.",
        );
    };
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return unavailable_provider(
            "opencode-go",
            "OpenCode Go",
            "Assinatura de modelos Open (USD)",
            source,
            "A chave OpenCode Go foi recusada.",
        );
    }
    let Ok(body) = response.json::<Value>() else {
        return unavailable_provider(
            "opencode-go",
            "OpenCode Go",
            "Assinatura de modelos Open (USD)",
            source,
            "Resposta oficial inválida.",
        );
    };
    let usage = body.get("usage").unwrap_or(&body);
    let mut windows = Vec::new();
    for (key, label, minutes, order) in [
        ("rolling", "Janela atual (5h)", 300u64, 0u8),
        ("weekly", "Semanal", 10080u64, 1u8),
        ("monthly", "Mensal", 43200u64, 2u8),
    ] {
        let Some(window) = usage.get(key) else {
            continue;
        };
        let used = percent(window.get("percent"));
        let Some(used) = used else { continue };
        let resets_at = parse_reset(window.get("resetsAt"));
        let expired = resets_at.is_some_and(|reset| reset * 1000 <= now_unix_ms());
        let (used, remaining, desc) = if expired {
            (Some(0.0), Some(100.0), Some("Ciclo renovado".into()))
        } else {
            (Some(used), Some((100.0 - used).clamp(0.0, 100.0)), None)
        };
        windows.push(QuotaWindow {
            id: format!("{key}-{order}"),
            label: label.into(),
            used_percent: used,
            remaining_percent: remaining,
            window_minutes: Some(minutes),
            resets_at_unix_seconds: resets_at,
            reset_description: desc,
            unavailable_reason: None,
        });
    }
    if windows.is_empty() {
        return unavailable_provider(
            "opencode-go",
            "OpenCode Go",
            "Assinatura de modelos Open (USD)",
            source,
            "A resposta não expôs uma cota confirmada.",
        );
    }
    ProviderUsage {
        id: "opencode-go".into(),
        provider: "OpenCode Go".into(),
        subtitle: "Assinatura de modelos Open (USD)".into(),
        plan: Some("Go · $10/mês".into()),
        environment: Some("HTTP oficial · chave no Credential Manager".into()),
        status: "available".into(),
        windows,
        credits_remaining: None,
        credits_total: None,
        credits_used: None,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
        disabled: false,
    }
}

fn query_openrouter() -> ProviderUsage {
    let source = "OpenRouter API · /api/v1/credits";
    let Some(key) = openrouter_key() else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Saldo em Dólares (USD)",
            source,
            "Chave não configurada. Informe uma chave nas configurações de Cotas de IA para consultar o saldo em dólares.",
        );
    };
    let Ok(client) = http_client() else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Saldo em Dólares (USD)",
            source,
            "Cliente HTTP indisponível.",
        );
    };
    let response = client
        .get("https://openrouter.ai/api/v1/credits")
        .bearer_auth(key)
        .header("Accept", "application/json")
        .send();
    let Ok(response) = response else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Saldo em Dólares (USD)",
            source,
            "A fonte oficial não respondeu dentro do limite de 4s.",
        );
    };
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Saldo em Dólares (USD)",
            source,
            "A chave OpenRouter foi recusada.",
        );
    }
    let Ok(body) = response.json::<Value>() else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Saldo em Dólares (USD)",
            source,
            "Resposta oficial inválida.",
        );
    };
    let data = body.get("data").unwrap_or(&body);
    let total = number(data.get("total_credits"));
    let used = number(data.get("total_usage"));
    let remaining = match (total, used) {
        (Some(total), Some(used)) if total >= used => Some(total - used),
        (Some(total), None) => Some(total),
        _ => None,
    };
    let Some(remaining) = remaining else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Saldo em Dólares (USD)",
            source,
            "A resposta não expôs o saldo em dólares.",
        );
    };
    ProviderUsage {
        id: "openrouter".into(),
        provider: "OpenRouter".into(),
        subtitle: "Saldo em Dólares (USD)".into(),
        plan: None,
        environment: Some("HTTP oficial · chave no Credential Manager".into()),
        status: "available".into(),
        windows: Vec::new(),
        credits_remaining: Some(remaining.max(0.0)),
        credits_total: total,
        credits_used: used,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
        disabled: false,
    }
}

fn placeholder_report(status: &str) -> SubscriptionUsageReport {
    SubscriptionUsageReport {
        providers: vec![
            unavailable_provider(
                "claude",
                "Claude Code",
                "Assinatura Anthropic",
                "Leitura local",
                "Aguardando leitura oficial.",
            ),
            unavailable_provider(
                "codex",
                "Codex",
                "ChatGPT",
                "Leitura local",
                "Aguardando leitura oficial.",
            ),
            unavailable_provider(
                "antigravity",
                "Antigravity",
                "Google AI Pro",
                "Leitura local",
                "Aguardando leitura oficial.",
            ),
            unavailable_provider(
                "openrouter",
                "OpenRouter",
                "Saldo em Dólares (USD)",
                "Leitura local",
                "Aguardando leitura oficial.",
            ),
            unavailable_provider(
                "opencode-go",
                "OpenCode Go",
                "Assinatura de modelos Open (USD)",
                "Leitura local",
                "Aguardando leitura oficial.",
            ),
        ],
        sampled_at_unix_ms: now_unix_ms(),
        duration_ms: 0,
        privacy_note: "Consultas locais e oficiais; credenciais nunca são retornadas à interface."
            .into(),
        scan_status: status.into(),
    }
}

static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

pub fn init(handle: tauri::AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

fn cache_file() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
        .join("subscription-cache.json")
}

fn load_persisted_cache() -> Option<SubscriptionUsageReport> {
    let content = fs::read_to_string(cache_file()).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_persisted_cache(report: &SubscriptionUsageReport) {
    let file = cache_file();
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(content) = serde_json::to_string_pretty(report) {
        let _ = fs::write(file, content);
    }
}

fn notify_subscription_updated(report: &SubscriptionUsageReport) {
    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit("pulse://subscription-usage-updated", report);
    }
}

fn preferences_file() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
        .join("subscription-preferences.json")
}

fn load_preferences() -> SubscriptionPreferences {
    fs::read_to_string(preferences_file())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_preferences(preferences: &SubscriptionPreferences) -> Result<(), String> {
    let file = preferences_file();
    if let Some(directory) = file.parent() {
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    let content = serde_json::to_string_pretty(preferences).map_err(|error| error.to_string())?;
    fs::write(file, content).map_err(|error| error.to_string())
}

fn disabled_provider_ids() -> Vec<String> {
    load_preferences()
        .disabled_providers
        .into_iter()
        .filter(|id| PROVIDER_IDS.contains(&id.as_str()))
        .collect()
}

fn apply_disabled_flags(providers: &mut [ProviderUsage], disabled: &[String]) {
    for provider in providers.iter_mut() {
        provider.disabled = disabled.iter().any(|id| id == &provider.id);
    }
}

fn provider_order() -> Vec<String> {
    let mut seen = Vec::new();
    for id in load_preferences().provider_order {
        if PROVIDER_IDS.contains(&id.as_str()) && !seen.contains(&id) {
            seen.push(id);
        }
    }
    seen
}

// Ordena pela preferência do usuário (dois sorts estáveis) e empurra os
// desativados para o final, tanto na tela de cotas quanto no tray.
fn apply_provider_order(providers: &mut [ProviderUsage], order: &[String]) {
    providers.sort_by_key(|provider| {
        order
            .iter()
            .position(|id| id == &provider.id)
            .unwrap_or(usize::MAX)
    });
    providers.sort_by_key(|provider| provider.disabled);
}

// Aplica a ordem salva ao cache em memória e ao disco para o tray refletir
// imediatamente, sem esperar a próxima varredura completa.
fn apply_order_to_cache(order: &[String]) {
    let report = {
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        let Ok(mut guard) = cache.lock() else {
            return;
        };
        let Some(cached) = guard.as_mut() else {
            return;
        };
        apply_provider_order(&mut cached.report.providers, order);
        cached.report.clone()
    };
    save_persisted_cache(&report);
    notify_subscription_updated(&report);
}

// Atualiza imediatamente o cache em memória e o disco para que o tray reflita o
// estado sem esperar a próxima varredura completa.
fn refresh_disabled_flag(provider_id: &str, disabled: bool) {
    let order = provider_order();
    let report = {
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        let Ok(mut guard) = cache.lock() else {
            return;
        };
        let Some(cached) = guard.as_mut() else {
            return;
        };
        if let Some(provider) = cached
            .report
            .providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
        {
            provider.disabled = disabled;
        } else {
            return;
        }
        apply_provider_order(&mut cached.report.providers, &order);
        cached.report.clone()
    };
    save_persisted_cache(&report);
    notify_subscription_updated(&report);
}

fn start_background_scan() {
    if SCAN_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    thread::spawn(|| {
        let started = Instant::now();
        // Preserva o cache anterior da memória ou do disco para não zerar os dados da interface
        let cached_providers = {
            let cache = CACHE.get_or_init(|| Mutex::new(None));
            if let Ok(guard) = cache.lock() {
                guard.as_ref().map(|c| c.report.providers.clone())
            } else {
                None
            }
        }
        .or_else(|| load_persisted_cache().map(|r| r.providers));

        let mut providers =
            cached_providers.unwrap_or_else(|| placeholder_report("running").providers);

        // Garante a presença de todos os provedores canônicos
        for required_id in PROVIDER_IDS {
            if !providers.iter().any(|p| p.id == required_id) {
                providers.push(unavailable_for_id(required_id));
            }
        }

        let disabled = disabled_provider_ids();
        apply_disabled_flags(&mut providers, &disabled);
        apply_provider_order(&mut providers, &provider_order());

        let (sender, receiver) = mpsc::channel();
        for (id, query) in [
            ("claude", query_claude as fn() -> ProviderUsage),
            ("codex", query_codex as fn() -> ProviderUsage),
            ("antigravity", query_antigravity as fn() -> ProviderUsage),
            ("openrouter", query_openrouter as fn() -> ProviderUsage),
            ("opencode-go", query_opencode_go as fn() -> ProviderUsage),
        ] {
            // Provedores desativados mantêm a última leitura sem disparar novas consultas.
            if disabled.iter().any(|disabled_id| disabled_id == id) {
                continue;
            }
            let sender = sender.clone();
            thread::spawn(move || {
                // Keep recursive log inspection off the runtime executor while still using
                // Tauri's bounded blocking pool for filesystem work.
                let provider_started = Instant::now();
                let provider =
                    tauri::async_runtime::block_on(tauri::async_runtime::spawn_blocking(query))
                        .unwrap_or_else(|_| unavailable_for_id(id));
                let operation = format!("subscription_usage_{id}");
                let available = provider.status == "available";
                let message = provider
                    .unavailable_reason
                    .as_deref()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Cota confirmada · {}", provider.source));
                crate::debug::record_message(
                    &operation,
                    provider_started.elapsed().as_millis(),
                    if available { "ok" } else { "error" },
                    &message,
                );
                let _ = sender.send((id, provider));
            });
        }
        drop(sender);
        for (id, provider) in receiver {
            if let Some(slot) = providers.iter_mut().find(|candidate| candidate.id == id) {
                *slot = provider;
            } else {
                providers.push(provider);
            }
            let report = SubscriptionUsageReport {
                providers: providers.clone(),
                sampled_at_unix_ms: now_unix_ms(),
                duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                privacy_note:
                    "Consultas locais e oficiais; credenciais nunca são retornadas à interface."
                        .into(),
                scan_status: "running".into(),
            };
            if let Ok(mut guard) = CACHE.get_or_init(|| Mutex::new(None)).lock() {
                *guard = Some(CachedReport {
                    stored_at: Instant::now(),
                    report: report.clone(),
                });
            }
            notify_subscription_updated(&report);
        }
        let report = SubscriptionUsageReport {
            providers,
            sampled_at_unix_ms: now_unix_ms(),
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            privacy_note:
                "Consultas locais e oficiais; credenciais nunca são retornadas à interface.".into(),
            scan_status: "ready".into(),
        };
        if let Ok(mut guard) = CACHE.get_or_init(|| Mutex::new(None)).lock() {
            *guard = Some(CachedReport {
                stored_at: Instant::now(),
                report: report.clone(),
            });
        }
        save_persisted_cache(&report);
        // Ponto único de gravação do histórico: cobre o app, o painel rápido
        // e o tray, que passam pelo mesmo ciclo de atualização de cotas.
        crate::quota_history::record_report(&report.providers);
        SCAN_RUNNING.store(false, Ordering::Release);
        notify_subscription_updated(&report);
    });
}

fn unavailable_for_id(id: &str) -> ProviderUsage {
    match id {
        "claude" => unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            "Leitura oficial",
            "Consulta falhou.",
        ),
        "codex" => unavailable_provider(
            "codex",
            "Codex",
            "ChatGPT",
            "Leitura oficial",
            "Consulta falhou.",
        ),
        "antigravity" => unavailable_provider(
            "antigravity",
            "Antigravity",
            "Google AI Pro",
            "Leitura oficial",
            "Consulta falhou.",
        ),
        "openrouter" => unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Saldo em Dólares (USD)",
            "Leitura oficial",
            "Consulta falhou.",
        ),
        _ => unavailable_provider(
            "opencode-go",
            "OpenCode Go",
            "Assinatura de modelos Open (USD)",
            "Leitura oficial",
            "Consulta falhou.",
        ),
    }
}

fn get_subscription_usage_sync(force_refresh: bool) -> SubscriptionUsageReport {
    let cache = CACHE.get_or_init(|| {
        let initial = load_persisted_cache().map(|report| CachedReport {
            stored_at: Instant::now() - CACHE_TTL,
            report,
        });
        Mutex::new(initial)
    });
    if !force_refresh {
        if let Ok(guard) = cache.lock() {
            if let Some(cached) = guard
                .as_ref()
                .filter(|cached| cached.stored_at.elapsed() < CACHE_TTL)
            {
                return cached.report.clone();
            }
        }
    }
    let report = if let Ok(guard) = cache.lock() {
        guard.as_ref().map(|cached| cached.report.clone())
    } else {
        None
    };
    start_background_scan();
    let mut snapshot = report.unwrap_or_else(|| placeholder_report("running"));
    if SCAN_RUNNING.load(Ordering::Acquire) {
        snapshot.scan_status = "running".into();
    }
    snapshot
}

#[tauri::command]
pub async fn get_subscription_usage(force_refresh: Option<bool>) -> SubscriptionUsageReport {
    let started = Instant::now();
    let report = tauri::async_runtime::spawn_blocking(move || {
        get_subscription_usage_sync(force_refresh.unwrap_or(false))
    })
    .await
    .unwrap_or_else(|_| placeholder_report("error"));
    let available = report
        .providers
        .iter()
        .filter(|provider| provider.status == "available")
        .count();
    crate::debug::record(
        "subscription_usage",
        started.elapsed().as_millis(),
        if available > 0 {
            Ok(())
        } else {
            Err("Nenhum provedor expôs cotas")
        },
    );
    report
}

fn mask_key(key: &str) -> String {
    let tail = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("••••••••{tail}")
}
#[tauri::command]
pub async fn get_subscription_settings() -> SubscriptionSettings {
    tauri::async_runtime::spawn_blocking(get_subscription_settings_sync)
        .await
        .unwrap_or_else(|_| current_settings())
}

fn current_settings() -> SubscriptionSettings {
    let openrouter = openrouter_key();
    let opencode_go = opencode_go_key();
    SubscriptionSettings {
        openrouter_configured: openrouter.is_some(),
        openrouter_masked: openrouter.as_deref().map(mask_key),
        opencode_go_configured: opencode_go.is_some(),
        opencode_go_masked: opencode_go.as_deref().map(mask_key),
        disabled_providers: disabled_provider_ids(),
        provider_order: provider_order(),
    }
}

fn get_subscription_settings_sync() -> SubscriptionSettings {
    current_settings()
}

fn invalidate_cache_and_rescan() {
    if let Ok(mut guard) = CACHE.get_or_init(|| Mutex::new(None)).lock() {
        if let Some(cached) = guard.as_mut() {
            cached.stored_at = Instant::now() - CACHE_TTL;
        }
    }
    start_background_scan();
}

#[tauri::command]
pub async fn set_openrouter_api_key(api_key: String) -> Result<SubscriptionSettings, String> {
    tauri::async_runtime::spawn_blocking(move || set_openrouter_api_key_sync(&api_key))
        .await
        .map_err(|_| "Falha ao guardar a chave com segurança.".to_string())?
}

fn set_openrouter_api_key_sync(api_key: &str) -> Result<SubscriptionSettings, String> {
    let key = api_key.trim();
    if key.len() < 20 || !key.starts_with("sk-or-") || key.chars().any(char::is_whitespace) {
        return Err("Informe uma chave OpenRouter válida (sk-or-…).".into());
    }
    let entry = keyring::Entry::new(OPENROUTER_SERVICE, OPENROUTER_ACCOUNT)
        .map_err(|_| "O Credential Manager do Windows está indisponível.".to_string())?;
    entry
        .set_password(key)
        .map_err(|_| "Não foi possível guardar a chave no Credential Manager.".to_string())?;
    invalidate_cache_and_rescan();
    Ok(current_settings())
}

#[tauri::command]
pub async fn clear_openrouter_api_key() -> Result<SubscriptionSettings, String> {
    tauri::async_runtime::spawn_blocking(clear_openrouter_api_key_sync)
        .await
        .map_err(|_| "Falha ao remover a chave com segurança.".to_string())?
}

fn clear_openrouter_api_key_sync() -> Result<SubscriptionSettings, String> {
    let entry = keyring::Entry::new(OPENROUTER_SERVICE, OPENROUTER_ACCOUNT)
        .map_err(|_| "O Credential Manager do Windows está indisponível.".to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(_) => return Err("Não foi possível remover a chave do Credential Manager.".into()),
    }
    invalidate_cache_and_rescan();
    Ok(current_settings())
}

#[tauri::command]
pub async fn set_opencode_go_api_key(api_key: String) -> Result<SubscriptionSettings, String> {
    tauri::async_runtime::spawn_blocking(move || set_opencode_go_api_key_sync(&api_key))
        .await
        .map_err(|_| "Falha ao guardar a chave com segurança.".to_string())?
}

fn set_opencode_go_api_key_sync(api_key: &str) -> Result<SubscriptionSettings, String> {
    let key = api_key.trim();
    if key.len() < 20 || !key.starts_with("sk-") || key.chars().any(char::is_whitespace) {
        return Err("Informe uma chave OpenCode Go válida (sk-…).".into());
    }
    let entry = keyring::Entry::new(OPENCODE_GO_SERVICE, OPENCODE_GO_ACCOUNT)
        .map_err(|_| "O Credential Manager do Windows está indisponível.".to_string())?;
    entry
        .set_password(key)
        .map_err(|_| "Não foi possível guardar a chave no Credential Manager.".to_string())?;
    invalidate_cache_and_rescan();
    Ok(current_settings())
}

#[tauri::command]
pub async fn clear_opencode_go_api_key() -> Result<SubscriptionSettings, String> {
    tauri::async_runtime::spawn_blocking(clear_opencode_go_api_key_sync)
        .await
        .map_err(|_| "Falha ao remover a chave com segurança.".to_string())?
}

fn clear_opencode_go_api_key_sync() -> Result<SubscriptionSettings, String> {
    let entry = keyring::Entry::new(OPENCODE_GO_SERVICE, OPENCODE_GO_ACCOUNT)
        .map_err(|_| "O Credential Manager do Windows está indisponível.".to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(_) => return Err("Não foi possível remover a chave do Credential Manager.".into()),
    }
    invalidate_cache_and_rescan();
    Ok(current_settings())
}

#[tauri::command]
pub async fn set_provider_enabled(
    provider_id: String,
    enabled: bool,
) -> Result<SubscriptionSettings, String> {
    tauri::async_runtime::spawn_blocking(move || set_provider_enabled_sync(&provider_id, enabled))
        .await
        .map_err(|_| "Falha ao atualizar o provedor.".to_string())?
}

fn set_provider_enabled_sync(
    provider_id: &str,
    enabled: bool,
) -> Result<SubscriptionSettings, String> {
    if !PROVIDER_IDS.contains(&provider_id) {
        return Err("Provedor de IA desconhecido.".into());
    }
    let mut preferences = load_preferences();
    preferences
        .disabled_providers
        .retain(|id| id != provider_id && PROVIDER_IDS.contains(&id.as_str()));
    if !enabled {
        preferences.disabled_providers.push(provider_id.to_owned());
        preferences.disabled_providers.dedup();
    }
    save_preferences(&preferences)?;
    refresh_disabled_flag(provider_id, !enabled);
    invalidate_cache_and_rescan();
    Ok(current_settings())
}

#[tauri::command]
pub async fn set_provider_order(order: Vec<String>) -> Result<SubscriptionSettings, String> {
    tauri::async_runtime::spawn_blocking(move || set_provider_order_sync(order))
        .await
        .map_err(|_| "Falha ao atualizar a ordem dos provedores.".to_string())?
}

fn set_provider_order_sync(order: Vec<String>) -> Result<SubscriptionSettings, String> {
    let mut sanitized = Vec::new();
    for id in order {
        if PROVIDER_IDS.contains(&id.as_str()) && !sanitized.contains(&id) {
            sanitized.push(id);
        }
    }
    let mut preferences = load_preferences();
    preferences.provider_order = sanitized;
    save_preferences(&preferences)?;
    let stored = provider_order();
    apply_order_to_cache(&stored);
    Ok(current_settings())
}

#[allow(dead_code)]
fn strip_terminal(raw: &str) -> String {
    let ansi = Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")
        .expect("regex ANSI válida");
    ansi.replace_all(raw, "")
        .replace('\r', "\n")
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        codex_from_rate_limits, mask_key, now_unix_ms, parse_antigravity_cli_usage,
        parse_antigravity_credential, parse_claude_cli_usage, parse_claude_credential,
        quota_from_used, strip_terminal,
    };
    use serde_json::json;
    #[test]
    fn maps_codex_root_and_model_specific_limits() {
        let provider = codex_from_rate_limits(&json!({"rateLimits":{"planType":"plus","primary":{"usedPercent":18,"windowMinutes":300,"resetsAt":4_000_000_000_i64},"secondary":{"usedPercent":59,"windowMinutes":10080,"resetsAt":4_000_100_000_i64},"credits":{"hasCredits":false}},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":18}},"reserve":{"limitName":"GPT Reserve","primary":{"usedPercent":38,"resetsAt":4_000_200_000_i64}}}}), "teste").unwrap();
        assert_eq!(provider.plan.as_deref(), Some("plus"));
        assert_eq!(provider.windows.len(), 3);
        assert_eq!(provider.windows[0].remaining_percent, Some(82.0));
        assert_eq!(provider.credits_remaining, Some(0.0));
    }
    #[test]
    fn absent_percentage_never_becomes_fake_zero() {
        assert!(
            quota_from_used("session", "Sessão", Some(&json!({"windowMinutes":300}))).is_none()
        );
    }
    #[test]
    fn reads_official_wham_usage_shape_with_snake_case_windows() {
        let provider = codex_from_rate_limits(
            &json!({
                "plan_type": "plus",
                "rate_limit": {
                    "allowed": true,
                    "primary_window": {
                        "used_percent": 0,
                        "limit_window_seconds": 18000,
                        "reset_after_seconds": 18000,
                        "reset_at": 4_000_000_000_i64
                    },
                    "secondary_window": {
                        "used_percent": 98,
                        "limit_window_seconds": 604800,
                        "reset_after_seconds": 258700,
                        "reset_at": 4_000_100_000_i64
                    }
                },
                "credits": {"has_credits": false, "balance": "0"},
                "rate_limit_reset_credits": {"available_count": 2, "applicable_available_count": 0}
            }),
            "teste",
        )
        .unwrap();
        assert_eq!(provider.plan.as_deref(), Some("plus"));
        assert_eq!(provider.windows.len(), 2);
        assert_eq!(provider.windows[0].label, "Sessão");
        assert_eq!(provider.windows[0].window_minutes, Some(300));
        assert_eq!(provider.windows[0].remaining_percent, Some(100.0));
        assert_eq!(provider.windows[1].window_minutes, Some(10080));
        assert_eq!(provider.windows[1].remaining_percent, Some(2.0));
        assert_eq!(provider.reset_credits_available, Some(2));
    }
    #[test]
    fn relative_reset_seconds_are_used_when_absolute_timestamp_is_absent() {
        let window = quota_from_used(
            "session",
            "Sessão",
            Some(&json!({"used_percent": 10, "window_minutes": 300, "reset_after_seconds": 3600})),
        )
        .unwrap();
        assert_eq!(window.remaining_percent, Some(90.0));
        assert!(window.resets_at_unix_seconds.is_some());
        assert!(window.resets_at_unix_seconds.unwrap() * 1000 > now_unix_ms());
    }
    #[test]
    fn resets_expired_quota_to_full_remaining_and_indicates_renewal() {
        let expired_window = quota_from_used(
            "weekly",
            "Semanal",
            Some(&json!({"used_percent": 80, "window_minutes": 10080, "resets_at": 1_000_000_000_i64})),
        )
        .unwrap();
        assert_eq!(expired_window.used_percent, Some(0.0));
        assert_eq!(expired_window.remaining_percent, Some(100.0));
        assert_eq!(expired_window.reset_description.as_deref(), Some("Ciclo renovado"));
        assert!(expired_window.unavailable_reason.is_none());
    }
    #[test]
    fn terminal_escape_sequences_are_removed_before_parsing() {
        assert_eq!(strip_terminal("\u{1b}[31m42%\u{1b}[0m\r\n"), "42%\n\n");
    }
    #[test]
    fn masks_provider_keys_before_they_reach_the_ui() {
        assert_eq!(mask_key("sk-or-v1-abcdefghijkl7890"), "••••••••7890");
    }

    #[test]
    fn reads_claude_oauth_shape_with_refresh_metadata() {
        let credential = parse_claude_credential(&json!({
            "claudeAiOauth": {
                "accessToken": "access-token",
                "refreshToken": "refresh-token",
                "subscriptionType": "team",
                "expiresAt": 4_000_000_000_000_i64
            }
        }))
        .unwrap();
        assert_eq!(credential.access_token, "access-token");
        assert_eq!(credential.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(credential.plan.as_deref(), Some("team"));
        assert_eq!(credential.expires_at_unix_ms, Some(4_000_000_000_000));
    }

    #[test]
    fn reads_antigravity_nested_wsl_token_shape() {
        let credential = parse_antigravity_credential(&json!({
            "token": {
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "expiry": "2099-01-02T03:04:05Z"
            },
            "auth_method": "consumer"
        }))
        .unwrap();
        assert_eq!(credential.access_token, "access-token");
        assert_eq!(credential.refresh_token.as_deref(), Some("refresh-token"));
        assert!(credential.expires_at_unix_ms.is_some());
    }

    #[test]
    fn parses_claude_usage_from_the_local_cli() {
        let windows = parse_claude_cli_usage(
            "Current session: 11% used · resets Sep 15, 4pm (America/Sao_Paulo)\nCurrent week (all models): 87% used · resets Sep 16, 8am (America/Sao_Paulo)",
        );
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].remaining_percent, Some(89.0));
        assert_eq!(windows[1].remaining_percent, Some(13.0));
        assert_eq!(
            windows[0].reset_description.as_deref(),
            Some("resets Sep 15, 4pm (America/Sao_Paulo)")
        );
    }

    #[test]
    fn parses_antigravity_usage_from_the_local_cli() {
        let windows = parse_antigravity_cli_usage(
            "Gemini Models\tWeekly Limit Remaining\t52%\t2026-09-18T01:08:10Z\nGemini Models\tFive Hour Limit Remaining\t100%\t2026-09-15T20:22:49Z\nClaude and GPT models\tWeekly Limit Remaining\t66%\t2026-09-18T03:29:34Z",
        );
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].remaining_percent, Some(52.0));
        assert_eq!(windows[1].window_minutes, Some(300));
        assert!(windows[2].resets_at_unix_seconds.is_some());
    }
}
