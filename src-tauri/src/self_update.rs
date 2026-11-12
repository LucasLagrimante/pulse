//! Atualização automática do próprio aplicativo Pulse ao iniciar.
//!
//! O Pulse consulta o manifesto oficial (`https://pulse.lagrimante.com.br/release.json`),
//! compara a versão semântica publicada com `CARGO_PKG_VERSION` e, quando há uma
//! versão mais nova e a preferência está ativa (padrão: ativada), baixa o instalador
//! NSIS da GitHub Release oficial, valida o hash `SHA-256` contra o manifesto e
//! dispara a instalação silenciosa (`/S`) com reinício automático.
//!
//! Salvaguardas arquiteturais:
//! - Somente URLs da GitHub Release oficial (`https://github.com/LucasLagrimante/pulse/releases/download/`)
//!   ou do domínio canônico (`https://pulse.lagrimante.com.br/download/`) são aceitas.
//! - O binário baixado é verificado integralmente por SHA-256 antes de tocar o disco
//!   como executável; qualquer divergência aborta a atualização imediatamente.
//! - Proteção anti-loop: se a mesma versão já foi tentada há menos de 6 horas e o
//!   executável ainda está na versão antiga, a tentativa automática na partida não se
//!   repete indefinidamente.
//! - Subprocesso PowerShell recebe parâmetros exclusivamente por variáveis de ambiente
//!   (`$env:PULSE_UPDATE_*`), sem interpolação na linha de comando.

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

pub const SELF_UPDATE_EVENT: &str = "pulse://self-update-status";
pub const UPDATED_RELAUNCH_ARG: &str = "--updated-relaunch";

const MANIFEST_URL: &str = "https://pulse.lagrimante.com.br/release.json";
const ALLOWED_URL_PREFIXES: [&str; 2] = [
    "https://github.com/LucasLagrimante/pulse/releases/download/",
    "https://pulse.lagrimante.com.br/download/",
];
const CONFIG_FILE_NAME: &str = "auto-update.json";
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(6);
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const MIN_INSTALLER_BYTES: usize = 512 * 1024;
const MAX_INSTALLER_BYTES: usize = 60 * 1024 * 1024;
const RETRY_COOLDOWN_MS: u64 = 6 * 60 * 60 * 1000;

static IS_RUNNING: AtomicBool = AtomicBool::new(false);
static STATUS_STATE: OnceLock<Mutex<SelfUpdateStatus>> = OnceLock::new();

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelfUpdateConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub last_checked_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub last_attempted_version: Option<String>,
    #[serde(default)]
    pub last_attempt_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub last_updated_to_version: Option<String>,
    #[serde(default)]
    pub last_updated_from_version: Option<String>,
    #[serde(default)]
    pub last_updated_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for SelfUpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            last_checked_at_unix_ms: None,
            last_attempted_version: None,
            last_attempt_at_unix_ms: None,
            last_updated_to_version: None,
            last_updated_from_version: None,
            last_updated_at_unix_ms: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelfUpdateStatus {
    pub enabled: bool,
    pub current_version: String,
    /// `"idle"` | `"checking"` | `"up_to_date"` | `"available"` | `"downloading"` | `"verifying"` | `"installing"` | `"error"`
    pub phase: String,
    pub available_version: Option<String>,
    pub release_size_formatted: Option<String>,
    pub last_checked_at_unix_ms: Option<u64>,
    pub last_updated_to_version: Option<String>,
    pub last_updated_from_version: Option<String>,
    pub last_updated_at_unix_ms: Option<u64>,
    pub message: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    pub version: String,
    pub installer: ReleaseInstaller,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInstaller {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub asset_url: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub size_formatted: Option<String>,
    pub sha256: String,
}

struct RunGuard;

impl RunGuard {
    fn acquire() -> Option<Self> {
        IS_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        IS_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn config_file_in(base: &Path) -> PathBuf {
    base.join("Pulse").join(CONFIG_FILE_NAME)
}

fn config_file() -> PathBuf {
    let base = env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    config_file_in(&base)
}

pub fn load_config_from(path: &Path) -> SelfUpdateConfig {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<SelfUpdateConfig>(&content).ok())
        .unwrap_or_default()
}

pub fn save_config_to(path: &Path, config: &SelfUpdateConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Não foi possível criar a pasta de configuração: {error}"))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| format!("Erro ao serializar configuração de atualização: {error}"))?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, content)
        .and_then(|()| fs::rename(&temp, path))
        .map_err(|error| format!("Não foi possível salvar a configuração de atualização: {error}"))
}

pub fn load_config() -> SelfUpdateConfig {
    load_config_from(&config_file())
}

fn save_config(config: &SelfUpdateConfig) -> Result<(), String> {
    save_config_to(&config_file(), config)
}

pub fn is_auto_update_enabled() -> bool {
    load_config().enabled
}

fn initial_status_from_config(config: &SelfUpdateConfig) -> SelfUpdateStatus {
    SelfUpdateStatus {
        enabled: config.enabled,
        current_version: current_version().to_string(),
        phase: if config.last_error.is_some() {
            "error".to_string()
        } else {
            "idle".to_string()
        },
        available_version: None,
        release_size_formatted: None,
        last_checked_at_unix_ms: config.last_checked_at_unix_ms,
        last_updated_to_version: config.last_updated_to_version.clone(),
        last_updated_from_version: config.last_updated_from_version.clone(),
        last_updated_at_unix_ms: config.last_updated_at_unix_ms,
        message: config
            .last_updated_to_version
            .as_deref()
            .filter(|ver| *ver == current_version())
            .map(|ver| format!("Pulse atualizado para a versão {ver}")),
        error: config.last_error.clone(),
    }
}

fn status_store() -> &'static Mutex<SelfUpdateStatus> {
    STATUS_STATE.get_or_init(|| {
        let config = load_config();
        Mutex::new(initial_status_from_config(&config))
    })
}

fn update_and_emit(app: Option<&AppHandle>, next: SelfUpdateStatus) -> SelfUpdateStatus {
    if let Ok(mut guard) = status_store().lock() {
        *guard = next.clone();
    }
    if let Some(handle) = app {
        let _ = handle.emit(SELF_UPDATE_EVENT, &next);
    }
    next
}

/// Reconcilia o estado persistido logo na partida: se a execução anterior
/// disparou o instalador para a versão que agora está rodando, registra a
/// conclusão e limpa a marca de tentativa.
pub fn reconcile_startup_config(
    mut config: SelfUpdateConfig,
    running_version: &str,
    now_unix_ms: u64,
) -> (SelfUpdateConfig, bool) {
    if config.last_attempted_version.as_deref() == Some(running_version) {
        config.last_updated_to_version = Some(running_version.to_string());
        config.last_updated_at_unix_ms = Some(now_unix_ms);
        config.last_attempted_version = None;
        config.last_error = None;
        return (config, true);
    }
    (config, false)
}

/// Verifica se a tentativa automática deve ser bloqueada por já ter sido
/// tentada recentemente para a mesma versão sem sucesso.
pub fn should_skip_auto_attempt(
    config: &SelfUpdateConfig,
    target_version: &str,
    now_unix_ms: u64,
) -> bool {
    if config.last_attempted_version.as_deref() != Some(target_version) {
        return false;
    }
    let Some(attempted_at) = config.last_attempt_at_unix_ms else {
        return false;
    };
    now_unix_ms.saturating_sub(attempted_at) < RETRY_COOLDOWN_MS
}

/// Extrai `(major, minor, patch)` de uma string SemVer (`"1.4.0"`, `"v1.4.0-rc1"`).
pub fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let cleaned = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()?;
    let mut parts = cleaned.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Retorna `true` somente quando `remote` é estritamente mais nova que `current`.
pub fn is_newer_version(current: &str, remote: &str) -> bool {
    let (Some(curr), Some(rem)) = (parse_semver(current), parse_semver(remote)) else {
        return false;
    };
    rem > curr
}

/// Valida e normaliza o hash SHA-256 esperado (64 dígitos hexadecimais).
pub fn validate_sha256_hex(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.len() != 64 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("O manifesto de release contém um SHA-256 inválido.".to_string());
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// Escolhe e valida a URL de download do instalador, aceitando apenas origens oficiais.
pub fn select_and_validate_installer_url(installer: &ReleaseInstaller) -> Result<String, String> {
    let candidate = installer
        .asset_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| installer.url.as_deref().filter(|s| !s.trim().is_empty()))
        .ok_or_else(|| "O manifesto de release não informa a URL do instalador.".to_string())?
        .trim();

    if !ALLOWED_URL_PREFIXES
        .iter()
        .any(|prefix| candidate.starts_with(prefix))
    {
        return Err(
            "A URL do instalador no manifesto não pertence ao repositório ou domínio oficial do Pulse."
                .to_string(),
        );
    }
    if !candidate.ends_with(".exe")
        || candidate.contains('"')
        || candidate.contains('\'')
        || candidate.chars().any(char::is_whitespace)
    {
        return Err("A URL do instalador no manifesto tem formato inválido.".to_string());
    }
    Ok(candidate.to_string())
}

/// Implementação FIPS 180-4 de SHA-256 em Rust puro (sem dependências externas).
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let (chunks, _) = padded.as_chunks::<64>();
    for chunk in chunks {
        let mut w = [0u32; 64];
        let (words, _) = chunk.as_chunks::<4>();
        for (i, word) in words.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{word:08x}");
    }
    out
}

fn fetch_manifest() -> Result<ReleaseManifest, String> {
    let client = Client::builder()
        .timeout(MANIFEST_TIMEOUT)
        .user_agent(format!("Pulse/{}", current_version()))
        .build()
        .map_err(|error| format!("Não foi possível iniciar o cliente HTTP: {error}"))?;

    let response = client
        .get(MANIFEST_URL)
        .header("Accept", "application/json")
        .header("Cache-Control", "no-cache")
        .send()
        .map_err(|error| format!("Não foi possível consultar o manifesto de versão: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "O servidor de atualização respondeu com status HTTP {}",
            response.status().as_u16()
        ));
    }

    response
        .json::<ReleaseManifest>()
        .map_err(|error| format!("O manifesto de versão retornado é inválido: {error}"))
}

fn download_installer_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(format!("Pulse/{}", current_version()))
        .build()
        .map_err(|error| format!("Não foi possível iniciar o download da atualização: {error}"))?;

    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("Falha ao baixar o instalador do Pulse: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Falha ao baixar o instalador (status HTTP {})",
            response.status().as_u16()
        ));
    }

    let mut bytes = Vec::new();
    let mut limited = response.take((MAX_INSTALLER_BYTES as u64) + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Erro durante a leitura do instalador: {error}"))?;

    if bytes.len() > MAX_INSTALLER_BYTES {
        return Err("O arquivo baixado excede o tamanho máximo permitido (60 MB).".to_string());
    }
    if bytes.len() < MIN_INSTALLER_BYTES {
        return Err("O arquivo baixado está incompleto ou corrompido.".to_string());
    }
    // Todo executável PE do Windows começa com a assinatura MZ (0x4D, 0x5A).
    if bytes.get(0..2) != Some(b"MZ") {
        return Err("O arquivo baixado não é um executável válido do Windows.".to_string());
    }
    Ok(bytes)
}

fn write_verified_installer(version: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let sanitized_version: String = version
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '-')
        .collect();
    // Prefixo `pulse-` na pasta temporária: `disk_cleanup::is_pulse_owned` preserva
    // entradas `pulse-*` na raiz de `%TEMP%`.
    let dir = env::temp_dir().join("pulse-self-update");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("Não foi possível preparar a pasta de atualização: {error}"))?;
    let file_name = format!("Pulse_{sanitized_version}_x64-setup.exe");
    let final_path = dir.join(file_name);
    let temp_path = final_path.with_extension("exe.part");
    fs::write(&temp_path, bytes)
        .and_then(|()| fs::rename(&temp_path, &final_path))
        .map_err(|error| format!("Não foi possível gravar o instalador verificado: {error}"))?;
    Ok(final_path)
}

/// Script PowerShell estático: todas as variáveis vêm do ambiente do processo
/// (`$env:PULSE_UPDATE_*`), sem qualquer interpolação de strings.
const POWERSHELL_UPDATE_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$pidToWait = [int]$env:PULSE_UPDATE_PID
if ($pidToWait -gt 0) {
  $proc = Get-Process -Id $pidToWait -ErrorAction SilentlyContinue
  if ($proc) { $proc.WaitForExit(15000) | Out-Null }
}
Start-Sleep -Milliseconds 400
if ($env:PULSE_UPDATE_INSTALLER -and (Test-Path -LiteralPath $env:PULSE_UPDATE_INSTALLER)) {
  Start-Process -FilePath $env:PULSE_UPDATE_INSTALLER -ArgumentList '/S' -Wait -WindowStyle Hidden
  Remove-Item -LiteralPath $env:PULSE_UPDATE_INSTALLER -Force -ErrorAction SilentlyContinue
}
$candidates = @(
  $env:PULSE_UPDATE_EXE,
  (Join-Path $env:LOCALAPPDATA 'Pulse\pulse-windows.exe'),
  (Join-Path $env:LOCALAPPDATA 'Pulse\Pulse.exe')
)
foreach ($exe in $candidates) {
  if ($exe -and (Test-Path -LiteralPath $exe)) {
    if ($env:PULSE_UPDATE_MINIMIZED -eq '1') {
      Start-Process -FilePath $exe -ArgumentList @('--updated-relaunch', '--minimized-to-tray')
    } else {
      Start-Process -FilePath $exe -ArgumentList @('--updated-relaunch')
    }
    break
  }
}
"#;

fn spawn_silent_installer_and_exit(app: &AppHandle, installer_path: &Path) -> Result<(), String> {
    let current_exe = env::current_exe()
        .map_err(|error| format!("Não foi possível identificar o executável atual: {error}"))?;
    let main_visible = app
        .get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    let relaunch_minimized = !main_visible;

    crate::process::hidden_command("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            POWERSHELL_UPDATE_SCRIPT,
        ])
        .env("PULSE_UPDATE_PID", std::process::id().to_string())
        .env("PULSE_UPDATE_INSTALLER", installer_path.as_os_str())
        .env("PULSE_UPDATE_EXE", current_exe.as_os_str())
        .env(
            "PULSE_UPDATE_MINIMIZED",
            if relaunch_minimized { "1" } else { "0" },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Não foi possível iniciar o instalador silencioso: {error}"))?;

    std::thread::sleep(Duration::from_millis(150));
    app.exit(0);
    Ok(())
}

fn perform_check_and_maybe_install(
    app: &AppHandle,
    install_if_available: bool,
    is_auto_startup: bool,
) -> Result<SelfUpdateStatus, String> {
    let Some(_guard) = RunGuard::acquire() else {
        return Ok(get_self_update_status());
    };

    let started = Instant::now();
    let mut config = load_config();
    let curr = current_version().to_string();

    update_and_emit(
        Some(app),
        SelfUpdateStatus {
            enabled: config.enabled,
            current_version: curr.clone(),
            phase: "checking".to_string(),
            available_version: None,
            release_size_formatted: None,
            last_checked_at_unix_ms: config.last_checked_at_unix_ms,
            last_updated_to_version: config.last_updated_to_version.clone(),
            last_updated_from_version: config.last_updated_from_version.clone(),
            last_updated_at_unix_ms: config.last_updated_at_unix_ms,
            message: Some("Verificando se há uma nova versão do Pulse…".to_string()),
            error: None,
        },
    );

    let manifest = match fetch_manifest() {
        Ok(manifest) => manifest,
        Err(error) => {
            crate::debug::record(
                "self_update_check",
                started.elapsed().as_millis(),
                Err(&error),
            );
            config.last_checked_at_unix_ms = Some(now_ms());
            config.last_error = Some(error.clone());
            let _ = save_config(&config);
            let status = update_and_emit(
                Some(app),
                SelfUpdateStatus {
                    enabled: config.enabled,
                    current_version: curr,
                    phase: "error".to_string(),
                    available_version: None,
                    release_size_formatted: None,
                    last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                    last_updated_to_version: config.last_updated_to_version,
                    last_updated_from_version: config.last_updated_from_version,
                    last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                    message: None,
                    error: Some(error.clone()),
                },
            );
            return if is_auto_startup {
                Ok(status)
            } else {
                Err(error)
            };
        }
    };

    let checked_at = now_ms();
    config.last_checked_at_unix_ms = Some(checked_at);
    config.last_error = None;
    let _ = save_config(&config);

    let remote_version = manifest.version.trim().trim_start_matches(['v', 'V']).to_string();
    let size_formatted = manifest.installer.size_formatted.clone();

    if !is_newer_version(&curr, &remote_version) {
        crate::debug::record_message(
            "self_update_check",
            started.elapsed().as_millis(),
            "ok",
            &format!("Pulse {curr} já está na versão mais recente"),
        );
        return Ok(update_and_emit(
            Some(app),
            SelfUpdateStatus {
                enabled: config.enabled,
                current_version: curr,
                phase: "up_to_date".to_string(),
                available_version: None,
                release_size_formatted: size_formatted,
                last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                last_updated_to_version: config.last_updated_to_version,
                last_updated_from_version: config.last_updated_from_version,
                last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                message: Some("O Pulse está atualizado.".to_string()),
                error: None,
            },
        ));
    }

    if is_auto_startup && should_skip_auto_attempt(&config, &remote_version, checked_at) {
        let msg = format!(
            "A versão {remote_version} já foi tentada automaticamente há pouco; aguardando confirmação manual."
        );
        crate::debug::record_message(
            "self_update_check",
            started.elapsed().as_millis(),
            "ok",
            &msg,
        );
        return Ok(update_and_emit(
            Some(app),
            SelfUpdateStatus {
                enabled: config.enabled,
                current_version: curr,
                phase: "available".to_string(),
                available_version: Some(remote_version),
                release_size_formatted: size_formatted,
                last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                last_updated_to_version: config.last_updated_to_version,
                last_updated_from_version: config.last_updated_from_version,
                last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                message: Some(msg),
                error: None,
            },
        ));
    }

    if !install_if_available {
        return Ok(update_and_emit(
            Some(app),
            SelfUpdateStatus {
                enabled: config.enabled,
                current_version: curr,
                phase: "available".to_string(),
                available_version: Some(remote_version.clone()),
                release_size_formatted: size_formatted,
                last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                last_updated_to_version: config.last_updated_to_version,
                last_updated_from_version: config.last_updated_from_version,
                last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                message: Some(format!("Nova versão {remote_version} disponível.")),
                error: None,
            },
        ));
    }

    // Valida URL e hash do manifesto antes de iniciar o download.
    let download_url = select_and_validate_installer_url(&manifest.installer)?;
    let expected_sha256 = validate_sha256_hex(&manifest.installer.sha256)?;

    update_and_emit(
        Some(app),
        SelfUpdateStatus {
            enabled: config.enabled,
            current_version: curr.clone(),
            phase: "downloading".to_string(),
            available_version: Some(remote_version.clone()),
            release_size_formatted: size_formatted.clone(),
            last_checked_at_unix_ms: config.last_checked_at_unix_ms,
            last_updated_to_version: config.last_updated_to_version.clone(),
            last_updated_from_version: config.last_updated_from_version.clone(),
            last_updated_at_unix_ms: config.last_updated_at_unix_ms,
            message: Some(format!("Baixando Pulse {remote_version}…")),
            error: None,
        },
    );

    let bytes = match download_installer_bytes(&download_url) {
        Ok(bytes) => bytes,
        Err(error) => {
            crate::debug::record(
                "self_update_download",
                started.elapsed().as_millis(),
                Err(&error),
            );
            config.last_error = Some(error.clone());
            let _ = save_config(&config);
            update_and_emit(
                Some(app),
                SelfUpdateStatus {
                    enabled: config.enabled,
                    current_version: curr,
                    phase: "error".to_string(),
                    available_version: Some(remote_version),
                    release_size_formatted: size_formatted,
                    last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                    last_updated_to_version: config.last_updated_to_version,
                    last_updated_from_version: config.last_updated_from_version,
                    last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                    message: None,
                    error: Some(error.clone()),
                },
            );
            return Err(error);
        }
    };

    update_and_emit(
        Some(app),
        SelfUpdateStatus {
            enabled: config.enabled,
            current_version: curr.clone(),
            phase: "verifying".to_string(),
            available_version: Some(remote_version.clone()),
            release_size_formatted: size_formatted.clone(),
            last_checked_at_unix_ms: config.last_checked_at_unix_ms,
            last_updated_to_version: config.last_updated_to_version.clone(),
            last_updated_from_version: config.last_updated_from_version.clone(),
            last_updated_at_unix_ms: config.last_updated_at_unix_ms,
            message: Some("Verificando integridade SHA-256 do instalador…".to_string()),
            error: None,
        },
    );

    let actual_sha256 = sha256_hex(&bytes);
    if actual_sha256 != expected_sha256 {
        let error =
            "O SHA-256 do instalador baixado não confere com o manifesto oficial; atualização abortada."
                .to_string();
        crate::debug::record(
            "self_update_verify",
            started.elapsed().as_millis(),
            Err(&error),
        );
        config.last_error = Some(error.clone());
        let _ = save_config(&config);
        update_and_emit(
            Some(app),
            SelfUpdateStatus {
                enabled: config.enabled,
                current_version: curr,
                phase: "error".to_string(),
                available_version: Some(remote_version),
                release_size_formatted: size_formatted,
                last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                last_updated_to_version: config.last_updated_to_version,
                last_updated_from_version: config.last_updated_from_version,
                last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                message: None,
                error: Some(error.clone()),
            },
        );
        return Err(error);
    }

    let installer_path = write_verified_installer(&remote_version, &bytes)?;

    config.last_attempted_version = Some(remote_version.clone());
    config.last_attempt_at_unix_ms = Some(now_ms());
    config.last_updated_from_version = Some(curr.clone());
    config.last_error = None;
    let _ = save_config(&config);

    let installing_status = update_and_emit(
        Some(app),
        SelfUpdateStatus {
            enabled: config.enabled,
            current_version: curr,
            phase: "installing".to_string(),
            available_version: Some(remote_version.clone()),
            release_size_formatted: size_formatted,
            last_checked_at_unix_ms: config.last_checked_at_unix_ms,
            last_updated_to_version: config.last_updated_to_version,
            last_updated_from_version: config.last_updated_from_version,
            last_updated_at_unix_ms: config.last_updated_at_unix_ms,
            message: Some(format!(
                "Instalando Pulse {remote_version} silenciosamente e reiniciando…"
            )),
            error: None,
        },
    );

    crate::debug::record_message(
        "self_update_install",
        started.elapsed().as_millis(),
        "ok",
        &format!("SHA-256 validado ({actual_sha256}); iniciando instalador silencioso para v{remote_version}"),
    );

    spawn_silent_installer_and_exit(app, &installer_path)?;
    Ok(installing_status)
}

#[tauri::command]
pub fn get_self_update_status() -> SelfUpdateStatus {
    let config = load_config();
    if let Ok(mut guard) = status_store().lock() {
        guard.enabled = config.enabled;
        guard.last_checked_at_unix_ms = config.last_checked_at_unix_ms;
        guard.last_updated_to_version = config.last_updated_to_version;
        guard.last_updated_from_version = config.last_updated_from_version;
        guard.last_updated_at_unix_ms = config.last_updated_at_unix_ms;
        return guard.clone();
    }
    initial_status_from_config(&config)
}

#[tauri::command]
pub async fn set_auto_self_update(
    enabled: bool,
    app: AppHandle,
) -> Result<SelfUpdateStatus, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        set_auto_self_update_sync(enabled, Some(&app_for_task))
    })
    .await
    .map_err(|error| format!("Não foi possível atualizar a preferência: {error}"))?
}

pub fn set_auto_self_update_sync(
    enabled: bool,
    app: Option<&AppHandle>,
) -> Result<SelfUpdateStatus, String> {
    let mut config = load_config();
    config.enabled = enabled;
    if !enabled {
        config.last_error = None;
    }
    save_config(&config)?;
    crate::debug::record_message(
        "self_update_preference",
        0,
        "ok",
        if enabled {
            "Atualização automática ao iniciar ativada"
        } else {
            "Atualização automática ao iniciar desativada"
        },
    );

    let mut current = get_self_update_status();
    current.enabled = enabled;
    if !enabled && current.phase == "error" {
        current.phase = "idle".to_string();
        current.error = None;
    }
    let updated = update_and_emit(app, current);
    if let Some(handle) = app {
        crate::tray::update_tray_menu(handle);
    }
    Ok(updated)
}

#[tauri::command]
pub async fn check_self_update(
    install_if_available: Option<bool>,
    app: AppHandle,
) -> Result<SelfUpdateStatus, String> {
    let install = install_if_available.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        perform_check_and_maybe_install(&app, install, false)
    })
    .await
    .map_err(|error| format!("A verificação de atualização foi interrompida: {error}"))?
}

/// Inicializa o verificador de atualização na partida do aplicativo.
pub fn init(app: AppHandle) {
    let (config, just_updated) =
        reconcile_startup_config(load_config(), current_version(), now_ms());
    if just_updated {
        let _ = save_config(&config);
        crate::debug::record_message(
            "self_update_applied",
            0,
            "ok",
            &format!(
                "Pulse iniciado na versão {} após atualização automática",
                current_version()
            ),
        );
    }
    update_and_emit(Some(&app), initial_status_from_config(&config));

    if cfg!(debug_assertions) {
        return;
    }

    let spawned = std::thread::Builder::new()
        .name("pulse-self-update".into())
        .spawn(move || {
            std::thread::sleep(STARTUP_CHECK_DELAY);
            if !load_config().enabled {
                return;
            }
            let _ = perform_check_and_maybe_install(&app, true, true);
        });

    if let Err(error) = spawned {
        eprintln!("Pulse: verificador de atualização ao iniciar indisponível: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn compares_semantic_versions_accurately() {
        assert!(is_newer_version("1.4.0", "1.4.1"));
        assert!(is_newer_version("1.4.0", "1.5.0"));
        assert!(is_newer_version("1.4.0", "v2.0.0"));
        assert!(!is_newer_version("1.4.0", "1.4.0"));
        assert!(!is_newer_version("1.4.0", "1.3.9"));
        assert!(!is_newer_version("1.4.0", "invalida"));
    }

    #[test]
    fn validates_installer_urls_against_official_whitelist() {
        let valid_github = ReleaseInstaller {
            filename: Some("Pulse-latest-x64-setup.exe".into()),
            url: Some("https://pulse.lagrimante.com.br/download/Pulse-latest-x64-setup.exe".into()),
            asset_url: Some(
                "https://github.com/LucasLagrimante/pulse/releases/download/v1.5.0/Pulse_1.5.0_x64-setup.exe"
                    .into(),
            ),
            size_bytes: Some(3_800_000),
            size_formatted: Some("3,6 MB".into()),
            sha256: "420d69613d72db6e6059d226aa297d4af9fa29e410b357b5f98e51796f2f72bf".into(),
        };
        assert_eq!(
            select_and_validate_installer_url(&valid_github).unwrap(),
            "https://github.com/LucasLagrimante/pulse/releases/download/v1.5.0/Pulse_1.5.0_x64-setup.exe"
        );

        let untrusted = ReleaseInstaller {
            asset_url: Some("https://malicious.example.com/Pulse_1.5.0_x64-setup.exe".into()),
            url: None,
            ..valid_github.clone()
        };
        assert!(select_and_validate_installer_url(&untrusted).is_err());

        let non_exe = ReleaseInstaller {
            asset_url: Some(
                "https://github.com/LucasLagrimante/pulse/releases/download/v1.5.0/script.bat"
                    .into(),
            ),
            url: None,
            ..valid_github
        };
        assert!(select_and_validate_installer_url(&non_exe).is_err());
    }

    #[test]
    fn validates_sha256_hex_format() {
        let valid = "420D69613D72DB6E6059D226AA297D4AF9FA29E410B357B5F98E51796F2F72BF";
        assert_eq!(
            validate_sha256_hex(valid).unwrap(),
            "420d69613d72db6e6059d226aa297d4af9fa29e410b357b5f98e51796f2f72bf"
        );
        assert!(validate_sha256_hex("curto").is_err());
        assert!(validate_sha256_hex(
            "zzzz69613d72db6e6059d226aa297d4af9fa29e410b357b5f98e51796f2f72bf"
        )
        .is_err());
    }

    #[test]
    fn default_config_is_enabled_and_persists_roundtrip() {
        let dir = env::temp_dir().join(format!("pulse-self-update-test-{}", std::process::id()));
        let path = config_file_in(&dir);
        let _ = fs::remove_dir_all(&dir);

        let default_cfg = load_config_from(&path);
        assert!(default_cfg.enabled, "auto-update deve vir ativado por padrão");

        let custom = SelfUpdateConfig {
            enabled: false,
            last_checked_at_unix_ms: Some(1_700_000_000_000),
            ..SelfUpdateConfig::default()
        };
        save_config_to(&path, &custom).expect("salvar config");
        let loaded = load_config_from(&path);
        assert!(!loaded.enabled);
        assert_eq!(loaded.last_checked_at_unix_ms, Some(1_700_000_000_000));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconciles_completed_update_and_prevents_retry_loops() {
        let cfg = SelfUpdateConfig {
            enabled: true,
            last_attempted_version: Some("1.5.0".into()),
            last_attempt_at_unix_ms: Some(1_000_000),
            last_updated_from_version: Some("1.4.0".into()),
            ..SelfUpdateConfig::default()
        };

        // Se ainda estamos na 1.4.0 logo após tentar a 1.5.0, bloqueia repetição automática.
        assert!(should_skip_auto_attempt(&cfg, "1.5.0", 1_000_000 + 60_000));
        // Passado o cooldown de 6h, permite tentar novamente.
        assert!(!should_skip_auto_attempt(
            &cfg,
            "1.5.0",
            1_000_000 + RETRY_COOLDOWN_MS + 1
        ));

        // Se o processo subiu na 1.5.0, reconcilia como atualizado com sucesso.
        let (reconciled, changed) = reconcile_startup_config(cfg, "1.5.0", 2_000_000);
        assert!(changed);
        assert_eq!(reconciled.last_updated_to_version.as_deref(), Some("1.5.0"));
        assert!(reconciled.last_attempted_version.is_none());
    }
}
