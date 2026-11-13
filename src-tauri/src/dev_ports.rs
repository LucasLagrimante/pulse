//! Inspetor de portas TCP em escuta e projetos de desenvolvimento ativos.
//!
//! Coleta sockets em estado `LISTEN` no Windows (API nativa `GetExtendedTcpTable`
//! junto com leitura de CWD/CommandLine via PEB) e nas distros WSL2 já em
//! execução (`wsl.exe -l --running -q`, sem nunca ligar a VM quando parada).
//! Infere runtime/framework e pasta do projeto para permitir abrir no navegador,
//! copiar a URL ou encerrar o processo diretamente pelo tray.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CACHE_TTL: Duration = Duration::from_millis(2500);

/// Portas nativas do Windows que nunca representam servidores de desenvolvimento.
const WINDOWS_SYSTEM_PORTS: &[u16] = &[135, 139, 445, 5040, 5357, 7680];

/// Runtimes de desenvolvimento que continuam relevantes mesmo em portas altas
/// da faixa efêmera IANA (`49152..=65535`).
const DEV_RUNTIME_BINARIES: &[&str] = &[
    "node",
    "bun",
    "deno",
    "python",
    "python3",
    "ruby",
    "php",
    "go",
    "cargo",
    "dotnet",
    "java",
    "javaw",
    "workerd",
    "wrangler",
    "cloudflared",
    "caddy",
    "nginx",
    "postgres",
    "mysqld",
    "redis-server",
    "mongod",
    "uvicorn",
    "gunicorn",
    "ollama",
    "vite",
    "tsx",
    "ts-node",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevPortEntry {
    pub id: String,
    pub port: u16,
    pub bind_address: String,
    pub url: String,
    pub pid: u32,
    pub process_name: String,
    pub runtime: String,
    pub project_name: String,
    pub project_path: Option<String>,
    pub command_line: Option<String>,
    /// `"windows"` ou `"wsl"`
    pub environment: String,
    pub wsl_distro: Option<String>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DevPortsInventory {
    pub ports: Vec<DevPortEntry>,
    pub sampled_at_unix_ms: u64,
    pub unavailable_reason: Option<String>,
}

struct DevPortsCache {
    inventory: DevPortsInventory,
    last_fetched: Instant,
}

static CACHE: OnceLock<Mutex<Option<DevPortsCache>>> = OnceLock::new();

fn cache_lock() -> &'static Mutex<Option<DevPortsCache>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Último snapshot em cache (usado pelo construtor do menu nativo do tray para
/// não bloquear a thread de UI em aberturas de menu).
pub fn cached_dev_ports_snapshot() -> DevPortsInventory {
    if let Ok(guard) = cache_lock().lock() {
        if let Some(cached) = guard.as_ref() {
            return cached.inventory.clone();
        }
    }
    DevPortsInventory::default()
}

fn entry_signature(inventory: &DevPortsInventory) -> Vec<(String, u16, u32)> {
    inventory
        .ports
        .iter()
        .map(|entry| (entry.id.clone(), entry.port, entry.pid))
        .collect()
}

fn invalidate_cache() {
    if let Ok(mut guard) = cache_lock().lock() {
        *guard = None;
    }
}

pub fn collect_dev_ports_sync(force_refresh: bool) -> Result<DevPortsInventory, String> {
    if !force_refresh {
        if let Ok(guard) = cache_lock().lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.last_fetched.elapsed() < CACHE_TTL {
                    return Ok(cached.inventory.clone());
                }
            }
        }
    }

    let started = Instant::now();
    let result = collect_dev_ports_uncached();
    let elapsed = started.elapsed().as_millis();

    match result {
        Ok(inventory) => {
            if let Ok(mut guard) = cache_lock().lock() {
                *guard = Some(DevPortsCache {
                    inventory: inventory.clone(),
                    last_fetched: Instant::now(),
                });
            }
            crate::debug::record("dev_ports", elapsed, Ok(()));
            Ok(inventory)
        }
        Err(failure) => {
            let msg = failure.message.clone();
            crate::debug::record_failure("dev_ports", elapsed, failure);
            Err(msg)
        }
    }
}

fn collect_dev_ports_uncached() -> Result<DevPortsInventory, crate::debug::Failure> {
    #[cfg(windows)]
    {
        let win_listeners = win32::list_tcp_listeners().map_err(|detail| {
            crate::debug::Failure::new("Não foi possível consultar a tabela TCP do Windows")
                .stage("iphlpapi")
                .detail(detail)
        })?;

        let wsl_entries = collect_wsl_entries();
        let wsl_ports: HashSet<u16> = wsl_entries.iter().map(|entry| entry.port).collect();

        let mut combined = build_windows_entries(win_listeners, &wsl_ports);
        combined.extend(wsl_entries);
        finalize_entries(&mut combined);

        Ok(DevPortsInventory {
            ports: combined,
            sampled_at_unix_ms: now_unix_ms(),
            unavailable_reason: None,
        })
    }

    #[cfg(not(windows))]
    {
        Ok(DevPortsInventory {
            ports: Vec::new(),
            sampled_at_unix_ms: now_unix_ms(),
            unavailable_reason: Some(
                "A inspeção nativa de portas do tray é suportada apenas no Windows 11.".to_owned(),
            ),
        })
    }
}

/// Atualiza a lista em segundo plano e sincroniza o menu nativo do tray.
pub fn refresh_and_update_tray<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    std::thread::spawn(move || {
        if collect_dev_ports_sync(true).is_ok() {
            crate::tray::update_tray_menu(&app);
            if let Some(flyout) = app.get_webview_window("tray-flyout") {
                use tauri::Emitter;
                let _ = flyout.emit("pulse://dev-ports-changed", ());
            }
        }
    });
}

use tauri::Manager;

#[tauri::command]
pub async fn get_dev_ports(
    app: tauri::AppHandle,
    force_refresh: Option<bool>,
) -> Result<DevPortsInventory, String> {
    let previous = cached_dev_ports_snapshot();
    let force = force_refresh.unwrap_or(false);
    let inventory = tauri::async_runtime::spawn_blocking(move || collect_dev_ports_sync(force))
        .await
        .map_err(|e| e.to_string())??;

    if entry_signature(&previous) != entry_signature(&inventory) {
        crate::tray::update_tray_menu(&app);
    }

    Ok(inventory)
}

#[tauri::command]
pub async fn kill_dev_port(
    app: tauri::AppHandle,
    port: u16,
    pid: u32,
    environment: String,
    wsl_distro: Option<String>,
) -> Result<DevPortsInventory, String> {
    let started = Instant::now();
    let env_clone = environment.clone();
    let distro_clone = wsl_distro.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        kill_dev_port_sync(port, pid, &env_clone, distro_clone.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?;

    crate::debug::record_result(
        "kill_dev_port",
        started.elapsed().as_millis(),
        &result,
        "Não foi possível encerrar o processo na porta",
    );

    if result.is_ok() {
        crate::tray::update_tray_menu(&app);
        if let Some(flyout) = app.get_webview_window("tray-flyout") {
            use tauri::Emitter;
            let _ = flyout.emit("pulse://dev-ports-changed", ());
        }
    }

    result
}

pub fn kill_dev_port_sync(
    port: u16,
    pid: u32,
    environment: &str,
    wsl_distro: Option<&str>,
) -> Result<DevPortsInventory, String> {
    if pid == 0 || pid == std::process::id() {
        return Err("O Pulse não pode encerrar este processo.".to_owned());
    }

    let current = collect_dev_ports_sync(true)?;
    let target = current
        .ports
        .iter()
        .find(|entry| entry.port == port && entry.pid == pid && entry.environment == environment)
        .cloned()
        .ok_or_else(|| {
            format!("Nenhum processo ativo com PID {pid} está escutando na porta {port}.")
        })?;

    #[cfg(windows)]
    {
        if target.environment == "wsl" {
            let distro = wsl_distro
                .or(target.wsl_distro.as_deref())
                .filter(|d| !d.trim().is_empty())
                .ok_or_else(|| {
                    "Distribuição WSL não identificada para encerrar o PID.".to_owned()
                })?;

            let pid_str = pid.to_string();
            let graceful = crate::process::hidden_command("wsl.exe")
                .args(["-d", distro, "-e", "kill", "-15", &pid_str])
                .output()
                .map_err(|e| format!("Falha ao solicitar encerramento no WSL ({distro}): {e}"))?;

            std::thread::sleep(Duration::from_millis(250));
            let after_graceful = collect_dev_ports_sync(true)?;
            let still_alive = after_graceful
                .ports
                .iter()
                .any(|entry| entry.port == port && entry.pid == pid && entry.environment == "wsl");

            if still_alive {
                let forced = crate::process::hidden_command("wsl.exe")
                    .args(["-d", distro, "-e", "kill", "-9", &pid_str])
                    .output()
                    .map_err(|e| format!("Falha ao forçar encerramento no WSL ({distro}): {e}"))?;
                if !forced.status.success() && !graceful.status.success() {
                    let detail = String::from_utf8_lossy(&forced.stderr).trim().to_owned();
                    return Err(if detail.is_empty() {
                        format!("O WSL recusou encerrar o PID {pid}.")
                    } else {
                        detail
                    });
                }
                std::thread::sleep(Duration::from_millis(120));
            }
        } else {
            let pid_str = pid.to_string();
            let graceful = crate::process::hidden_command("taskkill.exe")
                .args(["/PID", &pid_str, "/T"])
                .output()
                .map_err(|e| format!("Não foi possível executar taskkill: {e}"))?;

            if !graceful.status.success() {
                let forced = crate::process::hidden_command("taskkill.exe")
                    .args(["/F", "/PID", &pid_str, "/T"])
                    .output()
                    .map_err(|e| format!("Não foi possível forçar o encerramento: {e}"))?;

                if !forced.status.success() {
                    let detail = String::from_utf8_lossy(&forced.stderr).trim().to_owned();
                    return Err(if detail.is_empty() {
                        format!(
                            "O Windows não permitiu encerrar {} (PID {pid}).",
                            target.process_name
                        )
                    } else {
                        detail
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }

        invalidate_cache();
        collect_dev_ports_sync(true)
    }

    #[cfg(not(windows))]
    {
        let _ = (wsl_distro, target);
        Err(format!(
            "Encerramento de portas só está disponível no Windows (porta {port}, PID {pid})."
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawTcpListener {
    pid: u32,
    port: u16,
    bind_address: String,
    is_ipv4: bool,
}

/// Formata a URL navegável a partir do endereço de bind e da porta.
fn format_listen_url(bind_address: &str, port: u16) -> String {
    let trimmed = bind_address.trim();
    if trimmed.is_empty()
        || trimmed == "0.0.0.0"
        || trimmed == "127.0.0.1"
        || trimmed == "::"
        || trimmed == "::1"
        || trimmed == "[::]"
        || trimmed == "[::1]"
        || trimmed == "*"
    {
        format!("http://localhost:{port}")
    } else if trimmed.contains(':') && !trimmed.starts_with('[') {
        format!("http://[{trimmed}]:{port}")
    } else {
        format!("http://{trimmed}:{port}")
    }
}

fn clean_process_stem(process_name: &str) -> String {
    process_name
        .trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .to_ascii_lowercase()
}

fn is_dev_runtime_binary(process_name: &str) -> bool {
    let stem = clean_process_stem(process_name);
    DEV_RUNTIME_BINARIES
        .iter()
        .any(|candidate| stem == *candidate)
}

fn is_dev_build_binary(exe_path: Option<&str>) -> bool {
    let Some(path) = exe_path else {
        return false;
    };
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    lower.contains("/target/debug/") || lower.contains("/target/release/")
}

fn is_windows_noise_process(process_name: &str, exe_path: Option<&str>, port: u16) -> bool {
    if WINDOWS_SYSTEM_PORTS.contains(&port) {
        return true;
    }
    let stem = clean_process_stem(process_name);
    if stem.is_empty()
        || matches!(
            stem.as_str(),
            "system"
                | "idle"
                | "svchost"
                | "lsass"
                | "services"
                | "wininit"
                | "spoolsv"
                | "dashost"
                | "searchindexer"
                | "msmpeng"
                | "nissrv"
                | "pulse-windows"
                | "pulse"
        )
    {
        return true;
    }

    if let Some(path) = exe_path {
        let lower = path.replace('\\', "/").to_ascii_lowercase();
        if (lower.contains(":/windows/system32/")
            || lower.contains(":/windows/syswow64/")
            || lower.contains(":/programdata/microsoft/windows defender/"))
            && stem != "wslrelay"
            && !is_dev_runtime_binary(&stem)
        {
            return true;
        }
    }

    // Portas dinâmicas efêmeras (49152..=65535) costumam ser usadas por RPC/IPC de
    // apps desktop comuns (Spotify, Discord, Steam, WebView2). Mantemos apenas
    // se for runtime de dev ou binário compilado localmente.
    if port >= 49152 && !is_dev_runtime_binary(&stem) && !is_dev_build_binary(exe_path) {
        return true;
    }

    false
}

fn is_wsl_noise_process(comm: &str, command_line: Option<&str>, port: u16) -> bool {
    if WINDOWS_SYSTEM_PORTS.contains(&port) {
        return true;
    }
    let lower = comm.trim().to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "systemd-resolve"
            | "systemd-resolved"
            | "systemd-journal"
            | "dbus-daemon"
            | "rpcbind"
            | "dhclient"
            | "wsl-pro-service"
    ) {
        return true;
    }
    if port >= 49152 && !is_dev_runtime_binary(&lower) && !is_dev_build_binary(command_line) {
        // Editores/agentes no WSL (ex.: node / vscode server) ainda passam se `comm`
        // for node, mas processos IPC internos desconhecidos em portas altas são filtrados.
        let cmd_lower = command_line.unwrap_or("").to_ascii_lowercase();
        let looks_like_server = cmd_lower.contains("serve")
            || cmd_lower.contains("dev")
            || cmd_lower.contains("listen")
            || cmd_lower.contains("uvicorn")
            || cmd_lower.contains("vite")
            || cmd_lower.contains("next")
            || cmd_lower.contains("wrangler");
        if !looks_like_server {
            return true;
        }
    }
    false
}

/// Deduz o runtime/framework principal a partir do processo, linha de comando e caminho.
pub fn infer_runtime(
    process_name: &str,
    command_line: Option<&str>,
    exe_path: Option<&str>,
) -> String {
    let cmd = command_line.unwrap_or("").to_ascii_lowercase();
    let exe = exe_path
        .unwrap_or("")
        .replace('\\', "/")
        .to_ascii_lowercase();
    let stem = clean_process_stem(process_name);

    let rules: &[(&[&str], &str)] = &[
        (&["vite", "vitest"], "Vite"),
        (
            &[
                "next-server",
                "next dev",
                "next start",
                "/.next/",
                "\\.next\\",
                "node_modules/next",
                "node_modules\\next",
            ],
            "Next.js",
        ),
        (&["wrangler", "workerd"], "Wrangler"),
        (&["astro"], "Astro"),
        (&["nuxt", "nuxi"], "Nuxt"),
        (&["remix"], "Remix"),
        (&["@angular/cli", "ng serve"], "Angular"),
        (&["svelte-kit", "sveltekit"], "SvelteKit"),
        (&["webpack-dev-server", "webpack"], "Webpack"),
        (&["turbopack", "turbo "], "Turbopack"),
        (&["playwright"], "Playwright"),
        (&["uvicorn"], "Uvicorn"),
        (&["fastapi"], "FastAPI"),
        (&["gunicorn"], "Gunicorn"),
        (&["flask"], "Flask"),
        (&["manage.py runserver", "django"], "Django"),
        (&["docker", "com.docker"], "Docker"),
        (&["postgres"], "PostgreSQL"),
        (&["mysqld", "mariadb"], "MySQL"),
        (&["redis-server", "valkey"], "Redis"),
        (&["mongod"], "MongoDB"),
        (&["ollama"], "Ollama"),
        (&["cloudflared"], "Cloudflared"),
        (&["caddy"], "Caddy"),
        (&["nginx"], "Nginx"),
    ];

    for (patterns, label) in rules {
        if patterns.iter().any(|p| cmd.contains(p) || stem.contains(p)) {
            return (*label).to_owned();
        }
    }

    if exe.contains("/target/debug/")
        || exe.contains("/target/release/")
        || cmd.contains("/target/debug/")
        || cmd.contains("/target/release/")
        || stem == "cargo"
    {
        return "Rust".to_owned();
    }

    match stem.as_str() {
        "node" | "nodejs" => "Node.js".to_owned(),
        "bun" => "Bun".to_owned(),
        "deno" => "Deno".to_owned(),
        "python" | "python3" | "py" => "Python".to_owned(),
        "ruby" => "Ruby".to_owned(),
        "php" => "PHP".to_owned(),
        "go" => "Go".to_owned(),
        "dotnet" => ".NET".to_owned(),
        "java" | "javaw" => "Java".to_owned(),
        "wslrelay" => "WSL Relay".to_owned(),
        _ => {
            let trimmed = process_name.trim().trim_end_matches(".exe");
            if trimmed.is_empty() {
                "Servidor".to_owned()
            } else {
                trimmed.to_owned()
            }
        }
    }
}

fn is_generic_dir(path: &str) -> bool {
    let norm = path
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    if norm.is_empty() || norm == "/" || norm == "/root" || norm == "/home" {
        return true;
    }
    // Ex.: "c:" ou "c:/windows/system32" ou "/home/usuario"
    if norm.len() <= 3 && norm.ends_with(':') {
        return true;
    }
    if norm.contains("/windows/system32")
        || norm.contains("/windows/syswow64")
        || norm.starts_with("/usr/")
        || norm == "/usr"
        || norm == "/etc"
        || norm == "/var"
        || norm == "/tmp"
    {
        return true;
    }
    false
}

/// Remove subpastas internas de build/dependências para obter a raiz do projeto.
fn normalize_project_dir(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return None;
    }
    let sep = if trimmed.contains('\\') { '\\' } else { '/' };
    let normalized = trimmed.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();

    const MARKERS: &[&str] = &[
        "/node_modules/",
        "/node_modules",
        "/.next/",
        "/.next",
        "/target/debug/",
        "/target/release/",
        "/target/debug",
        "/target/release",
        "/.venv/",
        "/venv/",
        "/dist/",
        "/build/",
        "/.turbo/",
    ];

    let mut cut = normalized.as_str();
    for marker in MARKERS {
        if let Some(idx) = lower.find(marker) {
            if idx > 0 {
                cut = &normalized[..idx];
                break;
            }
        }
    }

    let clean = cut.trim_end_matches('/');
    if is_generic_dir(clean) {
        return None;
    }

    let restored = if sep == '\\' {
        clean.replace('/', "\\")
    } else {
        clean.to_owned()
    };
    Some(restored)
}

fn dir_basename(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/");
    let name = norm
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    if name.ends_with(':') {
        return None;
    }
    Some(name.to_owned())
}

fn extract_project_dir_from_command_line(command_line: &str) -> Option<String> {
    let tokens = split_command_tokens(command_line);
    for token in tokens.into_iter().skip(1) {
        let trimmed = token.trim_matches('"').trim_matches('\'');
        if trimmed.starts_with('-') || trimmed.len() < 4 {
            continue;
        }
        let looks_like_abs =
            trimmed.starts_with('/') || (trimmed.len() > 3 && trimmed.as_bytes()[1] == b':');
        if !looks_like_abs {
            continue;
        }
        if let Some(dir) = normalize_project_dir(trimmed) {
            // Se apontava para um arquivo (ex: /home/user/app/server.py), sobe para a pasta.
            let norm = dir.replace('\\', "/");
            if let Some(last_segment) = norm.rsplit('/').next() {
                if last_segment.contains('.') && !last_segment.starts_with('.') {
                    if let Some(parent) = Path::new(&dir).parent() {
                        if let Some(parent_str) = parent.to_str() {
                            if let Some(parent_clean) = normalize_project_dir(parent_str) {
                                return Some(parent_clean);
                            }
                        }
                    }
                }
            }
            return Some(dir);
        }
    }
    None
}

fn split_command_tokens(command_line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in command_line.chars() {
        match quote {
            Some(q) if ch == q => {
                quote = None;
            }
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Deduz o nome do projeto e a pasta raiz a partir de CWD, linha de comando e executável.
pub fn infer_project_info(
    process_name: &str,
    exe_path: Option<&str>,
    cwd: Option<&str>,
    command_line: Option<&str>,
    runtime: &str,
) -> (String, Option<String>) {
    if let Some(cwd_path) = cwd.and_then(normalize_project_dir) {
        if let Some(name) = dir_basename(&cwd_path) {
            return (name, Some(cwd_path));
        }
    }

    if let Some(cmd_dir) = command_line.and_then(extract_project_dir_from_command_line) {
        if let Some(name) = dir_basename(&cmd_dir) {
            return (name, Some(cmd_dir));
        }
    }

    if let Some(exe) = exe_path {
        if is_dev_build_binary(Some(exe)) {
            if let Some(dir) = normalize_project_dir(exe) {
                if let Some(name) = dir_basename(&dir) {
                    return (name, Some(dir));
                }
            }
        }
    }

    let fallback = if !runtime.is_empty() && runtime != "Servidor" {
        runtime.to_owned()
    } else {
        let stem = process_name.trim().trim_end_matches(".exe");
        if stem.is_empty() {
            "Servidor local".to_owned()
        } else {
            stem.to_owned()
        }
    };
    (fallback, None)
}

#[cfg(windows)]
fn build_windows_entries(
    listeners: Vec<RawTcpListener>,
    wsl_ports: &HashSet<u16>,
) -> Vec<DevPortEntry> {
    let mut pid_paths: HashMap<u32, Option<String>> = HashMap::new();
    let mut filtered_listeners = Vec::new();

    for listener in listeners {
        if listener.pid == 0 || listener.pid == 4 {
            continue;
        }
        if wsl_ports.contains(&listener.port) {
            // Se a porta já pertence a um processo real no WSL, o wslrelay do Windows
            // é apenas a ponte transparente — mantemos a entrada rica do WSL.
            let exe_path = pid_paths
                .entry(listener.pid)
                .or_insert_with(|| win32::image_path(listener.pid))
                .as_deref();
            let proc_name = exe_path
                .and_then(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
                .unwrap_or("");
            if clean_process_stem(proc_name) == "wslrelay" {
                continue;
            }
        }

        let exe_path = pid_paths
            .entry(listener.pid)
            .or_insert_with(|| win32::image_path(listener.pid))
            .clone();
        let proc_name = exe_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
            .unwrap_or("processo.exe")
            .to_owned();

        if is_windows_noise_process(&proc_name, exe_path.as_deref(), listener.port) {
            continue;
        }

        filtered_listeners.push((listener, proc_name, exe_path));
    }

    let unique_pids: Vec<u32> = filtered_listeners
        .iter()
        .map(|(l, _, _)| l.pid)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut details: HashMap<u32, (Option<String>, Option<String>)> = HashMap::new();
    let mut missing_cmd_pids = Vec::new();
    for pid in unique_pids {
        let (cwd, cmd) = win32::read_process_peb_strings(pid);
        if cmd.is_none() {
            missing_cmd_pids.push(pid);
        }
        details.insert(pid, (cwd, cmd));
    }

    if !missing_cmd_pids.is_empty() {
        let fallback = win32::batch_query_command_lines(&missing_cmd_pids);
        for (pid, cmd) in fallback {
            if let Some(entry) = details.get_mut(&pid) {
                if entry.1.is_none() {
                    entry.1 = Some(cmd);
                }
            }
        }
    }

    let mut entries = Vec::with_capacity(filtered_listeners.len());
    for (listener, proc_name, exe_path) in filtered_listeners {
        let (cwd, cmd) = details.get(&listener.pid).cloned().unwrap_or((None, None));
        let runtime = infer_runtime(&proc_name, cmd.as_deref(), exe_path.as_deref());
        let (project_name, project_path) = infer_project_info(
            &proc_name,
            exe_path.as_deref(),
            cwd.as_deref(),
            cmd.as_deref(),
            &runtime,
        );
        let icon = exe_path
            .as_deref()
            .and_then(|path| crate::app_icons::icon_for_path(Path::new(path)));

        entries.push(DevPortEntry {
            id: format!("windows:{}:{}", listener.pid, listener.port),
            port: listener.port,
            bind_address: listener.bind_address.clone(),
            url: format_listen_url(&listener.bind_address, listener.port),
            pid: listener.pid,
            process_name: proc_name,
            runtime,
            project_name,
            project_path,
            command_line: cmd,
            environment: "windows".to_owned(),
            wsl_distro: None,
            icon,
        });
    }

    entries
}

/// Remove duplicatas IPv4/IPv6 do mesmo processo/porta (preferindo IPv4) e ordena
/// por porta crescente.
fn finalize_entries(entries: &mut Vec<DevPortEntry>) {
    let mut by_key: HashMap<(String, u32, u16), DevPortEntry> = HashMap::new();
    for entry in entries.drain(..) {
        let key = (entry.environment.clone(), entry.pid, entry.port);
        match by_key.get(&key) {
            Some(existing) => {
                // Prefere IPv4 (`127.0.0.1` / `0.0.0.0`) sobre endereços IPv6.
                let existing_is_v6 = existing.bind_address.contains(':');
                let incoming_is_v6 = entry.bind_address.contains(':');
                if existing_is_v6 && !incoming_is_v6 {
                    by_key.insert(key, entry);
                }
            }
            None => {
                by_key.insert(key, entry);
            }
        }
    }

    *entries = by_key.into_values().collect();
    entries.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then_with(|| a.environment.cmp(&b.environment))
            .then_with(|| a.pid.cmp(&b.pid))
    });
}

#[cfg(windows)]
const WSL_SCAN_SCRIPT: &str = r#"ss -tlnpH 2>/dev/null | while IFS= read -r line; do
  addr=$(printf '%s' "$line" | awk '{print $4}')
  proc=$(printf '%s' "$line" | awk '{print $6}')
  pid=$(printf '%s' "$proc" | sed -n 's/.*pid=\([0-9][0-9]*\).*/\1/p')
  if [ -n "$pid" ] && [ -d "/proc/$pid" ]; then
    comm=$(cat "/proc/$pid/comm" 2>/dev/null | tr '\n\r\t' ' ')
    cwd=$(readlink -f "/proc/$pid/cwd" 2>/dev/null | tr '\n\r\t' ' ')
    cmd=$(tr '\0\n\r\t' '    ' < "/proc/$pid/cmdline" 2>/dev/null | cut -c1-400)
    printf '%s\t%s\t%s\t%s\t%s\n' "$addr" "$pid" "$comm" "$cwd" "$cmd"
  fi
done"#;

#[cfg(windows)]
fn running_wsl_distros() -> Vec<String> {
    let Ok(output) = crate::process::hidden_command("wsl.exe")
        .args(["-l", "--running", "-q"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    // O `wsl.exe -l` emite UTF-16LE no Windows; faz fallback para UTF-8 se necessário.
    let utf16_text = String::from_utf16_lossy(
        &output
            .stdout
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect::<Vec<_>>(),
    );
    let content = if utf16_text.contains('\n') || utf16_text.contains('\r') {
        utf16_text
    } else {
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let mut distros: Vec<String> = content
        .lines()
        .map(|l| {
            l.trim_matches(|c| c == '\u{feff}' || c == '\0')
                .trim()
                .to_owned()
        })
        .filter(|l| !l.is_empty())
        .collect();
    distros.sort_unstable();
    distros.dedup();
    distros
}

#[cfg(windows)]
fn collect_wsl_entries() -> Vec<DevPortEntry> {
    let distros = running_wsl_distros();
    if distros.is_empty() {
        return Vec::new();
    }

    let wsl_icon = crate::app_icons::icon_for_command("wsl.exe");
    let mut entries = Vec::new();
    for distro in distros {
        let Ok(output) = crate::process::hidden_command("wsl.exe")
            .args(["-d", &distro, "-e", "sh", "-lc", WSL_SCAN_SCRIPT])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        for mut entry in parse_wsl_ss_output(&distro, &stdout) {
            entry.icon = wsl_icon.clone();
            entries.push(entry);
        }
    }
    entries
}

/// Faz o parse da saída tabulada de `WSL_SCAN_SCRIPT`: `addr\tpid\tcomm\tcwd\tcmdline`.
pub fn parse_wsl_ss_output(distro: &str, stdout: &str) -> Vec<DevPortEntry> {
    let mut entries = Vec::new();
    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(5, '\t');
        let addr = parts.next().unwrap_or("").trim();
        let pid_str = parts.next().unwrap_or("").trim();
        let comm = parts.next().unwrap_or("").trim();
        let cwd = parts.next().unwrap_or("").trim();
        let cmdline = parts.next().unwrap_or("").trim();

        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == 0 {
            continue;
        }

        let Some((bind_address, port)) = split_host_and_port(addr) else {
            continue;
        };
        let cmd_opt = (!cmdline.is_empty()).then(|| cmdline.to_owned());
        let cwd_opt = (!cwd.is_empty()).then(|| cwd.to_owned());
        let process_name = if comm.is_empty() {
            "linux-server".to_owned()
        } else {
            comm.to_owned()
        };

        if is_wsl_noise_process(&process_name, cmd_opt.as_deref(), port) {
            continue;
        }

        let runtime = infer_runtime(&process_name, cmd_opt.as_deref(), None);
        let (project_name, project_path) = infer_project_info(
            &process_name,
            None,
            cwd_opt.as_deref(),
            cmd_opt.as_deref(),
            &runtime,
        );

        entries.push(DevPortEntry {
            id: format!("wsl:{distro}:{pid}:{port}"),
            port,
            bind_address: bind_address.clone(),
            url: format_listen_url(&bind_address, port),
            pid,
            process_name,
            runtime,
            project_name,
            project_path,
            command_line: cmd_opt,
            environment: "wsl".to_owned(),
            wsl_distro: Some(distro.to_owned()),
            icon: None,
        });
    }
    entries
}

/// Extrai `(bind_address, port)` de formatos do `ss` como `127.0.0.1:5173`,
/// `*:3000`, `[::]:8080`, `[::1]%lo:3000` ou `127.0.0.1%eth0:8000`.
fn split_host_and_port(addr: &str) -> Option<(String, u16)> {
    let trimmed = addr.trim();
    let colon_idx = trimmed.rfind(':')?;
    let port_part = trimmed[colon_idx + 1..].trim();
    let port = port_part.parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }
    let raw_host = &trimmed[..colon_idx];
    let host = raw_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split('%')
        .next()
        .unwrap_or("")
        .trim();
    let normalized_host = if host.is_empty() || host == "*" {
        "0.0.0.0".to_owned()
    } else {
        host.to_owned()
    };
    Some((normalized_host, port))
}

#[cfg(windows)]
mod win32 {
    use super::{Ipv4Addr, Ipv6Addr, RawTcpListener};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6TABLE_OWNER_PID, MIB_TCPTABLE_OWNER_PID, MIB_TCP_STATE_LISTEN,
        TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessBasicInformation {
        Reserved1: *mut c_void,
        PebBaseAddress: usize,
        Reserved2: [*mut c_void; 2],
        UniqueProcessId: usize,
        Reserved3: *mut c_void,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct UnicodeString64 {
        Length: u16,
        MaximumLength: u16,
        _padding: u32,
        Buffer: u64,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: *mut c_void,
            process_information_class: u32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn ReadProcessMemory(
            h_process: *mut c_void,
            lp_base_address: *const c_void,
            lp_buffer: *mut c_void,
            n_size: usize,
            lp_number_of_bytes_read: *mut usize,
        ) -> i32;
    }

    pub fn list_tcp_listeners() -> Result<Vec<RawTcpListener>, String> {
        let mut listeners = Vec::new();
        collect_table(AF_INET, &mut listeners)?;
        collect_table(AF_INET6, &mut listeners)?;
        Ok(listeners)
    }

    fn collect_table(af: u32, out: &mut Vec<RawTcpListener>) -> Result<(), String> {
        let mut size: u32 = 0;
        let first = unsafe {
            GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_LISTENER, 0)
        };
        if first != ERROR_INSUFFICIENT_BUFFER.0 && first != NO_ERROR.0 {
            return Err(format!(
                "GetExtendedTcpTable(af={af}) retornou código {first}"
            ));
        }
        if size == 0 {
            return Ok(());
        }

        let mut buffer = vec![0u8; size as usize];
        loop {
            let status = unsafe {
                GetExtendedTcpTable(
                    Some(buffer.as_mut_ptr().cast()),
                    &mut size,
                    false,
                    af,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            };
            if status == ERROR_INSUFFICIENT_BUFFER.0 {
                buffer.resize(size as usize, 0);
                continue;
            }
            if status != NO_ERROR.0 {
                return Err(format!(
                    "GetExtendedTcpTable(af={af}) falhou com código {status}"
                ));
            }
            break;
        }

        if af == AF_INET {
            let table_ptr = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            let count = unsafe { (*table_ptr).dwNumEntries as usize };
            let rows_ptr = unsafe {
                std::ptr::addr_of!((*table_ptr).table)
                    .cast::<windows::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID>()
            };
            let rows = unsafe { std::slice::from_raw_parts(rows_ptr, count) };
            for row in rows {
                if row.dwState != MIB_TCP_STATE_LISTEN.0 as u32 {
                    continue;
                }
                let port = u16::from_be(row.dwLocalPort as u16);
                if port == 0 {
                    continue;
                }
                let ip = Ipv4Addr::from(u32::from_be(row.dwLocalAddr));
                out.push(RawTcpListener {
                    pid: row.dwOwningPid,
                    port,
                    bind_address: ip.to_string(),
                    is_ipv4: true,
                });
            }
        } else {
            let table_ptr = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            let count = unsafe { (*table_ptr).dwNumEntries as usize };
            let rows_ptr = unsafe {
                std::ptr::addr_of!((*table_ptr).table)
                    .cast::<windows::Win32::NetworkManagement::IpHelper::MIB_TCP6ROW_OWNER_PID>()
            };
            let rows = unsafe { std::slice::from_raw_parts(rows_ptr, count) };
            for row in rows {
                if row.dwState != MIB_TCP_STATE_LISTEN.0 as u32 {
                    continue;
                }
                let port = u16::from_be(row.dwLocalPort as u16);
                if port == 0 {
                    continue;
                }
                let ip = Ipv6Addr::from(row.ucLocalAddr);
                out.push(RawTcpListener {
                    pid: row.dwOwningPid,
                    port,
                    bind_address: ip.to_string(),
                    is_ipv4: false,
                });
            }
        }

        Ok(())
    }

    pub fn image_path(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
        };
        unsafe {
            let _ = CloseHandle(handle);
        }
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        let path = path.trim();
        if path.is_empty() || !path.contains('\\') {
            None
        } else {
            Some(path.to_owned())
        }
    }

    /// Lê `CurrentDirectory` e `CommandLine` diretamente do PEB (x64) sem spawn de subprocesso.
    pub fn read_process_peb_strings(pid: u32) -> (Option<String>, Option<String>) {
        if pid == 0 || !cfg!(target_pointer_width = "64") {
            return (None, None);
        }
        let handle =
            match unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) } {
                Ok(h) => h,
                Err(_) => return (None, None),
            };

        let result = unsafe { read_peb_strings_inner(handle.0) };
        unsafe {
            let _ = CloseHandle(handle);
        }
        result
    }

    unsafe fn read_peb_strings_inner(handle: *mut c_void) -> (Option<String>, Option<String>) {
        let mut pbi = std::mem::zeroed::<ProcessBasicInformation>();
        let mut ret_len = 0u32;
        let status = NtQueryInformationProcess(
            handle,
            0,
            (&raw mut pbi).cast(),
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut ret_len,
        );
        if status < 0 || pbi.PebBaseAddress == 0 {
            return (None, None);
        }

        // Em Windows x64: PEB.ProcessParameters fica no offset 0x20.
        let mut params_addr: u64 = 0;
        if !read_mem(handle, pbi.PebBaseAddress + 0x20, &mut params_addr) || params_addr == 0 {
            return (None, None);
        }

        // Em RTL_USER_PROCESS_PARAMETERS (x64):
        //   CurrentDirectory.DosPath -> offset 0x38
        //   CommandLine              -> offset 0x70
        let cwd = read_remote_unicode_string(handle, params_addr as usize + 0x38);
        let cmd = read_remote_unicode_string(handle, params_addr as usize + 0x70);
        (cwd, cmd)
    }

    unsafe fn read_mem<T>(handle: *mut c_void, addr: usize, out: &mut T) -> bool {
        let mut read = 0usize;
        ReadProcessMemory(
            handle,
            addr as *const c_void,
            (out as *mut T).cast(),
            std::mem::size_of::<T>(),
            &mut read,
        ) != 0
            && read == std::mem::size_of::<T>()
    }

    unsafe fn read_remote_unicode_string(handle: *mut c_void, addr: usize) -> Option<String> {
        let mut us = std::mem::zeroed::<UnicodeString64>();
        if !read_mem(handle, addr, &mut us) || us.Length == 0 || us.Buffer == 0 {
            return None;
        }
        let byte_len = usize::from(us.Length).min(8192);
        let mut raw = vec![0u8; byte_len];
        let mut read = 0usize;
        if ReadProcessMemory(
            handle,
            us.Buffer as *const c_void,
            raw.as_mut_ptr().cast(),
            byte_len,
            &mut read,
        ) == 0
            || read < 2
        {
            return None;
        }
        raw.truncate(read & !1);
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let text = String::from_utf16_lossy(&units).trim().to_owned();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn batch_query_command_lines(pids: &[u32]) -> HashMap<u32, String> {
        let mut map = HashMap::new();
        if pids.is_empty() {
            return map;
        }
        let filter = pids
            .iter()
            .map(|pid| format!("ProcessId={pid}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let script = format!(
            "Get-CimInstance Win32_Process -Filter '{filter}' -ErrorAction SilentlyContinue | ForEach-Object {{ \"$($_.ProcessId)`t$($_.CommandLine)\" }}"
        );
        let Ok(output) = crate::process::hidden_command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
        else {
            return map;
        };
        if !output.status.success() {
            return map;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if let Some((pid_str, cmd)) = line.split_once('\t') {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    let cmd_clean = cmd.trim();
                    if !cmd_clean.is_empty() {
                        map.insert(pid, cmd_clean.to_owned());
                    }
                }
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::{
        finalize_entries, infer_project_info, infer_runtime, is_windows_noise_process,
        parse_wsl_ss_output, DevPortEntry,
    };

    #[test]
    fn infers_popular_dev_runtimes() {
        assert_eq!(
            infer_runtime(
                "node.exe",
                Some("node C:\\dev\\pulse\\node_modules\\vite\\bin\\vite.js"),
                None
            ),
            "Vite"
        );
        assert_eq!(
            infer_runtime(
                "node",
                Some("node /home/lucas/site/node_modules/.bin/next dev"),
                None
            ),
            "Next.js"
        );
        assert_eq!(
            infer_runtime("workerd.exe", Some("workerd serve --port 8787"), None),
            "Wrangler"
        );
        assert_eq!(
            infer_runtime(
                "python3",
                Some("python3 -m uvicorn app.main:app --reload"),
                None
            ),
            "Uvicorn"
        );
        assert_eq!(
            infer_runtime(
                "api.exe",
                None,
                Some("C:\\dev\\meu-api\\target\\debug\\api.exe")
            ),
            "Rust"
        );
        assert_eq!(infer_runtime("redis-server", None, None), "Redis");
    }

    #[test]
    fn infers_project_name_and_root_from_cwd_and_command_line() {
        let (name, path) = infer_project_info(
            "node",
            None,
            Some("/home/lucas/projects/pulse/landing"),
            Some("node /home/lucas/projects/pulse/landing/node_modules/.bin/vite"),
            "Vite",
        );
        assert_eq!(name, "landing");
        assert_eq!(path.as_deref(), Some("/home/lucas/projects/pulse/landing"));

        let (rust_name, rust_path) = infer_project_info(
            "backend.exe",
            Some("C:\\Users\\lucas\\dev\\billing-api\\target\\debug\\backend.exe"),
            None,
            None,
            "Rust",
        );
        assert_eq!(rust_name, "billing-api");
        assert_eq!(
            rust_path.as_deref(),
            Some("C:\\Users\\lucas\\dev\\billing-api")
        );

        let (cmd_name, cmd_path) = infer_project_info(
            "node.exe",
            None,
            Some("C:\\Windows\\System32"),
            Some("\"C:\\Program Files\\nodejs\\node.exe\" \"C:\\projetos\\loja-web\\node_modules\\next\\dist\\bin\\next\" dev"),
            "Next.js",
        );
        assert_eq!(cmd_name, "loja-web");
        assert_eq!(cmd_path.as_deref(), Some("C:\\projetos\\loja-web"));
    }

    #[test]
    fn parses_wsl_ss_output_with_ipv4_and_ipv6() {
        let sample = "\
127.0.0.1:5173\t1420\tnode\t/home/lucas/projects/pulse/landing\tnode /home/lucas/projects/pulse/landing/node_modules/.bin/vite --port 5173
[::]:3000\t2201\tnode\t/home/lucas/projects/saas-web\tnode /home/lucas/projects/saas-web/node_modules/.bin/next dev
127.0.0.1:53\t101\tsystemd-resolve\t/\t/lib/systemd/systemd-resolved
";
        let entries = parse_wsl_ss_output("Ubuntu", sample);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].port, 5173);
        assert_eq!(entries[0].runtime, "Vite");
        assert_eq!(entries[0].project_name, "landing");
        assert_eq!(entries[0].url, "http://localhost:5173");
        assert_eq!(entries[0].environment, "wsl");
        assert_eq!(entries[0].wsl_distro.as_deref(), Some("Ubuntu"));

        assert_eq!(entries[1].port, 3000);
        assert_eq!(entries[1].runtime, "Next.js");
        assert_eq!(entries[1].project_name, "saas-web");
        assert_eq!(entries[1].url, "http://localhost:3000");
    }

    #[test]
    fn deduplicates_ipv4_and_ipv6_keeping_ipv4() {
        let mut list = vec![
            DevPortEntry {
                id: "wsl:Ubuntu:500:3000".into(),
                port: 3000,
                bind_address: "::1".into(),
                url: "http://localhost:3000".into(),
                pid: 500,
                process_name: "node".into(),
                runtime: "Next.js".into(),
                project_name: "web".into(),
                project_path: Some("/home/lucas/web".into()),
                command_line: None,
                environment: "wsl".into(),
                wsl_distro: Some("Ubuntu".into()),
                icon: None,
            },
            DevPortEntry {
                id: "wsl:Ubuntu:500:3000".into(),
                port: 3000,
                bind_address: "127.0.0.1".into(),
                url: "http://localhost:3000".into(),
                pid: 500,
                process_name: "node".into(),
                runtime: "Next.js".into(),
                project_name: "web".into(),
                project_path: Some("/home/lucas/web".into()),
                command_line: None,
                environment: "wsl".into(),
                wsl_distro: Some("Ubuntu".into()),
                icon: None,
            },
        ];
        finalize_entries(&mut list);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].bind_address, "127.0.0.1");
    }

    #[test]
    fn filters_windows_system_and_ephemeral_desktop_noise() {
        assert!(is_windows_noise_process(
            "svchost.exe",
            Some("C:\\Windows\\System32\\svchost.exe"),
            8080
        ));
        assert!(is_windows_noise_process(
            "Spotify.exe",
            Some("C:\\Users\\lucas\\AppData\\Roaming\\Spotify\\Spotify.exe"),
            57621
        ));
        assert!(is_windows_noise_process("pulse-windows.exe", None, 3100));
        assert!(!is_windows_noise_process(
            "node.exe",
            Some("C:\\Program Files\\nodejs\\node.exe"),
            51234
        ));
        assert!(!is_windows_noise_process(
            "api.exe",
            Some("C:\\dev\\api\\target\\debug\\api.exe"),
            55000
        ));
    }
}
