use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HKCU_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const HKLM_RUN_KEY: &str = r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run";
const HKCU_APPROVED_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const HKLM_APPROVED_KEY: &str =
    r"HKLM\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

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
        .args(["query", HKLM_APPROVED_KEY])
        .output()
    {
        if out.status.success() {
            hklm = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
        }
    }

    // 2. HKCU StartupApproved\Run (prioridade sobre HKLM para o usuário atual)
    if let Ok(out) = crate::process::hidden_command("reg.exe")
        .args(["query", HKCU_APPROVED_KEY])
        .output()
    {
        if out.status.success() {
            hkcu = parse_approved_entries(&String::from_utf8_lossy(&out.stdout));
        }
    }

    (hkcu, hklm)
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

    let approved_key = match origin.as_str() {
        "HKCU\\Run" => HKCU_APPROVED_KEY,
        "HKLM\\Run" => HKLM_APPROVED_KEY,
        _ => return Err("Origem de inicialização inválida".into()),
    };

    // No Windows, o Task Manager grava 02 no primeiro byte para Ativado
    // e 03 no primeiro byte para Desativado em HKCU\...\StartupApproved\Run
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
                <Data Name='Name'>C:\Users\lucas\AppData\Local\Discord\app-1.0.9000\Discord.exe</Data>
                <Data Name='TotalTime'>2150</Data>
            </EventData>
        </Event>
        "#;
        let mut map = HashMap::new();
        parse_diagnostics_events(xml, &mut map);
        assert_eq!(map.get("discord.exe"), Some(&2150));
    }
}
