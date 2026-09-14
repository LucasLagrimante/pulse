//! Fast, local-first subscription quota collection.
//!
//! The UI never starts a terminal. A command returns a local snapshot immediately and a single
//! background worker refreshes official HTTP sources. Tokens stay in the OS credential store (or
//! the provider's own credential file) and are never serialized into the report.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::DateTime;
use regex::Regex;
use reqwest::{blocking::Client, StatusCode};
use serde::Serialize;
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
const MAX_DEPTH: usize = 3;
const MAX_FILES_TOTAL: usize = 2500;
const MAX_SCAN_DURATION: Duration = Duration::from_millis(1500);
const FILES_TO_INSPECT: usize = 24;
const OPENROUTER_SERVICE: &str = "Pulse";
const OPENROUTER_ACCOUNT: &str = "openrouter-api-key";

#[derive(Debug, Clone, Serialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, PartialEq)]
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
}

#[derive(Debug, Clone, Serialize, PartialEq)]
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
    }
}

fn json_field<'a>(value: &'a Value, snake: &str, camel: &str) -> Option<&'a Value> {
    value.get(snake).or_else(|| value.get(camel))
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

fn quota_from_used(id: &str, label: &str, value: Option<&Value>) -> Option<QuotaWindow> {
    let value = value?.as_object()?;
    let used_percent = percent(
        value
            .get("used_percent")
            .or_else(|| value.get("usedPercent")),
    )?;
    let resets_at = parse_reset(value.get("resets_at").or_else(|| value.get("resetsAt")));
    let expired = resets_at.is_some_and(|reset| reset * 1000 <= now_unix_ms());
    Some(QuotaWindow {
        id: id.into(),
        label: label.into(),
        used_percent: (!expired).then_some(used_percent),
        remaining_percent: (!expired).then_some((100.0 - used_percent).clamp(0.0, 100.0)),
        window_minutes: value
            .get("window_minutes")
            .or_else(|| value.get("windowMinutes"))
            .and_then(Value::as_u64),
        resets_at_unix_seconds: resets_at,
        reset_description: None,
        unavailable_reason: expired
            .then(|| "A leitura desta janela venceu; atualize a cota.".into()),
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
    let output = crate::process::hidden_command("where.exe").arg(name).output().ok()?;
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
    let rate_limits = json_field(data, "rate_limits", "rateLimits").unwrap_or(data);
    let mut windows = Vec::new();
    if let Some(window) = quota_from_used("session", "Sessão", rate_limits.get("primary")) {
        windows.push(window);
    }
    if let Some(window) = quota_from_used("weekly", "Semanal", rate_limits.get("secondary")) {
        windows.push(window);
    }
    if let Some(groups) = json_field(data, "rate_limits_by_limit_id", "rateLimitsByLimitId")
        .and_then(Value::as_object)
    {
        for (limit_id, value) in groups {
            if limit_id.eq_ignore_ascii_case("codex") {
                continue;
            }
            let name = json_field(value, "limit_name", "limitName")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(limit_id);
            if let Some(window) = quota_from_used(
                &format!("{limit_id}-session"),
                &format!("{name} · sessão"),
                value.get("primary"),
            ) {
                windows.push(window);
            }
            if let Some(window) = quota_from_used(
                &format!("{limit_id}-weekly"),
                &format!("{name} · semanal"),
                value.get("secondary"),
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
        plan: json_field(rate_limits, "plan_type", "planType")
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned),
        environment: Some("Windows · assinatura única".into()),
        status: "available".into(),
        windows,
        credits_remaining: credits(rate_limits.get("credits")),
        credits_total: None,
        credits_used: None,
        reset_credits_available: json_field(
            rate_limits,
            "rate_limit_reset_credits",
            "rateLimitResetCredits",
        )
        .and_then(|value| json_field(value, "available_count", "availableCount"))
        .and_then(Value::as_u64),
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
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

fn query_codex() -> ProviderUsage {
    if let Some(local) = codex_from_local_logs() {
        return local;
    }
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

fn claude_credentials() -> Option<(String, Option<String>)> {
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
    if let Some(token) = env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        return Some((token, None));
    }
    for path in paths {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let oauth = value
            .get("claudeAiOauth")
            .or_else(|| value.get("claude_ai_oauth"))
            .unwrap_or(&value);
        let token = oauth
            .get("accessToken")
            .or_else(|| oauth.get("access_token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())?;
        let plan = oauth
            .get("subscriptionType")
            .or_else(|| oauth.get("subscription_type"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Some((token.to_owned(), plan));
    }
    None
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| "cliente HTTP indisponível".into())
}

fn query_claude() -> ProviderUsage {
    let source = "Anthropic OAuth · /api/oauth/usage";
    let Some((token, plan)) = claude_credentials() else {
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            "Credencial local não encontrada.",
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
        .bearer_auth(token)
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
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return unavailable_provider(
            "claude",
            "Claude Code",
            "Assinatura Anthropic",
            source,
            "A credencial local expirou; autentique novamente no Claude Code.",
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
                windows.push(QuotaWindow {
                    id: id.into(),
                    label: label.into(),
                    used_percent: Some(used.clamp(0.0, 100.0)),
                    remaining_percent: Some((100.0 - used).clamp(0.0, 100.0)),
                    window_minutes: Some(minutes),
                    resets_at_unix_seconds: parse_reset(value.get("resets_at")),
                    reset_description: None,
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
    ProviderUsage {
        id: "claude".into(),
        provider: "Claude Code".into(),
        subtitle: "Assinatura Anthropic".into(),
        plan,
        environment: Some("Windows · OAuth local".into()),
        status: "available".into(),
        windows,
        credits_remaining: None,
        credits_total: None,
        credits_used: None,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
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
    let source = "Google Cloud Code · retrieveUserQuotaSummary";
    let credentials = keyring_secret("gemini", "antigravity");
    let token = credentials.as_ref().and_then(|raw| {
        raw.as_str().map(str::to_owned).or_else(|| {
            nested_string(
                raw,
                &[
                    "access_token",
                    "accessToken",
                    "id_token",
                    "idToken",
                    "token",
                    "bearerToken",
                ],
            )
        })
    });
    let refresh_token = credentials
        .as_ref()
        .and_then(|raw| nested_string(raw, &["refresh_token", "refreshToken"]));
    let Some(token) = token else {
        return unavailable_provider(
            "antigravity",
            "Antigravity",
            "Google AI Pro",
            source,
            "Credencial segura do Antigravity não encontrada.",
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
    let mut token = token;
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
                if let Some(refresh) = refresh_token
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
                "A credencial segura expirou; abra o Antigravity para autenticar novamente.",
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
        environment: Some("HTTP oficial · sem terminal".into()),
        status: "available".into(),
        windows,
        credits_remaining: None,
        credits_total: None,
        credits_used: None,
        reset_credits_available: None,
        sampled_at_unix_ms: Some(now_unix_ms()),
        source: source.into(),
        unavailable_reason: None,
    }
}

fn refresh_google_token(client: &Client, refresh_token: &str) -> Option<String> {
    let client_secret = env::var("GOOGLE_CLIENT_SECRET").ok()?;
    let client_id = env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| {
        "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com".to_string()
    });
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

fn query_openrouter() -> ProviderUsage {
    let source = "OpenRouter API · /api/v1/credits";
    let Some(key) = openrouter_key() else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Créditos de API",
            source,
            "Chave não configurada. Informe uma chave nas configurações de Cotas de IA.",
        );
    };
    let Ok(client) = http_client() else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Créditos de API",
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
            "Créditos de API",
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
            "Créditos de API",
            source,
            "A chave OpenRouter foi recusada.",
        );
    }
    let Ok(body) = response.json::<Value>() else {
        return unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Créditos de API",
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
            "Créditos de API",
            source,
            "A resposta não expôs o saldo de créditos.",
        );
    };
    ProviderUsage {
        id: "openrouter".into(),
        provider: "OpenRouter".into(),
        subtitle: "Créditos de API".into(),
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
                "Créditos de API",
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

fn start_background_scan() {
    if SCAN_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    thread::spawn(|| {
        let started = Instant::now();
        let mut providers = placeholder_report("running").providers;
        let (sender, receiver) = mpsc::channel();
        for (id, query) in [
            ("claude", query_claude as fn() -> ProviderUsage),
            ("codex", query_codex as fn() -> ProviderUsage),
            ("antigravity", query_antigravity as fn() -> ProviderUsage),
            ("openrouter", query_openrouter as fn() -> ProviderUsage),
        ] {
            let sender = sender.clone();
            thread::spawn(move || {
                // Keep recursive log inspection off the runtime executor while still using
                // Tauri's bounded blocking pool for filesystem work.
                let provider =
                    tauri::async_runtime::block_on(tauri::async_runtime::spawn_blocking(query))
                        .unwrap_or_else(|_| unavailable_for_id(id));
                let _ = sender.send((id, provider));
            });
        }
        drop(sender);
        for (id, provider) in receiver {
            if let Some(slot) = providers.iter_mut().find(|candidate| candidate.id == id) {
                *slot = provider;
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
                    report,
                });
            }
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
                report,
            });
        }
        SCAN_RUNNING.store(false, Ordering::Release);
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
        _ => unavailable_provider(
            "openrouter",
            "OpenRouter",
            "Créditos de API",
            "Leitura oficial",
            "Consulta falhou.",
        ),
    }
}
fn get_subscription_usage_sync(force_refresh: bool) -> SubscriptionUsageReport {
    let cache = CACHE.get_or_init(|| Mutex::new(None));
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
        .unwrap_or(SubscriptionSettings {
            openrouter_configured: false,
            openrouter_masked: None,
        })
}

fn get_subscription_settings_sync() -> SubscriptionSettings {
    let key = openrouter_key();
    SubscriptionSettings {
        openrouter_configured: key.is_some(),
        openrouter_masked: key.as_deref().map(mask_key),
    }
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
    if let Ok(mut guard) = CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *guard = None;
    }
    Ok(SubscriptionSettings {
        openrouter_configured: true,
        openrouter_masked: Some(mask_key(key)),
    })
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
    if let Ok(mut guard) = CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *guard = None;
    }
    Ok(SubscriptionSettings {
        openrouter_configured: false,
        openrouter_masked: None,
    })
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
    use super::{codex_from_rate_limits, mask_key, quota_from_used, strip_terminal};
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
    fn terminal_escape_sequences_are_removed_before_parsing() {
        assert_eq!(strip_terminal("\u{1b}[31m42%\u{1b}[0m\r\n"), "42%\n\n");
    }
    #[test]
    fn masks_provider_keys_before_they_reach_the_ui() {
        assert_eq!(mask_key("sk-or-v1-abcdefghijkl7890"), "••••••••7890");
    }
}
