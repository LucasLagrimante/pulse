//! Itens de inicialização no Linux (equivalente do Autoruns).
//!
//! Fontes, na ordem em que aparecem:
//!
//! 1. **XDG Autostart** — `~/.config/autostart` (usuário) sobre
//!    `$XDG_CONFIG_DIRS/autostart` (padrão `/etc/xdg/autostart`, sistema). Um
//!    arquivo do usuário com o mesmo nome **substitui** o do sistema (regra da
//!    especificação). Entradas que não valem para a sessão atual
//!    (`OnlyShowIn`/`NotShowIn` contra `$XDG_CURRENT_DESKTOP`) não iniciam e
//!    ficam de fora.
//! 2. **Units systemd do usuário** habilitadas (`.service` e `.timer`), e as
//!    desabilitadas cujo preset é "enabled" — foram desligadas por alguém e
//!    precisam continuar visíveis para serem religadas.
//! 3. **Timers systemd do sistema** habilitados (só leitura: exigem root).
//! 4. **Crontab do usuário** (`crontab -l`, só leitura).
//!
//! ## Ativar/desativar sem apagar nada
//!
//! XDG: o jeito freedesktop de desligar um autostart do sistema é criar uma
//! cópia em `~/.config/autostart` com `Hidden=true`. O arquivo de
//! `/etc/xdg/autostart` **nunca** é tocado (mesmo espírito do `StartupApproved`
//! no Windows). Religar grava `Hidden=false` na cópia.
//!
//! Units de usuário: `systemctl --user enable|disable`, sem senha. Units
//! essenciais da sessão (PipeWire, portal, Plasma…) não podem ser desativadas.
//!
//! ## Impacto no boot
//!
//! `systemd-analyze blame` (sistema e `--user`). O systemd executa os
//! autostarts XDG como `app-<id>@autostart.service`, então eles também têm
//! tempo medido. Sem `systemd-analyze`, o impacto é "Indisponível".

use super::StartupItem;
use crate::services::command;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

pub const ORIGIN_XDG_USER: &str = "Autostart XDG (usuário)";
pub const ORIGIN_XDG_SYSTEM: &str = "Autostart XDG (sistema)";
pub const ORIGIN_USER_UNIT: &str = "systemd (usuário)";
pub const ORIGIN_USER_TIMER: &str = "Timer systemd (usuário)";
pub const ORIGIN_SYSTEM_TIMER: &str = "Timer systemd (sistema)";
pub const ORIGIN_CRONTAB: &str = "Crontab (usuário)";

const IMPACT_UNAVAILABLE: &str = "Indisponível";
const IMPACT_NOT_MEASURED: &str = "Não medido";
const PULSE_MANAGED: &str = "O autostart do Pulse é gerenciado pela Visão geral";

/// Como alterar o item — interno, não vai para o frontend.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryKey {
    Xdg {
        id: String,
        system_path: Option<PathBuf>,
        user_path: Option<PathBuf>,
    },
    UserUnit(String),
    ReadOnly(String),
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub item: StartupItem,
    pub key: EntryKey,
}

/* ------------------------------------------------------------------ */
/* Desktop Entry                                                        */
/* ------------------------------------------------------------------ */

/// Chaves do grupo `[Desktop Entry]` (outros grupos, como `[Desktop Action]`,
/// têm `Exec`/`Name` próprios e são ignorados).
pub fn parse_desktop_entry(text: &str) -> HashMap<String, String> {
    let mut in_entry = false;
    let mut keys = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            keys.entry(key.trim().to_string())
                .or_insert_with(|| value.trim().to_string());
        }
    }
    keys
}

fn is_true(keys: &HashMap<String, String>, key: &str) -> bool {
    keys.get(key)
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

fn is_false(keys: &HashMap<String, String>, key: &str) -> bool {
    keys.get(key)
        .is_some_and(|v| v.eq_ignore_ascii_case("false"))
}

fn list_values(keys: &HashMap<String, String>, key: &str) -> Vec<String> {
    keys.get(key)
        .map(|value| {
            value
                .split(';')
                .map(|item| item.trim().to_lowercase())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A entrada roda na sessão com estes desktops (`$XDG_CURRENT_DESKTOP`)?
pub fn applies_to_desktop(keys: &HashMap<String, String>, desktops: &[String]) -> bool {
    let only = list_values(keys, "OnlyShowIn");
    if !only.is_empty() && !only.iter().any(|d| desktops.contains(d)) {
        return false;
    }
    let not = list_values(keys, "NotShowIn");
    !not.iter().any(|d| desktops.contains(d))
}

/// Ativo = não `Hidden` e sem `X-GNOME-Autostart-enabled=false`.
pub fn desktop_enabled(keys: &HashMap<String, String>) -> bool {
    !is_true(keys, "Hidden") && !is_false(keys, "X-GNOME-Autostart-enabled")
}

fn localized_name(keys: &HashMap<String, String>, fallback: &str) -> String {
    ["Name[pt_BR]", "Name[pt]", "Name"]
        .iter()
        .find_map(|key| keys.get(*key).filter(|v| !v.is_empty()))
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// Grava `key=value` no grupo `[Desktop Entry]`, preservando o resto do arquivo.
/// Se a chave não existir, entra logo depois do cabeçalho do grupo.
pub fn set_desktop_key(text: &str, key: &str, value: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_entry = false;
    let mut header_index = None;
    let mut replaced = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_entry = trimmed == "[Desktop Entry]";
            out.push(line.to_string());
            if in_entry && header_index.is_none() {
                header_index = Some(out.len() - 1);
            }
            continue;
        }
        if in_entry {
            if let Some((current, _)) = trimmed.split_once('=') {
                if current.trim() == key {
                    if !replaced {
                        out.push(format!("{key}={value}"));
                        replaced = true;
                    }
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if !replaced {
        match header_index {
            Some(index) => out.insert(index + 1, format!("{key}={value}")),
            None => {
                out.insert(0, format!("{key}={value}"));
                out.insert(0, "[Desktop Entry]".to_string());
            }
        }
    }
    let mut result = out.join("\n");
    result.push('\n');
    result
}

/* ------------------------------------------------------------------ */
/* Impacto                                                              */
/* ------------------------------------------------------------------ */

/// Desfaz o escape de nomes de unit (`\x2d` → `-`).
pub fn unescape_unit(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() && bytes[index + 1] == b'x' {
            let hex = std::str::from_utf8(&bytes[index + 2..index + 4]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Duração do `systemd-analyze blame` (`1min 2.5s`, `850ms`, `30.001s`) em ms.
pub fn parse_blame_duration(text: &str) -> Option<u64> {
    let mut total = 0f64;
    let mut any = false;
    for token in text.split_whitespace() {
        let (number, factor) = if let Some(v) = token.strip_suffix("ms") {
            (v, 1.0)
        } else if let Some(v) = token.strip_suffix("us") {
            (v, 0.001)
        } else if let Some(v) = token.strip_suffix("µs") {
            (v, 0.001)
        } else if let Some(v) = token.strip_suffix("min") {
            (v, 60_000.0)
        } else if let Some(v) = token.strip_suffix('h') {
            (v, 3_600_000.0)
        } else {
            (token.strip_suffix('s')?, 1000.0)
        };
        total += number.parse::<f64>().ok()? * factor;
        any = true;
    }
    any.then_some(total.round() as u64)
}

/// Linhas `  1.234s nome.service` → unit (sem escape) → ms.
pub fn parse_blame(output: &str) -> HashMap<String, u64> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let line = line.trim();
        let Some((duration, unit)) = line.rsplit_once(' ') else {
            continue;
        };
        if let Some(ms) = parse_blame_duration(duration) {
            map.entry(unescape_unit(unit.trim())).or_insert(ms);
        }
    }
    map
}

/// Mesmas faixas do Windows (`determine_impact`).
pub fn impact_from_ms(ms: u64) -> String {
    if ms >= 1000 {
        "Alto impacto".to_string()
    } else if ms >= 300 {
        "Médio impacto".to_string()
    } else {
        "Baixo impacto".to_string()
    }
}

/// Tempos de boot. `None` = `systemd-analyze` indisponível (impacto
/// "Indisponível", não "Baixo").
fn blame(user: bool) -> Option<HashMap<String, u64>> {
    let mut analyze = command::c_locale_command("systemd-analyze");
    if user {
        analyze.arg("--user");
    }
    analyze.args(["blame", "--no-pager"]);
    let output = command::run(analyze, COMMAND_TIMEOUT).ok()?;
    output.success().then(|| parse_blame(&output.stdout))
}

fn impact_for(unit: &str, durations: Option<&HashMap<String, u64>>) -> (String, Option<u64>) {
    match durations {
        None => (IMPACT_UNAVAILABLE.to_string(), None),
        Some(map) => match map.get(unit) {
            Some(ms) => (impact_from_ms(*ms), Some(*ms)),
            None => (IMPACT_NOT_MEASURED.to_string(), None),
        },
    }
}

/* ------------------------------------------------------------------ */
/* XDG Autostart                                                        */
/* ------------------------------------------------------------------ */

fn user_autostart_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("autostart"))
}

fn system_autostart_dirs() -> Vec<PathBuf> {
    let dirs = std::env::var("XDG_CONFIG_DIRS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "/etc/xdg".to_string());
    dirs.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join("autostart"))
        .collect()
}

fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty())
        .collect()
}

fn desktop_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "desktop") && path.is_file())
        .filter_map(|path| {
            let id = path.file_name()?.to_str()?.to_string();
            Some((id, path))
        })
        .collect();
    files.sort();
    files
}

/// Trechos de id/`Exec` de autostarts que sustentam a sessão gráfica: agente
/// do polkit (sem ele o `pkexec` não pede senha), shell do Plasma/GNOME,
/// carteira de senhas, energia, bandeja e acessibilidade. Desligar qualquer um
/// quebra a sessão no próximo login, então o Pulse só os exibe.
const CRITICAL_AUTOSTART_MARKERS: [&str; 16] = [
    "polkit",
    "plasmashell",
    "plasma-",
    "powerdevil",
    "kwallet",
    "ksmserver",
    "kwin",
    "at-spi",
    "gnome-keyring",
    "gnome-shell",
    "settingsdaemon",
    "settings-daemon",
    "xdg-desktop-portal",
    "xembedsniproxy",
    "gmenudbusmenuproxy",
    "pipewire",
];

pub fn is_critical_autostart(id: &str, exec: &str) -> bool {
    let id = id.to_lowercase();
    let exec = exec.to_lowercase();
    CRITICAL_AUTOSTART_MARKERS
        .iter()
        .any(|marker| id.contains(marker) || exec.contains(marker))
}

const CRITICAL_AUTOSTART_REASON: &str =
    "Componente essencial da sessão gráfica; o Pulse não o desativa";

/// Units XDG que o systemd gera: `app-<id sem .desktop>@autostart.service`.
fn xdg_unit_name(id: &str) -> String {
    format!("app-{}@autostart.service", id.trim_end_matches(".desktop"))
}

pub fn collect_xdg(
    user_dir: Option<&Path>,
    system_dirs: &[PathBuf],
    desktops: &[String],
    durations: Option<&HashMap<String, u64>>,
) -> Vec<Entry> {
    // id → caminho do sistema (o primeiro diretório de XDG_CONFIG_DIRS vence).
    let mut system: HashMap<String, PathBuf> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for dir in system_dirs {
        for (id, path) in desktop_files(dir) {
            if let std::collections::hash_map::Entry::Vacant(slot) = system.entry(id.clone()) {
                order.push(id);
                slot.insert(path);
            }
        }
    }
    let user: HashMap<String, PathBuf> = user_dir
        .map(desktop_files)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut user_ids: Vec<&String> = user.keys().filter(|id| !system.contains_key(*id)).collect();
    user_ids.sort();
    let mut ids: Vec<String> = user_ids.into_iter().cloned().collect();
    ids.extend(order);

    let mut entries = Vec::new();
    for id in ids {
        let user_path = user.get(&id).cloned();
        let system_path = system.get(&id).cloned();
        let Some(effective) = user_path.as_ref().or(system_path.as_ref()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(effective) else {
            continue;
        };
        let keys = parse_desktop_entry(&text);
        if keys.get("Type").is_some_and(|t| t != "Application") {
            continue;
        }
        if !applies_to_desktop(&keys, desktops) {
            continue;
        }
        let stem = id.trim_end_matches(".desktop");
        let name = localized_name(&keys, stem);
        let command = keys.get("Exec").cloned().unwrap_or_default();
        let is_pulse = id.eq_ignore_ascii_case(crate::startup::AUTOSTART_FILE_NAME)
            || name.eq_ignore_ascii_case("Pulse");
        let (impact, startup_time_ms) = impact_for(&xdg_unit_name(&id), durations);
        let origin = if system_path.is_some() {
            ORIGIN_XDG_SYSTEM
        } else {
            ORIGIN_XDG_USER
        };
        let reason = if is_pulse {
            Some(PULSE_MANAGED.to_string())
        } else if is_critical_autostart(&id, &command) {
            Some(CRITICAL_AUTOSTART_REASON.to_string())
        } else {
            None
        };
        let key = match &reason {
            Some(reason) => EntryKey::ReadOnly(reason.clone()),
            None => EntryKey::Xdg {
                id,
                system_path,
                user_path,
            },
        };
        entries.push(Entry {
            item: StartupItem {
                name,
                icon: crate::app_icons::icon_for_command(&command),
                command,
                origin: origin.to_string(),
                enabled: desktop_enabled(&keys),
                can_change: reason.is_none(),
                unavailable_reason: reason,
                impact,
                startup_time_ms,
            },
            key,
        });
    }
    entries
}

/// Liga/desliga um autostart XDG sem tocar no arquivo do sistema.
pub fn toggle_xdg(
    id: &str,
    system_path: Option<&Path>,
    user_path: Option<&Path>,
    user_dir: &Path,
    enabled: bool,
) -> Result<(), String> {
    let hidden = if enabled { "false" } else { "true" };
    let (source, target) = match (user_path, system_path) {
        (Some(user), _) => (user.to_path_buf(), user.to_path_buf()),
        (None, Some(system)) => {
            if enabled {
                // Sem cópia do usuário, o do sistema já está valendo.
                let text = std::fs::read_to_string(system)
                    .map_err(|e| format!("Não foi possível ler {}: {e}", system.display()))?;
                if desktop_enabled(&parse_desktop_entry(&text)) {
                    return Ok(());
                }
            }
            (system.to_path_buf(), user_dir.join(id))
        }
        (None, None) => return Err("Entrada de inicialização não localizada".to_string()),
    };
    let text = std::fs::read_to_string(&source)
        .map_err(|e| format!("Não foi possível ler {}: {e}", source.display()))?;
    let mut updated = set_desktop_key(&text, "Hidden", hidden);
    if enabled && is_false(&parse_desktop_entry(&updated), "X-GNOME-Autostart-enabled") {
        updated = set_desktop_key(&updated, "X-GNOME-Autostart-enabled", "true");
    }
    std::fs::create_dir_all(user_dir)
        .map_err(|e| format!("Não foi possível criar {}: {e}", user_dir.display()))?;
    std::fs::write(&target, updated)
        .map_err(|e| format!("Não foi possível gravar {}: {e}", target.display()))
}

/* ------------------------------------------------------------------ */
/* systemd e cron                                                       */
/* ------------------------------------------------------------------ */

/// `UNIT STATE PRESET` do `list-unit-files`. Mantém habilitadas e as
/// desabilitadas com preset "enabled" (alguém as desligou).
pub fn parse_unit_files(output: &str) -> Vec<(String, bool)> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let unit = fields.next()?;
            let state = fields.next()?;
            let preset = fields.next().unwrap_or("-");
            if unit.contains("@.") {
                return None;
            }
            match state {
                "enabled" | "enabled-runtime" => Some((unit.to_string(), true)),
                "disabled" if preset == "enabled" => Some((unit.to_string(), false)),
                _ => None,
            }
        })
        .collect()
}

fn list_unit_files(user: bool, types: &str, states: &str) -> Option<Vec<(String, bool)>> {
    let mut list = command::c_locale_command("systemctl");
    if user {
        list.arg("--user");
    }
    list.args([
        "list-unit-files",
        &format!("--type={types}"),
        &format!("--state={states}"),
        "--no-legend",
        "--no-pager",
    ]);
    let output = command::run(list, COMMAND_TIMEOUT).ok()?;
    output.success().then(|| parse_unit_files(&output.stdout))
}

/// Descrição e comando das units, numa chamada só.
fn unit_details(user: bool, units: &[String]) -> HashMap<String, (String, String)> {
    if units.is_empty() {
        return HashMap::new();
    }
    let mut show = command::c_locale_command("systemctl");
    if user {
        show.arg("--user");
    }
    show.args([
        "show",
        "--no-pager",
        "-p",
        "Id,Description,ExecStart,Unit",
        "--",
    ])
    .args(units);
    let Ok(output) = command::run(show, COMMAND_TIMEOUT) else {
        return HashMap::new();
    };
    let mut details = HashMap::new();
    for block in output.stdout.split("\n\n") {
        let mut id = None;
        let mut description = String::new();
        let mut exec = String::new();
        let mut triggers = String::new();
        for line in block.lines() {
            match line.split_once('=') {
                Some(("Id", v)) => id = Some(v.to_string()),
                Some(("Description", v)) => description = v.to_string(),
                Some(("ExecStart", v)) => {
                    exec = v
                        .split(';')
                        .find_map(|p| p.trim().strip_prefix("argv[]="))
                        .unwrap_or("")
                        .trim()
                        .to_string()
                }
                Some(("Unit", v)) => triggers = v.to_string(),
                _ => {}
            }
        }
        if let Some(id) = id {
            // Timer não tem comando próprio: mostra a unit que ele dispara.
            let command = if exec.is_empty() && !triggers.is_empty() {
                format!("dispara {triggers}")
            } else {
                exec
            };
            details.insert(id, (description, command));
        }
    }
    details
}

fn collect_units(
    user: bool,
    types: &str,
    states: &str,
    durations: Option<&HashMap<String, u64>>,
) -> Vec<Entry> {
    let Some(units) = list_unit_files(user, types, states) else {
        return Vec::new();
    };
    let names: Vec<String> = units.iter().map(|(name, _)| name.clone()).collect();
    let details = unit_details(user, &names);
    units
        .into_iter()
        .map(|(unit, enabled)| {
            let (description, command) = details.get(&unit).cloned().unwrap_or_default();
            let is_timer = unit.ends_with(".timer");
            let origin = match (user, is_timer) {
                (true, false) => ORIGIN_USER_UNIT,
                (true, true) => ORIGIN_USER_TIMER,
                (false, _) => ORIGIN_SYSTEM_TIMER,
            };
            let critical = crate::services::linux::is_critical_unit(&unit);
            let (can_change, reason, key) = if !user {
                let reason = "Timer do sistema: alterar exige administrador e o Pulse só o exibe"
                    .to_string();
                (false, Some(reason.clone()), EntryKey::ReadOnly(reason))
            } else if critical {
                let reason = "Unit essencial da sessão; o Pulse não a desativa".to_string();
                (false, Some(reason.clone()), EntryKey::ReadOnly(reason))
            } else {
                (true, None, EntryKey::UserUnit(unit.clone()))
            };
            let (impact, startup_time_ms) = if is_timer {
                // Timer dispara depois do boot; não há tempo de inicialização.
                (IMPACT_NOT_MEASURED.to_string(), None)
            } else {
                impact_for(&unit, durations)
            };
            let name = if description.is_empty() || description == unit {
                unit.clone()
            } else {
                format!("{description} ({unit})")
            };
            Entry {
                item: StartupItem {
                    name,
                    icon: None,
                    command,
                    origin: origin.to_string(),
                    enabled,
                    can_change,
                    unavailable_reason: reason,
                    impact,
                    startup_time_ms,
                },
                key,
            }
        })
        .collect()
}

/// Linhas do `crontab -l` que agendam algo (sem comentários nem variáveis).
pub fn parse_crontab(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| {
            // `VAR=valor` define ambiente; agendamento começa por @ ou campo de tempo.
            let first = line.split_whitespace().next().unwrap_or("");
            first.starts_with('@') || !first.contains('=')
        })
        .map(str::to_string)
        .collect()
}

fn collect_crontab() -> Vec<Entry> {
    let mut list = command::c_locale_command("crontab");
    list.arg("-l");
    let Ok(output) = command::run(list, COMMAND_TIMEOUT) else {
        return Vec::new();
    };
    // Código 1 com "no crontab for" = usuário sem crontab, não falha.
    if !output.success() {
        return Vec::new();
    }
    let reason = "Agendamento do crontab: só leitura no Pulse (edite com crontab -e)".to_string();
    parse_crontab(&output.stdout)
        .into_iter()
        .map(|line| Entry {
            item: StartupItem {
                name: line.chars().take(120).collect(),
                command: line,
                origin: ORIGIN_CRONTAB.to_string(),
                enabled: true,
                can_change: false,
                unavailable_reason: Some(reason.clone()),
                impact: IMPACT_NOT_MEASURED.to_string(),
                startup_time_ms: None,
                icon: None,
            },
            key: EntryKey::ReadOnly(reason.clone()),
        })
        .collect()
}

/* ------------------------------------------------------------------ */
/* API do módulo                                                        */
/* ------------------------------------------------------------------ */

pub fn collect() -> Vec<Entry> {
    let user_durations = blame(true);
    let system_durations = blame(false);
    let user_dir = user_autostart_dir();
    let mut entries = collect_xdg(
        user_dir.as_deref(),
        &system_autostart_dirs(),
        &current_desktops(),
        user_durations.as_ref(),
    );
    entries.extend(collect_units(
        true,
        "service,timer",
        "enabled,enabled-runtime,disabled",
        user_durations.as_ref(),
    ));
    entries.extend(collect_units(
        false,
        "timer",
        "enabled,enabled-runtime",
        system_durations.as_ref(),
    ));
    entries.extend(collect_crontab());

    let mut seen = HashSet::new();
    entries.retain(|entry| {
        seen.insert((
            entry.item.origin.to_lowercase(),
            entry.item.name.to_lowercase(),
        ))
    });
    entries
}

pub fn set_enabled(name: &str, origin: &str, enabled: bool) -> Result<(), String> {
    let entry = collect()
        .into_iter()
        .find(|entry| entry.item.name.eq_ignore_ascii_case(name) && entry.item.origin == origin)
        .ok_or_else(|| "Item de inicialização não localizado".to_string())?;
    if !entry.item.can_change {
        return Err(entry
            .item
            .unavailable_reason
            .unwrap_or_else(|| "Este item não pode ser alterado".to_string()));
    }
    match entry.key {
        EntryKey::Xdg {
            id,
            system_path,
            user_path,
        } => {
            let user_dir = user_autostart_dir()
                .ok_or_else(|| "Não foi possível localizar ~/.config/autostart".to_string())?;
            toggle_xdg(
                &id,
                system_path.as_deref(),
                user_path.as_deref(),
                &user_dir,
                enabled,
            )
        }
        EntryKey::UserUnit(unit) => {
            if crate::services::linux::is_critical_unit(&unit) {
                return Err("Unit essencial da sessão; o Pulse não a desativa".to_string());
            }
            let verb = if enabled { "enable" } else { "disable" };
            let mut toggle = command::c_locale_command("systemctl");
            toggle.args(["--user", verb, "--", unit.as_str()]);
            let output = command::run(toggle, COMMAND_TIMEOUT)?;
            if output.success() {
                Ok(())
            } else {
                Err(format!(
                    "O systemd recusou a alteração: {}",
                    output
                        .first_error_line()
                        .unwrap_or_else(|| format!("código {:?}", output.code))
                ))
            }
        }
        EntryKey::ReadOnly(reason) => Err(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pulse-autostart-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_only_the_desktop_entry_group() {
        let keys = parse_desktop_entry(
            "[Desktop Action new]\nExec=/x --new\nName=Nova\n\n[Desktop Entry]\nName=Code\nName[pt_BR]=Código\nExec=code %F\n",
        );
        assert_eq!(keys.get("Exec").map(String::as_str), Some("code %F"));
        assert_eq!(localized_name(&keys, "x"), "Código");
    }

    #[test]
    fn desktop_filters_follow_the_spec() {
        let kde = vec!["kde".to_string()];
        let only_cinnamon = parse_desktop_entry("[Desktop Entry]\nOnlyShowIn=X-Cinnamon;\n");
        assert!(!applies_to_desktop(&only_cinnamon, &kde));
        let not_kde = parse_desktop_entry("[Desktop Entry]\nNotShowIn=KDE;GNOME;\n");
        assert!(!applies_to_desktop(&not_kde, &kde));
        let any = parse_desktop_entry("[Desktop Entry]\nExec=x\n");
        assert!(applies_to_desktop(&any, &kde));
        assert!(is_critical_autostart(
            "polkit-kde-authentication-agent-1.desktop",
            "/usr/lib/x86_64-linux-gnu/libexec/polkit-kde-authentication-agent-1"
        ));
        assert!(is_critical_autostart(
            "org.kde.plasmashell.desktop",
            "plasmashell"
        ));
        assert!(!is_critical_autostart("code.desktop", "code %F"));
        assert!(!desktop_enabled(&parse_desktop_entry(
            "[Desktop Entry]\nHidden=true\n"
        )));
        assert!(!desktop_enabled(&parse_desktop_entry(
            "[Desktop Entry]\nX-GNOME-Autostart-enabled=false\n"
        )));
    }

    #[test]
    fn set_key_replaces_or_inserts_inside_entry_group() {
        let text = "[Desktop Action a]\nHidden=keep\n[Desktop Entry]\nName=X\nHidden=false\n";
        let updated = set_desktop_key(text, "Hidden", "true");
        assert!(updated.contains("[Desktop Action a]\nHidden=keep"));
        assert!(updated.contains("Name=X\nHidden=true"));
        let inserted = set_desktop_key("[Desktop Entry]\nName=X\n", "Hidden", "true");
        assert_eq!(inserted, "[Desktop Entry]\nHidden=true\nName=X\n");
    }

    #[test]
    fn disabling_system_entry_creates_user_copy_and_keeps_system_file() {
        let root = temp_dir("toggle");
        let system_dir = root.join("etc");
        let user_dir = root.join("user");
        std::fs::create_dir_all(&system_dir).unwrap();
        let system_file = system_dir.join("demo.desktop");
        let original = "[Desktop Entry]\nType=Application\nName=Demo\nExec=/usr/bin/demo\n";
        std::fs::write(&system_file, original).unwrap();

        let entries = collect_xdg(
            Some(&user_dir),
            std::slice::from_ref(&system_dir),
            &[],
            None,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].item.origin, ORIGIN_XDG_SYSTEM);
        assert!(entries[0].item.enabled);
        assert_eq!(entries[0].item.impact, IMPACT_UNAVAILABLE);

        toggle_xdg("demo.desktop", Some(&system_file), None, &user_dir, false).unwrap();
        assert_eq!(std::fs::read_to_string(&system_file).unwrap(), original);
        let entries = collect_xdg(
            Some(&user_dir),
            std::slice::from_ref(&system_dir),
            &[],
            None,
        );
        assert_eq!(
            entries.len(),
            1,
            "a cópia do usuário substitui, não duplica"
        );
        assert!(!entries[0].item.enabled);
        assert_eq!(entries[0].item.origin, ORIGIN_XDG_SYSTEM);

        let user_file = user_dir.join("demo.desktop");
        toggle_xdg(
            "demo.desktop",
            Some(&system_file),
            Some(&user_file),
            &user_dir,
            true,
        )
        .unwrap();
        let entries = collect_xdg(Some(&user_dir), &[system_dir], &[], None);
        assert!(entries[0].item.enabled);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_blame_and_unescapes_units() {
        let text = "7min 57.233s fstrim.service\n   850ms app-at\\x2dspi@autostart.service\n  1.5s claude.service\n 120us x.service\n";
        let map = parse_blame(text);
        assert_eq!(map.get("fstrim.service"), Some(&477_233));
        assert_eq!(map.get("app-at-spi@autostart.service"), Some(&850));
        assert_eq!(map.get("claude.service"), Some(&1500));
        assert_eq!(map.get("x.service"), Some(&0));
        assert_eq!(impact_from_ms(1500), "Alto impacto");
        assert_eq!(impact_from_ms(300), "Médio impacto");
        assert_eq!(impact_from_ms(10), "Baixo impacto");
    }

    #[test]
    fn parses_unit_files_and_crontab() {
        let units = parse_unit_files(
            "claude.service enabled enabled\nobex.service disabled enabled\nfoo.service disabled disabled\nbar@.service enabled enabled\nbaz.timer static -\n",
        );
        assert_eq!(
            units,
            vec![
                ("claude.service".to_string(), true),
                ("obex.service".to_string(), false)
            ]
        );
        let cron = parse_crontab(
            "# m h\nMAILTO=x@y\nSHELL=/bin/bash\n@reboot /home/u/a.sh\n*/5 * * * * /usr/bin/b\n",
        );
        assert_eq!(cron, vec!["@reboot /home/u/a.sh", "*/5 * * * * /usr/bin/b"]);
    }

    /// Coleta real nesta máquina: forma e sanidade.
    #[test]
    fn collects_real_startup_items() {
        let entries = collect();
        let mut keys = HashSet::new();
        for entry in &entries {
            assert!(!entry.item.name.is_empty());
            assert!(keys.insert((entry.item.origin.clone(), entry.item.name.to_lowercase())));
            if !entry.item.can_change {
                assert!(entry.item.unavailable_reason.is_some());
            }
        }
        eprintln!("itens de inicialização: {}", entries.len());
        for entry in entries.iter().take(60) {
            eprintln!(
                "{} | {} | ativo={} | {} | {:?}",
                entry.item.origin,
                entry.item.name,
                entry.item.enabled,
                entry.item.impact,
                entry.item.startup_time_ms
            );
        }
    }
}
