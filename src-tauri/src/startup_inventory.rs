use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HKCU_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const HKLM_RUN_KEY: &str = r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run";
const HKLM_RUN32_KEY: &str = r"HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";

pub const HKCU_APPROVED_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
pub const HKLM_APPROVED_KEY: &str =
    r"HKLM\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

const HKCU_APPROVED_RUN: &str = HKCU_APPROVED_KEY;
const HKLM_APPROVED_RUN: &str = HKLM_APPROVED_KEY;

const HKCU_APPROVED_RUN32: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32";
const HKLM_APPROVED_RUN32: &str =
    r"HKLM\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32";

const HKCU_APPROVED_STARTUP_FOLDER: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";
const HKLM_APPROVED_STARTUP_FOLDER: &str =
    r"HKLM\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";

const APPMODEL_APPDATA_KEY: &str = r"HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

// Cache de itens de inicialização com TTL de 10s para evitar subprocessos repetidos
struct StartupCache {
    items: Vec<StartupItem>,
    fetched_at: Instant,
}
static STARTUP_CACHE: Mutex<Option<StartupCache>> = Mutex::new(None);
const STARTUP_CACHE_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupItem {
    pub name: String,
    pub command: String,
    pub origin: String,
    pub enabled: bool,
    pub can_change: bool,
    pub unavailable_reason: Option<String>,
    #[serde(default = "default_impact")]
    pub impact: String,
    #[serde(default)]
    pub startup_time_ms: Option<u64>,
    #[serde(default)]
    pub icon: Option<String>,
}

fn default_impact() -> String {
    "Não medido".to_string()
}

pub fn parse_reg_line(line: &str) -> Option<(String, String)> {
    let reg_types = [
        "REG_SZ",
        "REG_EXPAND_SZ",
        "REG_BINARY",
        "REG_MULTI_SZ",
        "REG_DWORD",
    ];
    for reg_type in reg_types {
        if let Some(idx) = line.find(reg_type) {
            let name = line[..idx].trim();
            let val = line[idx + reg_type.len()..].trim();
            if !name.is_empty() {
                return Some((name.to_string(), val.to_string()));
            }
        }
    }
    None
}

pub fn is_approved_enabled(hex: &str) -> bool {
    let hex = hex.trim();
    if hex.len() >= 2 {
        if let Ok(b) = u8::from_str_radix(&hex[..2], 16) {
            // No Windows: 0x02 = ativado, 0x03 = desativado pelo usuário no Task Manager.
            // Bytes ímpares (b & 1 != 0) indicam item desativado.
            return (b & 1) == 0;
        }
    }
    true
}

pub fn parse_approved_entries(text: &str) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    for line in text.lines() {
        if let Some((name, val)) = parse_reg_line(line) {
            map.insert(name.to_lowercase(), is_approved_enabled(&val));
        }
    }
    map
}

pub fn determine_impact(
    name: &str,
    command: &str,
    durations: &HashMap<String, u64>,
) -> (String, Option<u64>) {
    let lower_name = name.to_lowercase();
    let lower_cmd = command.to_lowercase();

    // 1. Verifica se há tempo medido nos eventos de diagnóstico de boot do Windows
    for (proc_name, &ms) in durations {
        if lower_name.contains(proc_name) || lower_cmd.contains(proc_name) {
            let impact = if ms >= 1000 {
                "Alto impacto".to_string()
            } else if ms >= 300 {
                "Médio impacto".to_string()
            } else {
                "Baixo impacto".to_string()
            };
            return (impact, Some(ms));
        }
    }

    ("Não medido".to_string(), None)
}

fn startup_approved_hex(enabled: bool) -> Result<String, String> {
    if enabled {
        return Ok("020000000000000000000000".to_owned());
    }

    let filetime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| {
            elapsed
                .as_secs()
                .saturating_add(11_644_473_600)
                .saturating_mul(10_000_000)
        })
        .map_err(|error| format!("Relógio do sistema inválido: {error}"))?;
    let mut bytes = vec![0x03, 0x00, 0x00, 0x00];
    bytes.extend_from_slice(&filetime.to_le_bytes());
    Ok(bytes.iter().map(|byte| format!("{byte:02X}")).collect())
}

pub fn parse_run_entries(text: &str) -> Vec<StartupItem> {
    parse_run_entries_with_meta(text, "HKCU\\Run", &HashMap::new(), &HashMap::new())
}

pub fn parse_run_entries_with_meta(
    text: &str,
    origin: &str,
    approved_map: &HashMap<String, bool>,
    durations_map: &HashMap<String, u64>,
) -> Vec<StartupItem> {
    text.lines()
        .filter_map(|line| {
            let (name, command) = parse_reg_line(line)?;
            let key = name.to_lowercase();
            let enabled = approved_map.get(&key).copied().unwrap_or(true);
            let (impact, startup_time_ms) = determine_impact(&name, &command, durations_map);
            let icon = crate::app_icons::icon_for_command(&command);
            let is_pulse = name.eq_ignore_ascii_case("Pulse");
            Some(StartupItem {
                name,
                command,
                origin: origin.to_string(),
                enabled,
                can_change: !is_pulse,
                unavailable_reason: if is_pulse {
                    Some("O autostart do Pulse é gerenciado pela Visão geral".into())
                } else {
                    None
                },
                impact,
                startup_time_ms,
                icon,
            })
        })
        .collect()
}

pub fn parse_diagnostics_events(xml: &str, map: &mut HashMap<String, u64>) {
    for event_chunk in xml.split("</Event>") {
        let mut app_name = None;
        let mut total_time = None;

        for line in event_chunk.lines() {
            if line.contains("Name='Name'") || line.contains("Name=\"Name\"") {
                if let (Some(start), Some(end)) = (line.find('>'), line.rfind('<')) {
                    if start < end {
                        let path = line[start + 1..end].trim();
                        let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
                        if !file.is_empty() {
                            app_name = Some(file.to_lowercase());
                        }
                    }
                }
            } else if line.contains("Name='TotalTime'") || line.contains("Name=\"TotalTime\"") {
                if let (Some(start), Some(end)) = (line.find('>'), line.rfind('<')) {
                    if start < end {
                        if let Ok(ms) = line[start + 1..end].trim().parse::<u64>() {
                            total_time = Some(ms);
                        }
                    }
                }
            }
        }

        if let (Some(name), Some(time)) = (app_name, total_time) {
            map.entry(name).or_insert(time);
        }
    }
}

fn get_diagnostics_boot_durations() -> HashMap<String, u64> {
    let mut map = HashMap::new();
    let out = crate::process::hidden_command("wevtutil.exe")
        .args([
            "qe",
            "Microsoft-Windows-Diagnostics-Performance/Operational",
            "/q:*[System[(EventID=101 or EventID=102 or EventID=103 or EventID=107 or EventID=108)]]",
            "/c:50",
            "/rd:true",
            "/f:RenderedXml",
        ])
        .output();

    if let Ok(result) = out {
        if result.status.success() {
            let xml = String::from_utf8_lossy(&result.stdout);
            parse_diagnostics_events(&xml, &mut map);
        }
    }
    map
}

fn get_approved_maps() -> (HashMap<String, bool>, HashMap<String, bool>) {
    let mut hkcu = HashMap::new();
    let mut hklm = HashMap::new();

    // 1. HKLM StartupApproved\Run
    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKLM_APPROVED_RUN])
        .output()
    {
        if out.status.success() {
            hklm = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
        }
    }

    // 2. HKCU StartupApproved\Run (prioridade sobre HKLM para o usuário atual)
    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKCU_APPROVED_RUN])
        .output()
    {
        if out.status.success() {
            hkcu = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
        }
    }

    (hkcu, hklm)
}

fn get_run32_approved_maps() -> HashMap<String, bool> {
    let mut map = HashMap::new();

    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKLM_APPROVED_RUN32])
        .output()
    {
        if out.status.success() {
            map = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
        }
    }

    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKCU_APPROVED_RUN32])
        .output()
    {
        if out.status.success() {
            let user_map = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
            for (k, v) in user_map {
                map.insert(k, v);
            }
        }
    }

    map
}

fn get_folder_approved_maps() -> HashMap<String, bool> {
    let mut map = HashMap::new();

    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKLM_APPROVED_STARTUP_FOLDER])
        .output()
    {
        if out.status.success() {
            map = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
        }
    }

    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKCU_APPROVED_STARTUP_FOLDER])
        .output()
    {
        if out.status.success() {
            let user_map = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
            for (k, v) in user_map {
                map.insert(k, v);
            }
        }
    }

    map
}

pub fn parse_appx_state_output(output: &str) -> Vec<(String, String, bool)> {
    let mut items = Vec::new();
    let mut current_key: Option<String> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("HKEY_CURRENT_USER\\") || trimmed.starts_with("HKEY_LOCAL_MACHINE\\")
        {
            current_key = Some(trimmed.to_string());
            continue;
        }

        if let Some(ref key) = current_key {
            if let Some((name, val)) = parse_reg_line(line) {
                if name.eq_ignore_ascii_case("State") {
                    let hex_str = val.trim().trim_start_matches("0x");
                    let state_code = u32::from_str_radix(hex_str, 16).unwrap_or(0);
                    let enabled = state_code == 2;

                    let parts: Vec<&str> = key.split('\\').collect();
                    if parts.len() >= 2 {
                        let task_id = parts[parts.len() - 1].to_string();
                        let package_family = parts[parts.len() - 2].to_string();

                        let is_metadata = task_id.eq_ignore_ascii_case("Schemas")
                            || task_id.eq_ignore_ascii_case("HAM")
                            || task_id.eq_ignore_ascii_case("PSR")
                            || task_id.eq_ignore_ascii_case("SplashScreen")
                            || task_id.eq_ignore_ascii_case("AppUriHandlers")
                            || task_id.eq_ignore_ascii_case("PersistedStorageItemTable");

                        if !is_metadata {
                            items.push((package_family, task_id, enabled));
                        }
                    }
                }
            }
        }
    }

    items
}

pub fn parse_appx_values_by_family(output: &str, target_val_name: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut current_key: Option<String> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("HKEY_CURRENT_USER\\") || trimmed.starts_with("HKEY_LOCAL_MACHINE\\")
        {
            current_key = Some(trimmed.to_string());
            continue;
        }

        if let Some(ref key) = current_key {
            if let Some((name, val)) = parse_reg_line(line) {
                if name.eq_ignore_ascii_case(target_val_name) {
                    if let Some(idx) = key.find(r"AppModel\SystemAppData\") {
                        let sub = &key[idx + r"AppModel\SystemAppData\".len()..];
                        let family = sub.split('\\').next().unwrap_or(sub);
                        if !family.is_empty() {
                            map.insert(family.to_lowercase(), val.trim().to_string());
                        }
                    }
                }
            }
        }
    }

    map
}

pub fn clean_appx_display_name(
    raw_name: Option<&str>,
    package_family: &str,
    task_id: &str,
) -> String {
    if let Some(name) = raw_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("ms-resource:") && !trimmed.starts_with("@{")
        {
            if trimmed.eq_ignore_ascii_case("ChatGPT Classic") {
                return "ChatGPT".to_string();
            }
            return trimmed.to_string();
        }
    }

    let lower_task = task_id.to_lowercase();
    if lower_task.contains("chatgpt") {
        return "ChatGPT".to_string();
    }
    if lower_task.contains("claude") {
        return "Claude".to_string();
    }
    if lower_task.contains("spotify") {
        return "Spotify".to_string();
    }
    if lower_task.contains("whatsapp") {
        return "WhatsApp".to_string();
    }

    let base = package_family.split('_').next().unwrap_or(package_family);
    let name_part = base.rsplit('.').next().unwrap_or(base);
    let cleaned = name_part
        .strip_suffix("-Desktop")
        .or_else(|| name_part.strip_suffix("Desktop"))
        .unwrap_or(name_part);

    let mut result = String::new();
    let mut prev_is_lower = false;
    for ch in cleaned.chars() {
        if ch.is_uppercase() && prev_is_lower {
            result.push(' ');
        }
        result.push(ch);
        prev_is_lower = ch.is_lowercase();
    }

    if result.is_empty() {
        task_id.to_string()
    } else {
        result
    }
}

fn search_folder_for_icon(base: &Path) -> Option<String> {
    if !base.is_dir() {
        return None;
    }

    let candidates = [base.join("assets"), base.to_path_buf()];
    let icon_filenames = [
        "AppList.targetsize-48.png",
        "AppList.targetsize-32.png",
        "Square44x44Logo.targetsize-48.png",
        "Square44x44Logo.targetsize-32.png",
        "Square44x44Logo.png",
        "StoreLogo.scale-100.png",
        "StoreLogo.png",
    ];

    for folder in &candidates {
        if !folder.is_dir() {
            continue;
        }

        for fname in &icon_filenames {
            let p = folder.join(fname);
            if p.is_file() {
                if let Ok(bytes) = std::fs::read(&p) {
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    return Some(format!("data:image/png;base64,{b64}"));
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let ext = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if ext == "ico" {
                        if let Some(icon) = crate::app_icons::icon_for_path(&p) {
                            return Some(icon);
                        }
                    } else if ext == "png" {
                        let stem = p
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        if stem.contains("logo")
                            || stem.contains("targetsize-48")
                            || stem.contains("targetsize-32")
                        {
                            if let Ok(bytes) = std::fs::read(&p) {
                                use base64::Engine;
                                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                return Some(format!("data:image/png;base64,{b64}"));
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

fn find_appx_icon(package_family: &str, package_full_name: Option<&str>) -> Option<String> {
    if let Some(full_name) = package_full_name {
        let win_apps = PathBuf::from(r"C:\Program Files\WindowsApps").join(full_name);
        if let Some(icon) = search_folder_for_icon(&win_apps) {
            return Some(icon);
        }
    }

    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let pkg_folder = PathBuf::from(local_appdata)
            .join("Packages")
            .join(package_family);
        if let Some(icon) = search_folder_for_icon(&pkg_folder) {
            return Some(icon);
        }
    }

    None
}

pub fn parse_startup_folder(
    dir: &Path,
    origin: &str,
    approved_map: &HashMap<String, bool>,
    durations_map: &HashMap<String, u64>,
) -> Vec<StartupItem> {
    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return items;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name.is_empty() || file_name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }

        let display_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
            .to_string();

        let lower_file = file_name.to_lowercase();
        let lower_display = display_name.to_lowercase();

        let enabled = approved_map
            .get(&lower_file)
            .or_else(|| approved_map.get(&lower_display))
            .copied()
            .unwrap_or(true);

        let command = path.to_string_lossy().into_owned();
        let (impact, startup_time_ms) = determine_impact(&display_name, &command, durations_map);
        let icon = crate::app_icons::icon_for_command(&command)
            .or_else(|| crate::app_icons::icon_for_path(&path));

        items.push(StartupItem {
            name: display_name,
            command,
            origin: origin.to_string(),
            enabled,
            can_change: true,
            unavailable_reason: None,
            impact,
            startup_time_ms,
            icon,
        });
    }

    items
}

fn resolve_startup_folder_filename(name: &str) -> String {
    let user_folder =
        dirs::data_dir().map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\Startup"));
    let common_folder = std::env::var_os("ProgramData")
        .map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs\Startup"));

    for folder in [user_folder, common_folder].into_iter().flatten() {
        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let fstem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if fname.eq_ignore_ascii_case(name) || fstem.eq_ignore_ascii_case(name) {
                        return fname.to_string();
                    }
                }
            }
        }
    }

    if name.to_lowercase().ends_with(".lnk") || name.to_lowercase().ends_with(".url") {
        name.to_string()
    } else {
        format!("{name}.lnk")
    }
}

fn load_appx_startup_items(durations_map: &HashMap<String, u64>) -> Vec<StartupItem> {
    let mut items = Vec::new();

    let Ok(state_out) = crate::process::hidden_command("reg.exe")
        .args(["query", APPMODEL_APPDATA_KEY, "/s", "/v", "State"])
        .output()
    else {
        return items;
    };

    if !state_out.status.success() {
        return items;
    }

    let state_text = String::from_utf8_lossy(&state_out.stdout);
    let appx_entries = parse_appx_state_output(&state_text);
    if appx_entries.is_empty() {
        return items;
    }

    let mut full_names_map = HashMap::new();
    if let Ok(fn_out) = crate::process::hidden_command("reg.exe")
        .args(["query", APPMODEL_APPDATA_KEY, "/s", "/v", "PackageFullName"])
        .output()
    {
        if fn_out.status.success() {
            full_names_map = parse_appx_values_by_family(
                &String::from_utf8_lossy(&fn_out.stdout),
                "PackageFullName",
            );
        }
    }

    let mut app_names_map = HashMap::new();
    if let Ok(an_out) = crate::process::hidden_command("reg.exe")
        .args(["query", APPMODEL_APPDATA_KEY, "/s", "/v", "AppName"])
        .output()
    {
        if an_out.status.success() {
            app_names_map =
                parse_appx_values_by_family(&String::from_utf8_lossy(&an_out.stdout), "AppName");
        }
    }

    for (pkg_family, task_id, enabled) in appx_entries {
        let raw_app_name = app_names_map
            .get(&pkg_family.to_lowercase())
            .map(|s| s.as_str());
        let display_name = clean_appx_display_name(raw_app_name, &pkg_family, &task_id);
        let pkg_full_name = full_names_map
            .get(&pkg_family.to_lowercase())
            .map(|s| s.as_str());
        let icon = find_appx_icon(&pkg_family, pkg_full_name);
        let command = format!("{pkg_family}!{task_id}");
        let (impact, startup_time_ms) = determine_impact(&display_name, &command, durations_map);

        items.push(StartupItem {
            name: display_name,
            command,
            origin: "Pacote Windows (AppX)".to_string(),
            enabled,
            can_change: true,
            unavailable_reason: None,
            impact,
            startup_time_ms,
            icon,
        });
    }

    items
}

#[tauri::command]
pub fn list_startup_items() -> Result<Vec<StartupItem>, String> {
    // Retorna do cache se ainda válido (TTL 10s)
    if let Ok(guard) = STARTUP_CACHE.lock() {
        if let Some(ref cached) = *guard {
            if cached.fetched_at.elapsed() < STARTUP_CACHE_TTL {
                return Ok(cached.items.clone());
            }
        }
    }

    let (hkcu_approved, hklm_approved) = get_approved_maps();
    let run32_approved = get_run32_approved_maps();
    let folder_approved = get_folder_approved_maps();
    let durations_map = get_diagnostics_boot_durations();
    let mut items = Vec::new();

    // 1. HKCU\Run
    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKCU_RUN_KEY])
        .output()
    {
        if out.status.success() {
            let hkcu_items = parse_run_entries_with_meta(
                &String::from_utf8_lossy(&out.stdout),
                "HKCU\\Run",
                &hkcu_approved,
                &durations_map,
            );
            items.extend(hkcu_items);
        }
    }

    // 2. HKLM\Run
    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKLM_RUN_KEY])
        .output()
    {
        if out.status.success() {
            let hklm_items = parse_run_entries_with_meta(
                &String::from_utf8_lossy(&out.stdout),
                "HKLM\\Run",
                &hklm_approved,
                &durations_map,
            );
            items.extend(hklm_items);
        }
    }

    // 3. HKLM\Run (32-bit / WOW6432Node)
    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKLM_RUN32_KEY])
        .output()
    {
        if out.status.success() {
            let wow_items = parse_run_entries_with_meta(
                &String::from_utf8_lossy(&out.stdout),
                "HKLM\\Run (32-bit)",
                &run32_approved,
                &durations_map,
            );
            items.extend(wow_items);
        }
    }

    // 4. Pasta Inicializar do Usuário (%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup)
    if let Some(user_startup) =
        dirs::data_dir().map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
    {
        if user_startup.is_dir() {
            let folder_items = parse_startup_folder(
                &user_startup,
                "Startup (Usuário)",
                &folder_approved,
                &durations_map,
            );
            items.extend(folder_items);
        }
    }

    // 5. Pasta Inicializar Comum (%PROGRAMDATA%\Microsoft\Windows\Start Menu\Programs\Startup)
    if let Some(common_startup) = std::env::var_os("ProgramData")
        .map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
    {
        if common_startup.is_dir() {
            let common_items = parse_startup_folder(
                &common_startup,
                "Startup (Comum)",
                &folder_approved,
                &durations_map,
            );
            items.extend(common_items);
        }
    }

    // 6. Pacotes Windows (AppX / MSIX / UWP como ChatGPT, Claude, Spotify)
    let appx_items = load_appx_startup_items(&durations_map);
    items.extend(appx_items);

    // Deduplicação preservando a primeira ocorrência por (origem, nome)
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert((item.origin.to_lowercase(), item.name.to_lowercase())));

    // Persiste no cache
    if let Ok(mut guard) = STARTUP_CACHE.lock() {
        *guard = Some(StartupCache {
            items: items.clone(),
            fetched_at: Instant::now(),
        });
    }

    Ok(items)
}

#[tauri::command]
pub fn set_startup_item_enabled(
    name: String,
    origin: String,
    enabled: bool,
) -> Result<StartupItem, String> {
    if name.eq_ignore_ascii_case("Pulse") {
        return Err("O autostart do Pulse é gerenciado pela Visão geral".into());
    }
    if name.is_empty() || name.contains('"') {
        return Err("Nome de entrada inválido".into());
    }

    let origin_str = origin.as_str();

    if origin_str == "Pacote Windows (AppX)" || origin_str == "Microsoft Store (AppX)" {
        let current_items = list_startup_items().unwrap_or_default();
        let matched = current_items
            .into_iter()
            .find(|i| i.name.eq_ignore_ascii_case(&name) && (i.origin == origin_str));

        let (pkg_family, task_id) = if let Some(ref it) = matched {
            if let Some((p, t)) = it.command.split_once('!') {
                (p.to_string(), t.to_string())
            } else {
                (it.command.clone(), name.clone())
            }
        } else {
            return Err("Aplicativo de pacote Windows não localizado".into());
        };

        let key_path = format!(r"{APPMODEL_APPDATA_KEY}\{pkg_family}\{task_id}");
        let state_val = if enabled { "2" } else { "1" };
        let out = crate::process::hidden_command("reg.exe")
            .args([
                "add",
                &key_path,
                "/v",
                "State",
                "/t",
                "REG_DWORD",
                "/d",
                state_val,
                "/f",
            ])
            .output()
            .map_err(|e| format!("Não foi possível alterar a inicialização: {e}"))?;

        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }

        if enabled {
            let _ = crate::process::hidden_command("reg.exe")
                .args([
                    "add",
                    &key_path,
                    "/v",
                    "UserEnabledStartupOnce",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "1",
                    "/f",
                ])
                .output();
        }
    } else if origin_str == "Startup (Usuário)" || origin_str == "Startup (Comum)" {
        let filename = resolve_startup_folder_filename(&name);
        let hex_val = startup_approved_hex(enabled)?;

        let out = crate::process::hidden_command("reg.exe")
            .args([
                "add",
                HKCU_APPROVED_STARTUP_FOLDER,
                "/v",
                &filename,
                "/t",
                "REG_BINARY",
                "/d",
                hex_val.as_str(),
                "/f",
            ])
            .output()
            .map_err(|e| format!("Não foi possível alterar a inicialização: {e}"))?;

        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
    } else {
        let approved_key = match origin_str {
            "HKCU\\Run" => HKCU_APPROVED_RUN,
            "HKLM\\Run" => HKCU_APPROVED_RUN, // override em HKCU para dispensar elevação
            "HKLM\\Run (32-bit)" | "HKCU\\Run (32-bit)" => HKCU_APPROVED_RUN32,
            _ => return Err("Origem de inicialização inválida".into()),
        };

        let hex_val = startup_approved_hex(enabled)?;

        let out = crate::process::hidden_command("reg.exe")
            .args([
                "add",
                approved_key,
                "/v",
                &name,
                "/t",
                "REG_BINARY",
                "/d",
                hex_val.as_str(),
                "/f",
            ])
            .output()
            .map_err(|e| format!("Não foi possível alterar a inicialização: {e}"))?;

        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
    }

    // Invalida o cache para forçar releitura dos dados atualizados
    if let Ok(mut guard) = STARTUP_CACHE.lock() {
        *guard = None;
    }

    // Localiza e retorna o item atualizado
    let items = list_startup_items()?;
    if let Some(item) = items
        .into_iter()
        .find(|i| i.name.eq_ignore_ascii_case(&name) && i.origin == origin)
    {
        return Ok(item);
    }

    Ok(StartupItem {
        name,
        command: String::new(),
        origin,
        enabled,
        can_change: true,
        unavailable_reason: None,
        impact: default_impact(),
        startup_time_ms: None,
        icon: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hkcu_run_entries() {
        let v =
            parse_run_entries("HKEY_CURRENT_USER\\...\n    Foo    REG_SZ    \"C:\\\\Foo.exe\"\n");
        assert_eq!(v[0].name, "Foo");
        assert!(v[0].can_change);
        assert!(v[0].enabled);
    }

    #[test]
    fn parses_run_command_with_spaces_and_args() {
        let line =
            "    MyApp    REG_SZ    \"C:\\Program Files\\My App\\app.exe\" --start-minimized /bg";
        let parsed = parse_reg_line(line);
        assert_eq!(
            parsed,
            Some((
                "MyApp".to_string(),
                "\"C:\\Program Files\\My App\\app.exe\" --start-minimized /bg".to_string()
            ))
        );
    }

    #[test]
    fn parses_startup_approved_enabled_and_disabled() {
        assert!(is_approved_enabled("020000000000000000000000"));
        assert!(is_approved_enabled("060000000000000000000000"));
        assert!(!is_approved_enabled("030000009A7B316C467BD901"));
        assert!(!is_approved_enabled("010000000000000000000000"));
    }

    #[test]
    fn disabled_startup_approved_value_contains_a_timestamp() {
        let value = startup_approved_hex(false).unwrap();
        assert!(value.starts_with("03000000"));
        assert_ne!(&value[8..], "0000000000000000");
    }

    #[test]
    fn parses_disabled_items_correctly_from_approved() {
        let run_text = "    Discord    REG_SZ    C:\\Users\\user\\Discord.exe\n    OneDrive    REG_SZ    OneDrive.exe";
        let mut approved = HashMap::new();
        approved.insert("discord".to_string(), false);
        approved.insert("onedrive".to_string(), true);

        let items = parse_run_entries_with_meta(run_text, "HKCU\\Run", &approved, &HashMap::new());
        let discord = items.iter().find(|i| i.name == "Discord").unwrap();
        let onedrive = items.iter().find(|i| i.name == "OneDrive").unwrap();

        assert!(!discord.enabled);
        assert!(onedrive.enabled);
    }

    #[test]
    fn determines_slowest_items_impact() {
        let mut durations = HashMap::new();
        durations.insert("heavyapp.exe".to_string(), 2500);

        let (impact1, ms1) = determine_impact("HeavyApp", "C:\\heavyapp.exe", &durations);
        assert_eq!(impact1, "Alto impacto");
        assert_eq!(ms1, Some(2500));

        let (impact2, _) =
            determine_impact("Discord", "C:\\Users\\user\\Discord.exe", &HashMap::new());
        assert_eq!(impact2, "Não medido");
    }

    #[test]
    fn parses_diagnostics_events_xml() {
        let xml = r#"
        <Event>
            <EventData>
                <Data Name='Name'>C:\Users\dev\AppData\Local\Discord\app-1.0.9000\Discord.exe</Data>
                <Data Name='TotalTime'>2150</Data>
            </EventData>
        </Event>
        "#;
        let mut map = HashMap::new();
        parse_diagnostics_events(xml, &mut map);
        assert_eq!(map.get("discord.exe"), Some(&2150));
    }

    #[test]
    fn parses_appx_state_output_correctly() {
        let text = r#"
HKEY_CURRENT_USER\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0\ChatGPT
    State    REG_DWORD    0x2

HKEY_CURRENT_USER\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\Claude_pzs8sxrjxfjjc\ClaudeStartup
    State    REG_DWORD    0x0

HKEY_CURRENT_USER\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0\Schemas
    State    REG_DWORD    0x1
        "#;
        let entries = parse_appx_state_output(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0");
        assert_eq!(entries[0].1, "ChatGPT");
        assert!(entries[0].2); // 0x2 = enabled

        assert_eq!(entries[1].0, "Claude_pzs8sxrjxfjjc");
        assert_eq!(entries[1].1, "ClaudeStartup");
        assert!(!entries[1].2); // 0x0 = disabled
    }

    #[test]
    fn cleans_appx_display_name_accurately() {
        assert_eq!(
            clean_appx_display_name(
                Some("ChatGPT Classic"),
                "OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0",
                "ChatGPT"
            ),
            "ChatGPT"
        );
        assert_eq!(
            clean_appx_display_name(None, "OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0", "ChatGPT"),
            "ChatGPT"
        );
        assert_eq!(
            clean_appx_display_name(Some("Claude"), "Claude_pzs8sxrjxfjjc", "ClaudeStartup"),
            "Claude"
        );
        assert_eq!(
            clean_appx_display_name(
                Some("ms-resource:AppName"),
                "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
                "StartTerminalOnLoginTask"
            ),
            "Windows Terminal"
        );
    }

    #[test]
    fn parses_appx_package_full_names_and_app_names() {
        let fn_text = r#"
HKEY_CURRENT_USER\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0\Schemas
    PackageFullName    REG_SZ    OpenAI.ChatGPT-Desktop_1.2026.190.0_x64__2p2nqsd0c76g0
        "#;
        let map = parse_appx_values_by_family(fn_text, "PackageFullName");
        assert_eq!(
            map.get("openai.chatgpt-desktop_2p2nqsd0c76g0")
                .map(|s| s.as_str()),
            Some("OpenAI.ChatGPT-Desktop_1.2026.190.0_x64__2p2nqsd0c76g0")
        );
    }
}
