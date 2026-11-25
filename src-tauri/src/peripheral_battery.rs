//! Leitura local de bateria de periféricos conectados (mouse, teclado, fones,
//! controles e outros dispositivos sem fio).
//!
//! - Windows: SetupAPI (`DEVPKEY_Bluetooth_Battery`), WinRT Bluetooth LE GATT
//!   (`0x180F`/`0x2A19`), protocolo USB 2.4GHz / HID direto (Logitech HID++ 2.0,
//!   HyperX Wireless, Razer, SteelSeries, Corsair e DualSense/DualShock 4) e XInput.
//! - Linux: `/sys/class/power_supply/*` filtrado por `scope == Device`.
//! - macOS: `ioreg` para periféricos HID/Bluetooth.
//!
//! Princípio inviolável: nunca inventar `0%`. Quando um dispositivo não expõe
//! nível de bateria, `battery_percent` retorna `Capability { value: None, unavailable_reason }`.

use crate::performance::Capability;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
mod hid_windows;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

const CACHE_TTL: Duration = Duration::from_secs(30);
const MAX_ALIAS_CHARS: usize = 60;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeripheralKind {
    Mouse,
    Keyboard,
    Headset,
    Controller,
    Stylus,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeripheralTransport {
    BluetoothClassic,
    BluetoothLe,
    UsbHid,
    Xinput,
    Sysfs,
    Ioreg,
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeripheralBatteryDevice {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    pub kind: PeripheralKind,
    pub transport: PeripheralTransport,
    pub connected: bool,
    pub charging: Option<bool>,
    pub battery_percent: Capability<u8>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeripheralBatterySnapshot {
    pub devices: Vec<PeripheralBatteryDevice>,
    pub unavailable_reason: Option<String>,
    pub platform_note: Option<String>,
    pub collected_at_unix_ms: u64,
}

struct CachedSnapshot {
    collected_at: Instant,
    snapshot: PeripheralBatterySnapshot,
    hid_diagnostics: Vec<String>,
}

static CACHE: OnceLock<Mutex<Option<CachedSnapshot>>> = OnceLock::new();
static ALIASES_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CachedSnapshot>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn aliases_lock() -> &'static Mutex<()> {
    ALIASES_LOCK.get_or_init(|| Mutex::new(()))
}

fn aliases_path() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join("peripheral-aliases.json")
}

fn kinds_path() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join("peripheral-kinds.json")
}

fn load_kinds_from(path: &Path) -> BTreeMap<String, PeripheralKind> {
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, PeripheralKind>>(&raw).unwrap_or_default()
}

fn save_kinds_to(path: &Path, kinds: &BTreeMap<String, PeripheralKind>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Não foi possível criar a pasta de tipos: {err}"))?;
    }
    let payload = serde_json::to_string_pretty(kinds)
        .map_err(|err| format!("Falha ao serializar ícones de periféricos: {err}"))?;
    fs::write(path, payload)
        .map_err(|err| format!("Não foi possível salvar os ícones de periféricos: {err}"))
}

fn load_kinds() -> BTreeMap<String, PeripheralKind> {
    let _guard = aliases_lock().lock().unwrap_or_else(|e| e.into_inner());
    load_kinds_from(&kinds_path())
}

fn apply_kinds_to_snapshot(
    snapshot: &mut PeripheralBatterySnapshot,
    kinds: &BTreeMap<String, PeripheralKind>,
) {
    for device in &mut snapshot.devices {
        if let Some(&custom_kind) = kinds.get(&device.id) {
            device.kind = custom_kind;
        }
    }
}

/// Sanitiza um nome personalizado: remove caracteres de controle, apara espaços
/// e limita a 60 caracteres. String vazia retorna `None` (remove o alias).
pub(crate) fn sanitize_alias(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|ch| {
            if matches!(ch, '\r' | '\n' | '\t') {
                ' '
            } else {
                ch
            }
        })
        .filter(|ch| !ch.is_control())
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    let truncated: String = trimmed.chars().take(MAX_ALIAS_CHARS).collect();
    let final_name = truncated.trim_end().to_owned();
    if final_name.is_empty() {
        None
    } else {
        Some(final_name)
    }
}

fn load_aliases_from(path: &Path) -> BTreeMap<String, String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(parsed) = serde_json::from_str::<BTreeMap<String, String>>(&raw) else {
        return BTreeMap::new();
    };
    parsed
        .into_iter()
        .filter_map(|(id, alias)| {
            let clean_id = id.trim().to_owned();
            if clean_id.is_empty() {
                return None;
            }
            let clean_alias = sanitize_alias(&alias)?;
            Some((clean_id, clean_alias))
        })
        .collect()
}

fn save_aliases_to(path: &Path, aliases: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Não foi possível criar a pasta de aliases: {err}"))?;
    }
    let payload = serde_json::to_string_pretty(aliases)
        .map_err(|err| format!("Falha ao serializar aliases de periféricos: {err}"))?;
    fs::write(path, payload)
        .map_err(|err| format!("Não foi possível salvar os aliases de periféricos: {err}"))
}

fn load_aliases() -> BTreeMap<String, String> {
    let _guard = aliases_lock().lock().unwrap_or_else(|e| e.into_inner());
    load_aliases_from(&aliases_path())
}

fn update_alias_in_path(
    path: &Path,
    device_id: &str,
    alias: &str,
) -> Result<BTreeMap<String, String>, String> {
    let clean_id = device_id.trim();
    if clean_id.is_empty() {
        return Err("Identificador do dispositivo inválido.".to_owned());
    }
    let mut aliases = load_aliases_from(path);
    match sanitize_alias(alias) {
        Some(clean_alias) => {
            aliases.insert(clean_id.to_owned(), clean_alias);
        }
        None => {
            aliases.remove(clean_id);
        }
    }
    save_aliases_to(path, &aliases)?;
    Ok(aliases)
}

/// Aplica aliases aos dispositivos de um snapshot de forma idempotente,
/// preservando o nome original do hardware em `hardware_name` enquanto houver alias.
pub(crate) fn apply_aliases_to_snapshot(
    snapshot: &mut PeripheralBatterySnapshot,
    aliases: &BTreeMap<String, String>,
) {
    for device in &mut snapshot.devices {
        let original = device
            .hardware_name
            .clone()
            .unwrap_or_else(|| device.name.clone());
        if let Some(custom) = aliases.get(&device.id) {
            device.hardware_name = Some(original);
            device.custom_name = Some(custom.clone());
            device.name = custom.clone();
        } else {
            device.name = original;
            device.hardware_name = None;
            device.custom_name = None;
        }
    }
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn measured_percent(percent: u8) -> Capability<u8> {
    if percent <= 100 {
        Capability {
            value: Some(percent),
            unavailable_reason: None,
        }
    } else {
        Capability {
            value: None,
            unavailable_reason: Some(format!("Valor fora da faixa 0–100% ({percent}).")),
        }
    }
}

pub(crate) fn unavailable_percent(reason: impl Into<String>) -> Capability<u8> {
    Capability {
        value: None,
        unavailable_reason: Some(reason.into()),
    }
}

/// Classifica o tipo do periférico a partir do nome reportado pelo sistema.
pub fn classify_kind(name: &str) -> PeripheralKind {
    let lower = name.to_lowercase();

    if [
        "controller",
        "gamepad",
        "xbox",
        "dualsense",
        "dualshock",
        "wireless controller",
        "joy-con",
        "pro controller",
        "8bitdo",
        "controle",
        "joystick",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return PeripheralKind::Controller;
    }

    if [
        "headset",
        "headphone",
        "earbud",
        "earphone",
        "airpods",
        "beats",
        "buds",
        "pods",
        "fone",
        "audio",
        "hands-free",
        "handsfree",
        "wh-1000",
        "wf-1000",
        "qc35",
        "qc45",
        "quietcomfort",
        "arctis",
        "nova 5",
        "nova 7",
        "nova pro",
        "cloud ",
        "cloud ii",
        "cloud iii",
        "cloud alpha",
        "cloud flight",
        "cloud mix",
        "stinger",
        "barracuda",
        "blackshark",
        "kraken",
        "nari",
        "kaira",
        "thresher",
        "virtuoso",
        "void",
        "hs55",
        "hs65",
        "hs70",
        "hs75",
        "hs80",
        "g435",
        "g533",
        "g535",
        "g733",
        "g735",
        "g933",
        "g935",
        "g522",
        "pro x 2",
        "a30",
        "a50",
        "jbl ",
        "bose ",
        "sennheiser",
        "momentum",
        "edifier",
        "anker",
        "soundcore",
        "qcy",
        "jabra",
        "marshall",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return PeripheralKind::Headset;
    }

    if [
        "mouse",
        "mx master",
        "mx anywhere",
        "mx vertical",
        "lift",
        "deathadder",
        "viper",
        "basilisk",
        "cobra",
        "naga",
        "mamba",
        "orochi",
        "superlight",
        "g pro",
        "g304",
        "g305",
        "g502",
        "g602",
        "g603",
        "g604",
        "g703",
        "g903",
        "aerox",
        "rival",
        "prime wireless",
        "sensei",
        "harpoon",
        "katar",
        "dark core",
        "ironclaw",
        "sabre",
        "scimitar",
        "m65",
        "nightsabre",
        "pulsefire",
        "model o",
        "model d",
        "model i",
        "trackball",
        "trackpad",
        "magic trackpad",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return PeripheralKind::Mouse;
    }

    if [
        "keyboard",
        "teclado",
        "keychron",
        "mx keys",
        "mx mechanical",
        "g915",
        "g715",
        "g515",
        "pro x tkl",
        "pro x 60",
        "huntsman",
        "blackwidow",
        "deathstalker",
        "ornata",
        "apex",
        "anne pro",
        "hhkb",
        "nuphy",
        "magic keyboard",
        "k57",
        "k63",
        "k65",
        "k70",
        "k100",
        "alloy",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return PeripheralKind::Keyboard;
    }

    if ["pen", "stylus", "pencil", "caneta", "wacom", "surface slim"]
        .iter()
        .any(|kw| lower.contains(kw))
    {
        return PeripheralKind::Stylus;
    }

    PeripheralKind::Other
}

/// Remove duplicatas mantendo o registro mais informativo (com percentual
/// medido e conectado primeiro) e ordena por menor bateria para destacar alertas.
pub(crate) fn normalize_devices(
    mut devices: Vec<PeripheralBatteryDevice>,
) -> Vec<PeripheralBatteryDevice> {
    // Prioriza conectados, medições reais e estado de carga antes de deduplicar por nome.
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| {
                b.battery_percent
                    .value
                    .is_some()
                    .cmp(&a.battery_percent.value.is_some())
            })
            .then_with(|| b.charging.is_some().cmp(&a.charging.is_some()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let mut seen_keys = HashSet::new();
    let mut deduped = Vec::with_capacity(devices.len());
    for device in devices {
        let norm_name = normalize_name_key(&device.name);
        let key = if norm_name.is_empty() {
            device.id.to_lowercase()
        } else {
            norm_name
        };
        if seen_keys.insert(key) {
            deduped.push(device);
        }
    }

    // Ordenação final para exibição: conectados primeiro, depois menor bateria
    // primeiro (facilita identificar o que precisa carregar), indisponíveis no fim.
    deduped.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(
                || match (a.battery_percent.value, b.battery_percent.value) {
                    (Some(pa), Some(pb)) => pa.cmp(&pb),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                },
            )
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    deduped
}

fn normalize_name_key(name: &str) -> String {
    let trimmed = name
        .trim()
        .trim_end_matches(" Hands-Free AG Audio")
        .trim_end_matches(" Hands-Free")
        .trim_end_matches(" Stereo")
        .trim_end_matches(" Avrcp Transport")
        .trim();
    trimmed.to_lowercase()
}

fn collect_fresh() -> (PeripheralBatterySnapshot, Vec<String>) {
    #[cfg(windows)]
    {
        windows::collect_with_diagnostics()
    }
    #[cfg(target_os = "linux")]
    {
        (linux::collect(), Vec::new())
    }
    #[cfg(target_os = "macos")]
    {
        (macos::collect(), Vec::new())
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        (
            PeripheralBatterySnapshot {
                devices: Vec::new(),
                unavailable_reason: Some(
                    "Leitura de bateria de periféricos ainda não suportada nesta plataforma."
                        .to_owned(),
                ),
                platform_note: None,
                collected_at_unix_ms: now_unix_ms(),
            },
            Vec::new(),
        )
    }
}

fn get_snapshot_with_diagnostics(force_refresh: bool) -> (PeripheralBatterySnapshot, Vec<String>) {
    let aliases = load_aliases();
    let kinds = load_kinds();
    if !force_refresh {
        if let Ok(mut guard) = cache().lock() {
            if let Some(cached) = guard.as_mut() {
                if cached.collected_at.elapsed() < CACHE_TTL {
                    apply_aliases_to_snapshot(&mut cached.snapshot, &aliases);
                    apply_kinds_to_snapshot(&mut cached.snapshot, &kinds);
                    return (cached.snapshot.clone(), cached.hid_diagnostics.clone());
                }
            }
        }
    }

    let (mut snapshot, hid_diagnostics) = collect_fresh();
    apply_aliases_to_snapshot(&mut snapshot, &aliases);
    apply_kinds_to_snapshot(&mut snapshot, &kinds);
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CachedSnapshot {
            collected_at: Instant::now(),
            snapshot: snapshot.clone(),
            hid_diagnostics: hid_diagnostics.clone(),
        });
    }
    (snapshot, hid_diagnostics)
}

fn get_snapshot(force_refresh: bool) -> PeripheralBatterySnapshot {
    get_snapshot_with_diagnostics(force_refresh).0
}

#[tauri::command]
pub async fn set_peripheral_alias(
    device_id: String,
    alias: String,
) -> Result<PeripheralBatterySnapshot, String> {
    let started = Instant::now();
    let result: Result<PeripheralBatterySnapshot, String> =
        tauri::async_runtime::spawn_blocking(move || {
            let aliases = {
                let _guard = aliases_lock().lock().unwrap_or_else(|e| e.into_inner());
                update_alias_in_path(&aliases_path(), &device_id, &alias)?
            };
            let kinds = load_kinds();

            // Aplica imediatamente no cache atual sem re-escanear HID/BLE.
            let out = if let Ok(mut guard) = cache().lock() {
                if let Some(cached) = guard.as_mut() {
                    apply_aliases_to_snapshot(&mut cached.snapshot, &aliases);
                    apply_kinds_to_snapshot(&mut cached.snapshot, &kinds);
                    Some(cached.snapshot.clone())
                } else {
                    None
                }
            } else {
                None
            };

            #[cfg(windows)]
            crate::custom_dock::telemetry_feed::invalidate_peripherals();

            Ok(out.unwrap_or_else(|| get_snapshot(false)))
        })
        .await
        .map_err(|_| "Falha ao salvar o nome personalizado do periférico.".to_owned())?;

    let outcome = match &result {
        Ok(_) => Ok(()),
        Err(reason) => Err(reason.as_str()),
    };
    crate::debug::record(
        "peripheral_battery_alias",
        started.elapsed().as_millis(),
        outcome,
    );
    result
}

#[tauri::command]
pub async fn set_peripheral_kind(
    device_id: String,
    kind: Option<PeripheralKind>,
) -> Result<PeripheralBatterySnapshot, String> {
    let started = Instant::now();
    let result: Result<PeripheralBatterySnapshot, String> =
        tauri::async_runtime::spawn_blocking(move || {
            let clean_id = device_id.trim().to_owned();
            if clean_id.is_empty() {
                return Err("Identificador do dispositivo inválido.".to_owned());
            }
            let kinds = {
                let _guard = aliases_lock().lock().unwrap_or_else(|e| e.into_inner());
                let path = kinds_path();
                let mut map = load_kinds_from(&path);
                match kind {
                    Some(k) => {
                        map.insert(clean_id.clone(), k);
                    }
                    None => {
                        map.remove(&clean_id);
                    }
                }
                save_kinds_to(&path, &map)?;
                map
            };

            let out = if let Ok(mut guard) = cache().lock() {
                if let Some(cached) = guard.as_mut() {
                    apply_kinds_to_snapshot(&mut cached.snapshot, &kinds);
                    Some(cached.snapshot.clone())
                } else {
                    None
                }
            } else {
                None
            };

            #[cfg(windows)]
            crate::custom_dock::telemetry_feed::invalidate_peripherals();

            Ok(out.unwrap_or_else(|| get_snapshot(false)))
        })
        .await
        .map_err(|_| "Falha ao salvar o ícone personalizado do periférico.".to_owned())?;

    let outcome = match &result {
        Ok(_) => Ok(()),
        Err(reason) => Err(reason.as_str()),
    };
    crate::debug::record(
        "peripheral_battery_kind",
        started.elapsed().as_millis(),
        outcome,
    );
    result
}

#[tauri::command]
pub async fn get_peripheral_batteries() -> PeripheralBatterySnapshot {
    let started = Instant::now();
    let snapshot = tauri::async_runtime::spawn_blocking(|| get_snapshot(false))
        .await
        .unwrap_or_else(|_| PeripheralBatterySnapshot {
            devices: Vec::new(),
            unavailable_reason: Some(
                "Falha ao executar a coleta de periféricos em segundo plano.".to_owned(),
            ),
            platform_note: None,
            collected_at_unix_ms: now_unix_ms(),
        });
    let outcome = match snapshot.unavailable_reason.as_deref() {
        Some(reason) => Err(reason),
        None => Ok(()),
    };
    crate::debug::record("peripheral_battery", started.elapsed().as_millis(), outcome);
    snapshot
}

#[tauri::command]
pub async fn refresh_peripheral_batteries() -> PeripheralBatterySnapshot {
    let started = Instant::now();
    let snapshot = tauri::async_runtime::spawn_blocking(|| get_snapshot(true))
        .await
        .unwrap_or_else(|_| PeripheralBatterySnapshot {
            devices: Vec::new(),
            unavailable_reason: Some(
                "Falha ao atualizar a leitura de periféricos em segundo plano.".to_owned(),
            ),
            platform_note: None,
            collected_at_unix_ms: now_unix_ms(),
        });
    let outcome = match snapshot.unavailable_reason.as_deref() {
        Some(reason) => Err(reason),
        None => Ok(()),
    };
    crate::debug::record(
        "peripheral_battery_refresh",
        started.elapsed().as_millis(),
        outcome,
    );
    snapshot
}

#[tauri::command]
pub async fn export_peripheral_diagnostics() -> String {
    let started = Instant::now();
    let report = tauri::async_runtime::spawn_blocking(|| {
        let (snapshot, hid_diagnostics) = get_snapshot_with_diagnostics(true);
        let mut lines = Vec::new();
        lines.push(format!(
            "Pulse — Diagnóstico de Bateria de Periféricos (SO: {}, {} dispositivo(s))",
            std::env::consts::OS,
            snapshot.devices.len()
        ));
        if let Some(note) = &snapshot.platform_note {
            lines.push(format!("Nota: {note}"));
        }
        if let Some(reason) = &snapshot.unavailable_reason {
            lines.push(format!("Indisponibilidade geral: {reason}"));
        }
        for device in &snapshot.devices {
            let pct = match (
                &device.battery_percent.value,
                &device.battery_percent.unavailable_reason,
            ) {
                (Some(v), _) => format!("{v}%"),
                (None, Some(r)) => format!("Indisponível ({r})"),
                (None, None) => "Indisponível".to_owned(),
            };
            let charging = match device.charging {
                Some(true) => "carregando",
                Some(false) => "descarregando",
                None => "estado de carga desconhecido",
            };
            let display_name = match &device.hardware_name {
                Some(hw) if hw != &device.name => format!("{} (hardware: {hw})", device.name),
                _ => device.name.clone(),
            };
            lines.push(format!(
                "- {} [{:?} | {:?}] conectado={} | bateria={} | {} | fonte={}",
                display_name,
                device.kind,
                device.transport,
                device.connected,
                pct,
                charging,
                device.source
            ));
        }
        if snapshot.devices.is_empty() {
            lines.push(
                "- Nenhum periférico sem fio com telemetria de bateria detectado.".to_owned(),
            );
        }
        if !hid_diagnostics.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "Interfaces HID inspecionadas ({}):",
                hid_diagnostics.len()
            ));
            for entry in &hid_diagnostics {
                lines.push(format!("  - {entry}"));
            }
        }
        lines.join("\n")
    })
    .await
    .unwrap_or_else(|_| "Não foi possível gerar o diagnóstico de periféricos.".to_owned());

    crate::debug::record(
        "peripheral_battery_diagnostics",
        started.elapsed().as_millis(),
        Ok(()),
    );
    report
}

#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod hid_protocol {
    use super::PeripheralKind;

    pub const LOGITECH_VID: u16 = 0x046D;
    pub const HP_VID: u16 = 0x03F0;
    pub const KINGSTON_VID: u16 = 0x0951;
    pub const RAZER_VID: u16 = 0x1532;
    pub const STEELSERIES_VID: u16 = 0x1038;
    pub const CORSAIR_VID: u16 = 0x1B1C;
    pub const SONY_VID: u16 = 0x054C;

    pub const HIDPP_SW_ID: u8 = 0x0D;
    pub const HIDPP_FEAT_ROOT: u16 = 0x0000;
    pub const HIDPP_FEAT_DEVICE_NAME: u16 = 0x0005;
    pub const HIDPP_FEAT_BATTERY_STATUS: u16 = 0x1000;
    pub const HIDPP_FEAT_BATTERY_VOLTAGE: u16 = 0x1001;
    pub const HIDPP_FEAT_UNIFIED_BATTERY: u16 = 0x1004;

    pub const HIDPP_ERR_UNKNOWN_DEVICE: u8 = 0x08;
    pub const HIDPP_ERR_RESOURCE_ERROR: u8 = 0x09;
    pub const HIDPP_ERR_CONNECTION_FAILED: u8 = 0x0B;

    /// Curva de tensão de descarga Li-ion do driver `hid-logitech-hidpp.c` (100% -> 1% em mV).
    const HIDPP_VOLTAGE_CURVE: [u16; 100] = [
        4186, 4156, 4143, 4133, 4122, 4113, 4103, 4094, 4086, 4075, 4067, 4059, 4051, 4043, 4035,
        4027, 4019, 4011, 4003, 3997, 3989, 3983, 3976, 3969, 3961, 3955, 3949, 3942, 3935, 3929,
        3922, 3916, 3909, 3902, 3896, 3890, 3883, 3877, 3870, 3865, 3859, 3853, 3848, 3842, 3837,
        3833, 3828, 3824, 3819, 3815, 3811, 3808, 3804, 3800, 3797, 3793, 3790, 3787, 3784, 3781,
        3778, 3775, 3772, 3770, 3767, 3764, 3762, 3759, 3757, 3754, 3751, 3748, 3744, 3741, 3737,
        3734, 3730, 3726, 3724, 3720, 3717, 3714, 3710, 3706, 3702, 3697, 3693, 3688, 3683, 3677,
        3671, 3666, 3662, 3658, 3654, 3646, 3633, 3612, 3579, 3537,
    ];

    const STINGER_CORE_CHARGING_CURVE_V: [f32; 11] = [
        3.4000, 3.8309, 3.8707, 3.9162, 3.9299, 3.9456, 3.9674, 4.0000, 4.0497, 4.0900, 4.1300,
    ];

    const STINGER_CORE_DISCHARGING_CURVE_V: [f32; 11] = [
        3.5174, 3.6508, 3.6998, 3.7381, 3.7633, 3.7847, 3.8381, 3.9052, 3.9542, 4.0423, 4.1205,
    ];

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct HidBatteryReading {
        pub percent: Option<u8>,
        pub charging: Option<bool>,
        pub connected: bool,
        pub unavailable_reason: Option<String>,
        pub discrete_note: Option<&'static str>,
    }

    impl HidBatteryReading {
        pub fn measured(percent: u8, charging: Option<bool>) -> Self {
            Self {
                percent: Some(percent.min(100)),
                charging,
                connected: true,
                unavailable_reason: None,
                discrete_note: None,
            }
        }

        pub fn measured_discrete(percent: u8, charging: Option<bool>, note: &'static str) -> Self {
            Self {
                percent: Some(percent.min(100)),
                charging,
                connected: true,
                unavailable_reason: None,
                discrete_note: Some(note),
            }
        }

        pub fn disconnected(reason: impl Into<String>) -> Self {
            Self {
                percent: None,
                charging: None,
                connected: false,
                unavailable_reason: Some(reason.into()),
                discrete_note: None,
            }
        }

        pub fn unavailable(charging: Option<bool>, reason: impl Into<String>) -> Self {
            Self {
                percent: None,
                charging,
                connected: true,
                unavailable_reason: Some(reason.into()),
                discrete_note: None,
            }
        }

        pub fn into_capability(self) -> (bool, Option<bool>, super::Capability<u8>) {
            let cap = match (self.percent, self.unavailable_reason) {
                (Some(v), _) => super::measured_percent(v),
                (None, Some(reason)) => super::unavailable_percent(reason),
                (None, None) => {
                    super::unavailable_percent("Leitura de bateria HID indisponível no momento.")
                }
            };
            (self.connected, self.charging, cap)
        }
    }

    pub fn anonymize_hid_path_label(path_lower: &str) -> String {
        let mut parts = Vec::new();
        for seg in path_lower.split('#').nth(1).unwrap_or("").split('&') {
            if seg.starts_with("mi_") || seg.starts_with("col") {
                parts.push(seg.to_ascii_uppercase());
            }
        }
        if parts.is_empty() {
            "HID".to_owned()
        } else {
            parts.join("&")
        }
    }

    pub fn normalize_vendor_prefix(brand: &str, raw: &str) -> String {
        let cleaned = raw
            .trim()
            .replace(" USB Receiver", "")
            .replace(" Receiver", "")
            .replace(" Wireless Dongle", "")
            .replace(" Dongle", "")
            .replace(" Transceiver", "")
            .trim()
            .to_owned();
        if cleaned.is_empty() {
            return brand.to_owned();
        }
        if cleaned
            .to_ascii_lowercase()
            .contains(&brand.to_ascii_lowercase())
        {
            cleaned
        } else {
            format!("{brand} {cleaned}")
        }
    }

    // -----------------------------------------------------------------------
    // Logitech HID++ 2.0
    // -----------------------------------------------------------------------

    pub fn is_logitech_hidpp_collection(
        usage_page: u16,
        _usage: u16,
        input_len: u16,
        output_len: u16,
    ) -> bool {
        matches!(usage_page, 0xFF00 | 0xFF43) && input_len >= 7 && output_len >= 7
    }

    pub fn logitech_collection_priority(
        usage_page: u16,
        usage: u16,
        input_len: u16,
        output_len: u16,
    ) -> u8 {
        if output_len == 20 && input_len == 20 {
            if matches!((usage_page, usage), (0xFF00, 0x0002) | (0xFF43, 0x0202)) {
                0
            } else {
                1
            }
        } else if output_len >= 64 && input_len >= 64 {
            2
        } else {
            3
        }
    }

    /// Gera chave física do receptor/dispositivo preservando a instância USB, mas
    /// agrupando coleções irmãs (`Col01`, `Col02`, `Col03`) no mesmo dispositivo.
    pub fn logitech_hidpp_receiver_key(path: &str, vid: u16, pid: u16) -> String {
        let lower = path.to_ascii_lowercase();
        let segments: Vec<&str> = lower.split('#').collect();
        if segments.len() >= 3 {
            let hw: Vec<&str> = segments[1]
                .split('&')
                .filter(|part| !part.starts_with("col"))
                .collect();
            let instance = segments[2]
                .rsplit_once('&')
                .map(|(prefix, _)| prefix)
                .unwrap_or(segments[2]);
            return format!("{vid:04x}:{pid:04x}:{}:{instance}", hw.join("&"));
        }
        format!("{vid:04x}:{pid:04x}:{lower}")
    }

    pub fn logitech_pid_fallback_name(pid: u16) -> Option<&'static str> {
        match pid {
            0xC53F => Some("Dispositivo Logitech LIGHTSPEED"),
            0xC539 | 0xC53A | 0xC53D | 0xC541 | 0xC545 | 0xC547 => {
                Some("Dispositivo Logitech LIGHTSPEED")
            }
            0xC52B | 0xC532 | 0xC534 => Some("Dispositivo Logitech Unifying"),
            0xC548 => Some("Dispositivo Logitech Bolt"),
            0x0AA7 => Some("Logitech G PRO Wireless Headset"),
            0x0AAA => Some("Logitech G533 Gaming Headset"),
            0x0ABA => Some("Logitech G733 Gaming Headset"),
            0x0AFB | 0x0AFC => Some("Logitech G535 / G435 Headset"),
            0x0A66 => Some("Logitech G933 / G935 Headset"),
            _ => None,
        }
    }

    fn hidpp_report_layout(report_len: usize) -> Option<(u8, usize)> {
        match report_len {
            7 => Some((0x10, 7)),
            8..=63 => Some((0x11, report_len.max(20))),
            n if n >= 64 => Some((0x12, n)),
            _ => None,
        }
    }

    pub fn build_hidpp_request(
        report_len: usize,
        dev_idx: u8,
        feat_idx: u8,
        func_id: u8,
        sw_id: u8,
        params: &[u8],
    ) -> Option<Vec<u8>> {
        let (report_id, frame_len) = hidpp_report_layout(report_len)?;
        let mut buf = vec![0u8; frame_len];
        buf[0] = report_id;
        buf[1] = dev_idx;
        buf[2] = feat_idx;
        buf[3] = ((func_id & 0x0F) << 4) | (sw_id & 0x0F);
        let max_params = frame_len.saturating_sub(4);
        let copy_len = params.len().min(max_params);
        buf[4..4 + copy_len].copy_from_slice(&params[..copy_len]);
        Some(buf)
    }

    pub fn build_hidpp10_pairing_name_request(report_len: usize, slot: u8) -> Option<Vec<u8>> {
        if !(1..=6).contains(&slot) {
            return None;
        }
        let (report_id, frame_len) = hidpp_report_layout(report_len)?;
        let mut buf = vec![0u8; frame_len];
        buf[0] = report_id;
        buf[1] = 0xFF;
        buf[2] = 0x83;
        buf[3] = 0xB5;
        buf[4] = 0x40 + (slot - 1);
        Some(buf)
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum HidppReply<'a> {
        Data(&'a [u8]),
        Error(u8),
        Ignore,
    }

    pub fn classify_hidpp_reply<'a>(
        buf: &'a [u8],
        dev_idx: u8,
        feat_idx: u8,
        func_id: u8,
        sw_id: u8,
    ) -> HidppReply<'a> {
        if buf.len() < 7 || !matches!(buf[0], 0x10..=0x12) || buf[1] != dev_idx {
            return HidppReply::Ignore;
        }
        let expected_func_sw = ((func_id & 0x0F) << 4) | (sw_id & 0x0F);
        if buf[2] == 0xFF && buf[3] == feat_idx && buf[4] == expected_func_sw {
            return HidppReply::Error(buf[5]);
        }
        if buf[2] == 0x8F && buf[3] == feat_idx && (buf[4] & 0x0F) == (sw_id & 0x0F) {
            return HidppReply::Error(buf[5]);
        }
        if buf[2] == feat_idx && buf[3] == expected_func_sw {
            return HidppReply::Data(buf);
        }
        HidppReply::Ignore
    }

    pub fn parse_hidpp10_pairing_name(resp: &[u8], slot: u8) -> Option<String> {
        if !(1..=6).contains(&slot) || resp.len() < 7 {
            return None;
        }
        if !matches!(resp[0], 0x11 | 0x12)
            || resp[1] != 0xFF
            || resp[2] != 0x83
            || resp[3] != 0xB5
            || resp[4] != 0x40 + (slot - 1)
        {
            return None;
        }
        let declared_len = (resp[5] as usize).min(resp.len().saturating_sub(6)).min(14);
        if declared_len == 0 {
            return None;
        }
        let slice = &resp[6..6 + declared_len];
        let trimmed_len = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        let text = String::from_utf8_lossy(&slice[..trimmed_len])
            .trim()
            .to_owned();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn parse_hidpp_ping_ok(resp: &[u8], nonce: u8) -> bool {
        resp.len() >= 7 && resp[4] >= 2 && resp[6] == nonce
    }

    pub fn parse_hidpp_root_feature(resp: &[u8]) -> Option<u8> {
        if resp.len() < 5 || resp[4] == 0 {
            return None;
        }
        let feat_type = resp.get(5).copied().unwrap_or(0);
        if (feat_type & 0x80) != 0 {
            return None;
        }
        Some(resp[4])
    }

    pub fn parse_hidpp_name_length(resp: &[u8]) -> Option<usize> {
        let len = (*resp.get(4)? as usize).min(64);
        (len > 0).then_some(len)
    }

    pub fn parse_hidpp_name_chunk(resp: &[u8], remaining: usize) -> &[u8] {
        if resp.len() <= 4 || remaining == 0 {
            return &[];
        }
        let max_payload = if resp[0] == 0x10 { 3 } else { 16 };
        let available = resp.len().saturating_sub(4).min(max_payload).min(remaining);
        let slice = &resp[4..4 + available];
        let nul_pos = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        &slice[..nul_pos]
    }

    pub fn parse_hidpp_device_type(resp: &[u8]) -> Option<PeripheralKind> {
        match resp.get(4).copied()? {
            0 | 2 => Some(PeripheralKind::Keyboard),
            3..=5 => Some(PeripheralKind::Mouse),
            8 => Some(PeripheralKind::Headset),
            11 | 12 => Some(PeripheralKind::Controller),
            _ => None,
        }
    }

    pub fn parse_hidpp_unified_battery(resp: &[u8], soc_supported: bool) -> HidBatteryReading {
        if resp.len() < 7 {
            return HidBatteryReading::unavailable(None, "Resposta HID++ 0x1004 incompleta.");
        }
        let pct_raw = resp[4];
        let level_flags = resp[5];
        let chg_status = resp[6];

        if chg_status == 4 {
            return HidBatteryReading::unavailable(
                None,
                "O periférico reportou erro de carregamento (HID++ 0x1004).",
            );
        }

        let charging = match chg_status {
            1 | 2 => Some(true),
            0 | 3 => Some(false),
            _ => None,
        };

        if !soc_supported {
            let level_desc = if (level_flags & 0x08) != 0 {
                "Cheio"
            } else if (level_flags & 0x04) != 0 {
                "Bom"
            } else if (level_flags & 0x02) != 0 {
                "Baixo"
            } else if (level_flags & 0x01) != 0 {
                "Crítico"
            } else {
                return HidBatteryReading::unavailable(
                    charging,
                    "Dispositivo HID++ 0x1004 sem leitura percentual disponível.",
                );
            };
            return HidBatteryReading::unavailable(
                charging,
                format!(
                    "O periférico expõe apenas faixa discreta ({level_desc}), sem percentual exato (HID++ 0x1004)."
                ),
            );
        }

        if pct_raw == 0 && chg_status == 3 {
            return HidBatteryReading::measured(100, charging);
        }

        if pct_raw == 0 && level_flags == 0 && chg_status == 0 {
            return HidBatteryReading::unavailable(
                charging,
                "Sensor HID++ 0x1004 sem medição válida (resposta zerada).",
            );
        }

        if pct_raw > 100 {
            return HidBatteryReading::unavailable(
                charging,
                format!("Percentual HID++ 0x1004 fora da faixa ({pct_raw}%)."),
            );
        }

        HidBatteryReading::measured(pct_raw, charging)
    }

    pub fn parse_hidpp_battery_status(resp: &[u8]) -> HidBatteryReading {
        if resp.len() < 7 {
            return HidBatteryReading::unavailable(None, "Resposta HID++ 0x1000 incompleta.");
        }
        let pct = resp[4];
        let status = resp[6];

        match status {
            3 => HidBatteryReading::measured(
                if (1..=100).contains(&pct) { pct } else { 100 },
                Some(false),
            ),
            1 | 2 | 4 => {
                if (1..=100).contains(&pct) {
                    HidBatteryReading::measured(pct, Some(true))
                } else {
                    HidBatteryReading::unavailable(
                        Some(true),
                        "Carregando via USB; percentual exato não reportado durante carga (HID++ 0x1000).",
                    )
                }
            }
            0 => {
                if (1..=100).contains(&pct) {
                    HidBatteryReading::measured(pct, Some(false))
                } else {
                    HidBatteryReading::unavailable(
                        Some(false),
                        "O dispositivo HID++ 0x1000 retornou nível 0/inválido fora de carga.",
                    )
                }
            }
            _ => HidBatteryReading::unavailable(
                None,
                format!("Estado de bateria inválido/erro (código {status}) no HID++ 0x1000."),
            ),
        }
    }

    pub fn parse_hidpp_battery_voltage(resp: &[u8]) -> HidBatteryReading {
        if resp.len() < 7 {
            return HidBatteryReading::unavailable(None, "Resposta HID++ 0x1001 incompleta.");
        }
        let mv = u16::from_be_bytes([resp[4], resp[5]]);
        let flags = resp[6];
        let ext_power = (flags & 0x80) != 0;
        let chg_sub = flags & 0x07;

        if ext_power && chg_sub == 3 {
            return HidBatteryReading::unavailable(
                None,
                "Erro térmico ou de bateria reportado em HID++ 0x1001.",
            );
        }
        if !(2500..=4900).contains(&mv) {
            return HidBatteryReading::unavailable(
                None,
                format!("Tensão HID++ 0x1001 fora da faixa esperada ({mv} mV)."),
            );
        }

        let charging = if ext_power {
            match chg_sub {
                0 => Some(true),
                1 | 2 => Some(false),
                _ => None,
            }
        } else {
            Some(false)
        };

        if ext_power && chg_sub == 1 {
            return HidBatteryReading::measured(100, charging);
        }

        let pct = HIDPP_VOLTAGE_CURVE
            .iter()
            .position(|&threshold| mv >= threshold)
            .map_or(0, |idx| (100 - idx) as u8);
        HidBatteryReading::measured_discrete(pct, charging, "curva mV 0x1001")
    }

    // -----------------------------------------------------------------------
    // HyperX Wireless (HP 0x03F0 / Kingston 0x0951)
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum HyperXProtocol {
        StingerCoreHp,
        StingerCoreKingston,
        Cloud2Hp,
        Cloud2Kingston,
        CloudFlightS,
        CloudAlpha,
        CloudFlight,
        Cloud3,
        Cloud3S,
        Mix2,
        Stinger3,
        CloudAlpha2,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct HyperXModelSpec {
        pub vid: u16,
        pub pid: u16,
        pub name: &'static str,
        pub protocol: HyperXProtocol,
    }

    const HYPERX_MODELS: &[HyperXModelSpec] = &[
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0A8E,
            name: "HyperX Cloud Stinger Core Wireless DTS",
            protocol: HyperXProtocol::StingerCoreHp,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0A93,
            name: "HyperX Cloud Stinger Core Wireless (PS)",
            protocol: HyperXProtocol::StingerCoreHp,
        },
        HyperXModelSpec {
            vid: KINGSTON_VID,
            pid: 0x170B,
            name: "HyperX Cloud Stinger Core Wireless + 7.1",
            protocol: HyperXProtocol::StingerCoreKingston,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0696,
            name: "HyperX Cloud II Wireless",
            protocol: HyperXProtocol::Cloud2Hp,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x018B,
            name: "HyperX Cloud II Wireless",
            protocol: HyperXProtocol::Cloud2Hp,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0D93,
            name: "HyperX Cloud Stinger 2 Wireless",
            protocol: HyperXProtocol::Cloud2Hp,
        },
        HyperXModelSpec {
            vid: KINGSTON_VID,
            pid: 0x1718,
            name: "HyperX Cloud II Wireless",
            protocol: HyperXProtocol::Cloud2Kingston,
        },
        HyperXModelSpec {
            vid: KINGSTON_VID,
            pid: 0x16EA,
            name: "HyperX Cloud Flight S",
            protocol: HyperXProtocol::CloudFlightS,
        },
        HyperXModelSpec {
            vid: KINGSTON_VID,
            pid: 0x16EB,
            name: "HyperX Cloud Flight S",
            protocol: HyperXProtocol::CloudFlightS,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x098D,
            name: "HyperX Cloud Alpha Wireless",
            protocol: HyperXProtocol::CloudAlpha,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x048D,
            name: "HyperX Cloud Alpha Wireless",
            protocol: HyperXProtocol::CloudAlpha,
        },
        HyperXModelSpec {
            vid: KINGSTON_VID,
            pid: 0x16C4,
            name: "HyperX Cloud Flight Wireless",
            protocol: HyperXProtocol::CloudFlight,
        },
        HyperXModelSpec {
            vid: KINGSTON_VID,
            pid: 0x1723,
            name: "HyperX Cloud Flight Wireless",
            protocol: HyperXProtocol::CloudFlight,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0E90,
            name: "HyperX Cloud Flight Wireless",
            protocol: HyperXProtocol::CloudFlight,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x05B7,
            name: "HyperX Cloud III Wireless",
            protocol: HyperXProtocol::Cloud3,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0FCC,
            name: "HyperX Cloud III Wireless",
            protocol: HyperXProtocol::Cloud3,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0995,
            name: "HyperX Cloud 2 Core Wireless",
            protocol: HyperXProtocol::Cloud3,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x06BE,
            name: "HyperX Cloud III S Wireless",
            protocol: HyperXProtocol::Cloud3S,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x02CC,
            name: "HyperX Cloud III S Wireless",
            protocol: HyperXProtocol::Cloud3S,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0FAE,
            name: "HyperX Cloud Mix 2",
            protocol: HyperXProtocol::Mix2,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x01C6,
            name: "HyperX Cloud Stinger 3 Wireless",
            protocol: HyperXProtocol::Stinger3,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x03C6,
            name: "HyperX Cloud Stinger 3 Wireless",
            protocol: HyperXProtocol::Stinger3,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0DC5,
            name: "HyperX Cloud Stinger 3 Wireless",
            protocol: HyperXProtocol::Stinger3,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x08BE,
            name: "HyperX Cloud Alpha 2 Wireless",
            protocol: HyperXProtocol::CloudAlpha2,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0AC1,
            name: "HyperX Cloud Flight 2",
            protocol: HyperXProtocol::CloudAlpha2,
        },
        HyperXModelSpec {
            vid: HP_VID,
            pid: 0x0CC1,
            name: "HyperX Cloud Flight 2",
            protocol: HyperXProtocol::CloudAlpha2,
        },
    ];

    pub fn lookup_hyperx_model(vid: u16, pid: u16) -> Option<HyperXModelSpec> {
        HYPERX_MODELS
            .iter()
            .copied()
            .find(|m| m.vid == vid && m.pid == pid)
    }

    pub fn hyperx_accepts_collection(
        spec: HyperXModelSpec,
        usage_page: u16,
        usage: u16,
        input_len: u16,
        output_len: u16,
        feature_len: u16,
    ) -> bool {
        match spec.protocol {
            HyperXProtocol::StingerCoreHp | HyperXProtocol::StingerCoreKingston => {
                (usage_page == 0xFF00 && usage == 0x0001 && feature_len >= 20)
                    || (usage_page >= 0xFF00 && feature_len >= 32)
            }
            HyperXProtocol::Cloud2Hp => {
                ((usage_page == 0xFF90 && usage == 0x0303) || usage_page >= 0xFF00)
                    && input_len >= 8
                    && output_len >= 4
            }
            HyperXProtocol::Cloud3S | HyperXProtocol::Mix2 | HyperXProtocol::Stinger3 => {
                (usage_page == 0x01C0 || usage_page >= 0xFF00) && input_len >= 8 && output_len >= 6
            }
            HyperXProtocol::CloudAlpha2 => {
                usage_page >= 0xFF00 && input_len >= 16 && output_len >= 16
            }
            _ => usage_page >= 0xFF00 && input_len >= 4 && output_len >= 2,
        }
    }

    pub fn build_stinger_core_conn_feature(report_len: usize) -> Vec<u8> {
        const CMD: [u8; 20] = [
            0xFF, 0x07, 0x00, 0xFD, 0x04, 0x00, 0x00, 0x02, 0x82, 0x03, 0x05, 0x29, 0x0F, 0x00,
            0x00, 0x03, 0x68, 0x10, 0x00, 0x00,
        ];
        let mut buf = vec![0u8; report_len.max(CMD.len())];
        buf[..CMD.len()].copy_from_slice(&CMD);
        buf
    }

    pub fn parse_stinger_core_conn_response(buf: &[u8]) -> Option<bool> {
        if buf.len() < 12 || buf[0] != 0xFF {
            return None;
        }
        Some(buf[11] == 0x01)
    }

    pub fn build_stinger_core_batt_feature(report_len: usize) -> Vec<u8> {
        const CMD: [u8; 12] = [
            0xFF, 0x07, 0x00, 0xFD, 0x04, 0x0C, 0xF1, 0x02, 0x01, 0x04, 0xF0, 0x0C,
        ];
        let mut buf = vec![0u8; report_len.max(CMD.len())];
        buf[..CMD.len()].copy_from_slice(&CMD);
        buf
    }

    fn interpolate_voltage_knots(voltage_mv: i16, knots_v: &[f32; 11]) -> u8 {
        let v = voltage_mv as f32 / 1000.0;
        if v <= knots_v[0] {
            return 0;
        }
        if v >= knots_v[10] {
            return 100;
        }
        for (i, pair) in knots_v.windows(2).enumerate() {
            if v <= pair[1] {
                let span = pair[1] - pair[0];
                let frac = if span > f32::EPSILON {
                    (v - pair[0]) / span
                } else {
                    0.0
                };
                return ((i as f32 * 10.0 + frac * 10.0).floor() as u8).min(100);
            }
        }
        100
    }

    pub fn parse_stinger_core_batt_response(
        buf: &[u8],
        strict_hp_header: bool,
    ) -> Option<HidBatteryReading> {
        if buf.len() < 13 || buf[0] != 0xFF {
            return None;
        }
        if strict_hp_header && (buf[1] != 0x12 || buf[2] != 0x05) {
            return None;
        }
        let charge = buf[9];
        let voltage_mv = i16::from_le_bytes([buf[11], buf[12]]);
        if !(2000..=5000).contains(&voltage_mv) {
            return None;
        }

        match charge {
            6 => Some(HidBatteryReading::measured(100, Some(false))),
            5 => Some(HidBatteryReading::measured_discrete(
                interpolate_voltage_knots(voltage_mv, &STINGER_CORE_CHARGING_CURVE_V),
                Some(true),
                "curva de tensão",
            )),
            3 => Some(HidBatteryReading::measured_discrete(
                interpolate_voltage_knots(voltage_mv, &STINGER_CORE_DISCHARGING_CURVE_V),
                Some(false),
                "curva de tensão",
            )),
            _ => None,
        }
    }

    fn strip_optional_zero(buf: &[u8]) -> &[u8] {
        if buf.first() == Some(&0x00) && buf.len() > 1 {
            &buf[1..]
        } else {
            buf
        }
    }

    pub fn parse_hyperx_cloud2_hp_battery(buf: &[u8]) -> Option<u8> {
        let s = strip_optional_zero(buf);
        if s.len() >= 8 && s[0] == 0x06 && s[1] == 0xFF && s[2] == 0xBB && s[3] == 0x02 {
            let pct = s[7];
            (pct <= 100).then_some(pct)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud2_hp_charging(buf: &[u8]) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 5 && s[0] == 0x06 && s[1] == 0xFF && s[2] == 0xBB && s[3] == 0x03 {
            Some(s[4] == 0x01)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud2_kingston_conn(buf: &[u8]) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 5 && s[0] == 0x0B && s[2] == 0xBB && s[3] == 0x01 {
            Some(s[4] == 0x01)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud2_kingston_battery(buf: &[u8]) -> Option<u8> {
        let s = strip_optional_zero(buf);
        if s.len() >= 8 && s[0] == 0x0B && s[2] == 0xBB && s[3] == 0x02 {
            let pct = s[7];
            (pct <= 100).then_some(pct)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud2_kingston_charging(buf: &[u8]) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 5 && s[0] == 0x0B && s[2] == 0xBB && s[3] == 0x03 {
            Some(s[4] == 0x01)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud_alpha_conn(buf: &[u8]) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 4 && s[0] == 0x21 && s[1] == 0xBB && s[2] == 0x03 {
            Some(s[3] != 0x01)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud_alpha_charging(buf: &[u8]) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 4 && s[0] == 0x21 && s[1] == 0xBB && s[2] == 0x0C {
            Some(s[3] == 0x01)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud_alpha_battery(buf: &[u8]) -> Option<u8> {
        let s = strip_optional_zero(buf);
        if s.len() >= 4 && s[0] == 0x21 && s[1] == 0xBB && s[2] == 0x0B {
            (s[3] <= 100).then_some(s[3])
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud_flight_battery(buf: &[u8]) -> Option<HidBatteryReading> {
        let s = strip_optional_zero(buf);
        if s.len() < 5 || s[0] != 0x21 || s[1] != 0xFF || s[2] != 0x05 {
            return None;
        }
        let mv = u16::from_be_bytes([s[3], s[4]]);
        if mv > 0x100B {
            return Some(HidBatteryReading::unavailable(
                Some(true),
                "Carregando via USB; percentual exato indisponível durante a carga.",
            ));
        }
        if mv < 2500 {
            return Some(HidBatteryReading::disconnected(
                "Headset HyperX Cloud Flight desligado ou desconectado.",
            ));
        }
        let pct_f = if mv <= 3648 {
            0.00125_f64 * f64::from(mv)
        } else if mv > 3975 {
            100.0
        } else {
            let v = f64::from(mv);
            0.000_000_025_475_05 * v.powi(4) - 0.000_390_029_9 * v.powi(3) + 2.238_321 * v.powi(2)
                - 5706.256 * v
                + 5_452_299.0
        };
        let pct = pct_f.round().clamp(0.0, 100.0) as u8;
        Some(HidBatteryReading::measured_discrete(
            pct,
            Some(false),
            "curva mV",
        ))
    }

    pub fn parse_hyperx_cloud3_battery(buf: &[u8]) -> Option<u8> {
        let s = strip_optional_zero(buf);
        if s.len() >= 5 && s[0] == 0x66 && s[1] == 0x89 {
            if s[2] == 0 && s[3] == 0 {
                return None;
            }
            (s[4] <= 100).then_some(s[4])
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud3_charging(buf: &[u8]) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 3 && s[0] == 0x66 && s[1] == 0x8A {
            Some(s[2] == 0x01)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud3s_conn(buf: &[u8], expected_online_byte: u8) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 7 && s[0] == 0x0C && s[5] == 0x02 {
            Some(s[6] == expected_online_byte)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud3s_battery(buf: &[u8]) -> Option<u8> {
        let s = strip_optional_zero(buf);
        if s.len() >= 7 && s[0] == 0x0C && s[5] == 0x06 && s[6] <= 100 {
            Some(s[6])
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud3s_charging(buf: &[u8]) -> Option<bool> {
        let s = strip_optional_zero(buf);
        if s.len() >= 7 && s[0] == 0x0C && s[5] == 0x48 {
            Some(s[6] == 0x01)
        } else {
            None
        }
    }

    pub fn parse_hyperx_cloud_alpha2_battery(buf: &[u8]) -> Option<HidBatteryReading> {
        let s = strip_optional_zero(buf);
        if matches!(s.first(), Some(0xFF | 0xFB)) {
            return None;
        }
        if s.len() < 4 || s[0] != 0x51 || (s[1] & 0x3F) != 0x02 || s[2] > 100 {
            return None;
        }
        let pct = s[2];
        let charging = Some(s[3] == 0x01);
        let mv = s
            .get(6..8)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .unwrap_or(0);
        if pct == 0 && mv == 0 && charging != Some(true) {
            return Some(HidBatteryReading::disconnected(
                "Headset HyperX desconectado da base.",
            ));
        }
        Some(HidBatteryReading::measured(pct, charging))
    }

    // -----------------------------------------------------------------------
    // Razer (VID 0x1532)
    // -----------------------------------------------------------------------

    pub fn build_razer_feature_request(
        report_len: usize,
        command_class: u8,
        command_id: u8,
        data_size: u8,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; report_len.max(91)];
        buf[0] = 0x00;
        buf[1] = 0x00;
        buf[2] = 0x1F;
        buf[6] = data_size;
        buf[7] = command_class;
        buf[8] = command_id;
        let mut crc = 0u8;
        for &b in &buf[3..89] {
            crc ^= b;
        }
        buf[89] = crc;
        buf[90] = 0x00;
        buf
    }

    fn extract_razer_reply_fields(buf: &[u8]) -> Option<(u8, u8, u8, u8)> {
        if buf.len() >= 91 && buf[0] == 0x00 {
            Some((buf[1], buf[7], buf[8], buf[10]))
        } else if buf.len() >= 90 {
            Some((buf[0], buf[6], buf[7], buf[9]))
        } else {
            None
        }
    }

    pub fn parse_razer_battery_response(buf: &[u8]) -> Option<u8> {
        let (status, class, cmd, val) = extract_razer_reply_fields(buf)?;
        if status != 0x02 || class != 0x07 || cmd != 0x80 {
            return None;
        }
        if val == 0 {
            return Some(0);
        }
        let pct = if val <= 100 {
            val
        } else {
            ((u32::from(val) * 100 + 127) / 255) as u8
        };
        Some(pct.min(100))
    }

    pub fn parse_razer_charging_response(buf: &[u8]) -> Option<bool> {
        let (status, class, cmd, val) = extract_razer_reply_fields(buf)?;
        if status != 0x02 || class != 0x07 || cmd != 0x84 {
            return None;
        }
        Some(val == 0x01)
    }

    pub fn interpret_razer_reading(
        raw_pct: Option<u8>,
        charging: Option<bool>,
    ) -> HidBatteryReading {
        match raw_pct {
            Some(0) => {
                if charging == Some(true) {
                    HidBatteryReading::unavailable(
                        Some(true),
                        "Carregando; nível percentual Razer ainda em inicialização.",
                    )
                } else {
                    HidBatteryReading::disconnected(
                        "Periférico Razer desligado ou em suspensão (dongle retornou 0%).",
                    )
                }
            }
            Some(pct) => HidBatteryReading::measured(pct, charging),
            None => {
                HidBatteryReading::unavailable(charging, "Sem resposta válida do periférico Razer.")
            }
        }
    }

    pub fn resolve_razer_name(pid: u16, product_string: Option<&str>) -> String {
        if let Some(s) = product_string {
            let trimmed = s.trim();
            if !trimmed.is_empty()
                && !trimmed.eq_ignore_ascii_case("razer")
                && !trimmed.eq_ignore_ascii_case("razer device")
            {
                return normalize_vendor_prefix("Razer", trimmed);
            }
        }
        match pid {
            0x0078 | 0x007A => "Razer Viper Ultimate".to_owned(),
            0x008A | 0x008C => "Razer DeathAdder V2 Pro".to_owned(),
            0x0083 | 0x0088 => "Razer Basilisk Ultimate".to_owned(),
            0x0090 | 0x0092 => "Razer Naga Pro".to_owned(),
            0x0086 => "Razer BlackWidow V3 Pro".to_owned(),
            0x00A5 | 0x00A6 => "Razer Viper V2 Pro".to_owned(),
            0x00B6 | 0x00B7 => "Razer DeathAdder V3 Pro".to_owned(),
            0x0099 | 0x009A => "Razer Basilisk V3 Pro".to_owned(),
            0x00AB => "Razer Cobra Pro".to_owned(),
            0x00B9 | 0x00BA => "Razer Viper V3 Pro".to_owned(),
            0x00BC => "Razer DeathAdder V3 HyperSpeed".to_owned(),
            0x025A => "Razer BlackShark V2 Pro".to_owned(),
            0x0287 => "Razer BlackShark V2 Pro (2023)".to_owned(),
            0x026D => "Razer Barracuda X".to_owned(),
            0x0283 => "Razer Barracuda Pro".to_owned(),
            0x008F => "Razer DeathStalker V2 Pro".to_owned(),
            0x00A3 => "Razer DeathStalker V2 Pro TKL".to_owned(),
            _ => format!("Razer Wireless Device ({pid:04x})"),
        }
    }

    // -----------------------------------------------------------------------
    // SteelSeries (VID 0x1038)
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SteelSeriesProtocol {
        NovaDiscrete,
        NovaPercent,
        Nova5,
        Arctis7Plus,
        Arctis7Legacy,
        MouseFeature,
    }

    pub fn steelseries_protocol_for_pid(pid: u16) -> SteelSeriesProtocol {
        match pid {
            0x2202 | 0x2206 | 0x220A | 0x223A | 0x227A | 0x22A4 => {
                SteelSeriesProtocol::NovaDiscrete
            }
            0x22A1 | 0x227E | 0x2258 | 0x229E | 0x22AD | 0x22A9 | 0x22A5 | 0x22A7 | 0x2298 => {
                SteelSeriesProtocol::NovaPercent
            }
            0x2232 | 0x2253 => SteelSeriesProtocol::Nova5,
            0x220E | 0x2212 | 0x2216 | 0x2236 => SteelSeriesProtocol::Arctis7Plus,
            0x1260 | 0x12AD | 0x12AE | 0x1252 | 0x1280 => SteelSeriesProtocol::Arctis7Legacy,
            _ => SteelSeriesProtocol::MouseFeature,
        }
    }

    pub fn parse_steelseries_response(
        proto: SteelSeriesProtocol,
        buf: &[u8],
    ) -> Option<HidBatteryReading> {
        let s = strip_optional_zero(buf);
        match proto {
            SteelSeriesProtocol::NovaDiscrete => {
                if s.len() < 4 || s[0] != 0xB0 {
                    return None;
                }
                if s[3] == 0 {
                    return Some(HidBatteryReading::disconnected(
                        "Headset SteelSeries desligado ou fora do alcance.",
                    ));
                }
                let charging = Some(s[3] == 0x01);
                let val = s[2];
                if val > 4 {
                    return None;
                }
                Some(HidBatteryReading::measured_discrete(
                    val * 25,
                    charging,
                    "nível discreto 0–4",
                ))
            }
            SteelSeriesProtocol::NovaPercent => {
                if s.len() < 4 || s[0] != 0xB0 {
                    return None;
                }
                if s[3] == 0 {
                    return Some(HidBatteryReading::disconnected(
                        "Headset SteelSeries desligado ou fora do alcance.",
                    ));
                }
                if s[2] > 100 {
                    return None;
                }
                Some(HidBatteryReading::measured(s[2], Some(s[3] == 0x01)))
            }
            SteelSeriesProtocol::Nova5 => {
                if s.len() < 4 || s[0] != 0xB0 {
                    return None;
                }
                if !matches!(s[1], 0 | 1) {
                    return Some(HidBatteryReading::disconnected(
                        "Headset SteelSeries Nova 5 desligado ou fora do alcance.",
                    ));
                }
                if s[3] > 100 {
                    return None;
                }
                Some(HidBatteryReading::measured(s[3], Some(s[1] == 0x01)))
            }
            SteelSeriesProtocol::Arctis7Plus => {
                if s.len() < 4 || s[0] != 0xB0 {
                    return None;
                }
                if s[1] == 0x01 {
                    return Some(HidBatteryReading::disconnected(
                        "Headset SteelSeries Arctis 7+ desligado ou fora do alcance.",
                    ));
                }
                if s[2] > 4 {
                    return None;
                }
                Some(HidBatteryReading::measured_discrete(
                    s[2] * 25,
                    Some(s[3] == 0x01),
                    "nível discreto 0–4",
                ))
            }
            SteelSeriesProtocol::Arctis7Legacy => {
                if s.len() < 3 || s[0] != 0x06 || s[2] > 100 {
                    return None;
                }
                if s[2] == 0 {
                    return Some(HidBatteryReading::disconnected(
                        "Headset SteelSeries Arctis desligado ou sem sinal.",
                    ));
                }
                Some(HidBatteryReading::measured(s[2], None))
            }
            SteelSeriesProtocol::MouseFeature => {
                if s.len() < 3 {
                    return None;
                }
                if !matches!(s[0], 0x90 | 0x92 | 0xAA) || s[1] > 100 {
                    return None;
                }
                let charging = matches!(s[2], 0x01 | 0x10);
                if s[1] == 0 && !charging {
                    return Some(HidBatteryReading::disconnected(
                        "Periférico SteelSeries em suspensão ou desligado.",
                    ));
                }
                Some(HidBatteryReading::measured(s[1], Some(charging)))
            }
        }
    }

    pub fn resolve_steelseries_name(pid: u16, product_string: Option<&str>) -> String {
        if let Some(s) = product_string {
            let trimmed = s.trim();
            if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("steelseries") {
                return normalize_vendor_prefix("SteelSeries", trimmed);
            }
        }
        match pid {
            0x1830 | 0x1832 => "SteelSeries Rival 3 Wireless".to_owned(),
            0x1836 | 0x1838 => "SteelSeries Aerox 3 Wireless".to_owned(),
            0x1852 | 0x1854 => "SteelSeries Aerox 5 Wireless".to_owned(),
            0x1858 | 0x185A => "SteelSeries Aerox 9 Wireless".to_owned(),
            0x1842 | 0x1844 => "SteelSeries Prime Wireless".to_owned(),
            0x172B | 0x172E => "SteelSeries Rival 650 Wireless".to_owned(),
            0x1260 | 0x12AD | 0x12AE => "SteelSeries Arctis 7".to_owned(),
            0x1252 => "SteelSeries Arctis Pro Wireless".to_owned(),
            0x12B3 => "SteelSeries Arctis 1 Wireless".to_owned(),
            0x220E | 0x2212 | 0x2216 | 0x2236 | 0x12C2 => "SteelSeries Arctis 7+".to_owned(),
            0x12C0 => "SteelSeries Arctis 9".to_owned(),
            0x2232 | 0x2253 => "SteelSeries Arctis Nova 5".to_owned(),
            0x2202..=0x22AD => "SteelSeries Arctis Nova 7".to_owned(),
            _ => format!("SteelSeries Wireless Device ({pid:04x})"),
        }
    }

    // -----------------------------------------------------------------------
    // Corsair (VID 0x1B1C)
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CorsairProtocol {
        Void,
        Bragi,
        SlipstreamFeature,
    }

    pub fn corsair_protocol_for_pid(pid: u16) -> CorsairProtocol {
        match pid {
            0x0A14 | 0x0A16 | 0x0A17 | 0x0A1A | 0x0A1D | 0x0A2B | 0x0A38 | 0x0A4F | 0x0A51
            | 0x0A52 | 0x0A55 | 0x0A56 | 0x0A75 | 0x1B23 | 0x1B29 | 0x1B2A => CorsairProtocol::Void,
            0x0A64 | 0x0A62 | 0x0A3E | 0x0A3D | 0x0A6A => CorsairProtocol::Bragi,
            _ => CorsairProtocol::SlipstreamFeature,
        }
    }

    pub fn parse_corsair_void_response(buf: &[u8]) -> Option<HidBatteryReading> {
        let s = strip_optional_zero(buf);
        if s.len() < 5 || s[0] != 100 || s[3] != 177 {
            return None;
        }
        let status = s[4];
        if status == 0 {
            return Some(HidBatteryReading::disconnected(
                "Headset Corsair desconectado ou desligado.",
            ));
        }
        let charging = match status {
            4 | 5 => true,
            1 | 2 => false,
            _ => return None,
        };
        let pct = s[2] & 0x7F;
        if pct > 100 || (pct == 0 && !charging) {
            return None;
        }
        Some(HidBatteryReading::measured(pct, Some(charging)))
    }

    pub fn parse_corsair_bragi_reply(buf: &[u8], expected_cmd: u8) -> Option<Result<u32, u8>> {
        if buf.len() < 8 || buf[0] != 0x01 || buf[2] != expected_cmd {
            return None;
        }
        let status = buf[3];
        if status != 0x00 {
            return Some(Err(status));
        }
        let val = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        Some(Ok(val))
    }

    pub fn parse_corsair_slipstream_feature(buf: &[u8]) -> Option<HidBatteryReading> {
        let s = strip_optional_zero(buf);
        if s.len() < 3 || !matches!(s[0], 0x08 | 0x0E) || s[1] > 100 {
            return None;
        }
        let charging = matches!(s[2], 0x02 | 0x04);
        if s[1] == 0 && !charging {
            return Some(HidBatteryReading::disconnected(
                "Periférico Corsair em suspensão ou desligado.",
            ));
        }
        Some(HidBatteryReading::measured(s[1], Some(charging)))
    }

    pub fn resolve_corsair_name(pid: u16, product_string: Option<&str>) -> String {
        if let Some(s) = product_string {
            let trimmed = s.trim();
            if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("corsair") {
                return normalize_vendor_prefix("Corsair", trimmed);
            }
        }
        match pid {
            0x1BA0 => "Corsair Katar Pro Wireless".to_owned(),
            0x1B82 => "Corsair Harpoon RGB Wireless".to_owned(),
            0x1B27 => "Corsair Dark Core RGB Pro".to_owned(),
            0x0A55 | 0x0A56 => "Corsair Virtuoso RGB Wireless".to_owned(),
            0x0A64 | 0x0A62 | 0x0A3E | 0x0A3D => "Corsair Virtuoso XT/SE".to_owned(),
            0x0A4A => "Corsair HS70 Wireless".to_owned(),
            0x0A6A => "Corsair HS80 RGB Wireless".to_owned(),
            _ => format!("Corsair Wireless Device ({pid:04x})"),
        }
    }

    // -----------------------------------------------------------------------
    // Sony DualSense / DualShock 4 (VID 0x054C)
    // -----------------------------------------------------------------------

    pub fn is_sony_controller_pid(pid: u16) -> Option<(&'static str, bool)> {
        match pid {
            0x0DF2 => Some(("PlayStation DualSense Edge Wireless Controller", true)),
            0x0CE6 => Some(("PlayStation DualSense Wireless Controller", true)),
            0x0BA0 => Some(("PlayStation DualShock 4 Wireless Adapter", false)),
            0x05C4 | 0x09CC => Some(("PlayStation DualShock 4 Controller", false)),
            _ => None,
        }
    }

    pub fn parse_sony_battery_report(buf: &[u8], is_dualsense: bool) -> Option<HidBatteryReading> {
        if buf.is_empty() {
            return None;
        }
        if is_dualsense {
            let byte = if buf[0] == 0x01 && buf.len() >= 54 {
                buf[53]
            } else if buf[0] == 0x31 && buf.len() >= 55 {
                buf[54]
            } else {
                return None;
            };
            let level_nibble = byte & 0x0F;
            let chg_state = (byte >> 4) & 0x0F;
            return match chg_state {
                0x00 => Some(HidBatteryReading::measured(
                    level_nibble.min(10) * 10,
                    Some(false),
                )),
                0x01 => Some(HidBatteryReading::measured(
                    level_nibble.min(10) * 10,
                    Some(true),
                )),
                0x02 => Some(HidBatteryReading::measured(100, Some(false))),
                _ => Some(HidBatteryReading::unavailable(
                    None,
                    format!("Estado de bateria DualSense não disponível (0x{chg_state:X})."),
                )),
            };
        }

        let byte = if buf[0] == 0x01 && buf.len() >= 31 {
            buf[30]
        } else if buf[0] == 0x11 && buf.len() >= 33 {
            buf[32]
        } else {
            return None;
        };
        let level_nibble = byte & 0x0F;
        let charging = (byte & 0x10) != 0;
        Some(HidBatteryReading::measured(
            level_nibble.min(10) * 10,
            Some(charging),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_peripheral_names() {
        assert_eq!(
            classify_kind("Logitech MX Master 3S"),
            PeripheralKind::Mouse
        );
        assert_eq!(
            classify_kind("Keychron K3 Pro Keyboard"),
            PeripheralKind::Keyboard
        );
        assert_eq!(classify_kind("Sony WH-1000XM5"), PeripheralKind::Headset);
        assert_eq!(
            classify_kind("HyperX Cloud Stinger Core Wireless DTS"),
            PeripheralKind::Headset
        );
        assert_eq!(
            classify_kind("Logitech G PRO X 2 LIGHTSPEED"),
            PeripheralKind::Headset
        );
        assert_eq!(
            classify_kind("Xbox Wireless Controller"),
            PeripheralKind::Controller
        );
        assert_eq!(classify_kind("Surface Slim Pen 2"), PeripheralKind::Stylus);
        assert_eq!(classify_kind("Generic Gadget"), PeripheralKind::Other);
    }

    #[test]
    fn measured_percent_rejects_out_of_range_values() {
        assert_eq!(measured_percent(85).value, Some(85));
        assert_eq!(measured_percent(100).value, Some(100));
        assert_eq!(measured_percent(101).value, None);
        assert!(measured_percent(101).unavailable_reason.is_some());
    }

    #[test]
    fn normalize_deduplicates_audio_profiles_and_sorts_lowest_battery_first() {
        let raw = vec![
            PeripheralBatteryDevice {
                id: "1".into(),
                name: "Sony WH-1000XM5 Hands-Free AG Audio".into(),
                hardware_name: None,
                custom_name: None,
                kind: PeripheralKind::Headset,
                transport: PeripheralTransport::BluetoothClassic,
                connected: true,
                charging: None,
                battery_percent: measured_percent(72),
                source: "PnP".into(),
            },
            PeripheralBatteryDevice {
                id: "2".into(),
                name: "Sony WH-1000XM5".into(),
                hardware_name: None,
                custom_name: None,
                kind: PeripheralKind::Headset,
                transport: PeripheralTransport::BluetoothClassic,
                connected: true,
                charging: None,
                battery_percent: unavailable_percent("Sem leitura"),
                source: "PnP".into(),
            },
            PeripheralBatteryDevice {
                id: "3".into(),
                name: "MX Master 3S".into(),
                hardware_name: None,
                custom_name: None,
                kind: PeripheralKind::Mouse,
                transport: PeripheralTransport::BluetoothLe,
                connected: true,
                charging: Some(false),
                battery_percent: measured_percent(18),
                source: "BLE".into(),
            },
        ];

        let normalized = normalize_devices(raw);
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].name, "MX Master 3S");
        assert_eq!(normalized[0].battery_percent.value, Some(18));
        assert_eq!(normalized[1].battery_percent.value, Some(72));
    }

    #[test]
    fn parses_logitech_hidpp_unified_battery_1004() {
        // SoC suportado: 74%, descarregando (0)
        let discharging = [0x11, 0x01, 0x07, 0x1D, 74, 0x04, 0x00, 0x00];
        let parsed = hid_protocol::parse_hidpp_unified_battery(&discharging, true);
        assert_eq!(parsed.percent, Some(74));
        assert_eq!(parsed.charging, Some(false));
        assert!(parsed.connected);

        // Carregamento lento (2) -> charging = true
        let slow_chg = [0x11, 0xFF, 0x07, 0x1D, 88, 0x04, 0x02, 0x00];
        let parsed_chg = hid_protocol::parse_hidpp_unified_battery(&slow_chg, true);
        assert_eq!(parsed_chg.percent, Some(88));
        assert_eq!(parsed_chg.charging, Some(true));

        // Erro de carga (4) -> indisponível, nunca inventa percentual
        let err_chg = [0x11, 0x01, 0x07, 0x1D, 50, 0x04, 0x04, 0x00];
        let parsed_err = hid_protocol::parse_hidpp_unified_battery(&err_chg, true);
        assert_eq!(parsed_err.percent, None);
        assert!(parsed_err.unavailable_reason.is_some());

        // Sem suporte SoC (apenas faixa discreta) -> explicita indisponibilidade honesta
        let discrete_only = [0x11, 0x01, 0x07, 0x1D, 0, 0x08, 0x00, 0x00];
        let parsed_disc = hid_protocol::parse_hidpp_unified_battery(&discrete_only, false);
        assert_eq!(parsed_disc.percent, None);
        assert!(parsed_disc
            .unavailable_reason
            .as_deref()
            .is_some_and(|r| r.contains("Cheio")));
    }

    #[test]
    fn parses_logitech_hidpp_status_1000_without_fake_zero() {
        let discharging = [0x11, 0x01, 0x05, 0x0D, 65, 50, 0x00];
        let r = hid_protocol::parse_hidpp_battery_status(&discharging);
        assert_eq!(r.percent, Some(65));
        assert_eq!(r.charging, Some(false));

        // Em 0x1000, carregando com pct=0 significa nível omitido durante carga — NUNCA 0% falso.
        let charging_zero = [0x11, 0x01, 0x05, 0x0D, 0, 0, 0x01];
        let r_chg = hid_protocol::parse_hidpp_battery_status(&charging_zero);
        assert_eq!(r_chg.percent, None);
        assert_eq!(r_chg.charging, Some(true));
        assert!(r_chg.unavailable_reason.is_some());

        // Zero fora de carga também deve ser rejeitado
        let bogus_zero = [0x11, 0x01, 0x05, 0x0D, 0, 0, 0x00];
        assert_eq!(
            hid_protocol::parse_hidpp_battery_status(&bogus_zero).percent,
            None
        );

        // Carga completa (3) com pct=0 -> 100%, não carregando
        let full = [0x11, 0x01, 0x05, 0x0D, 0, 0, 0x03];
        let r_full = hid_protocol::parse_hidpp_battery_status(&full);
        assert_eq!(r_full.percent, Some(100));
        assert_eq!(r_full.charging, Some(false));
    }

    #[test]
    fn parses_logitech_hidpp_voltage_1001_and_error_frames() {
        // 3955 mV (0x0F73), descarregando (flags = 0) -> 75% na curva Logitech (índice 25)
        let volt_frame = [0x11, 0x01, 0x06, 0x0D, 0x0F, 0x73, 0x00];
        let r = hid_protocol::parse_hidpp_battery_voltage(&volt_frame);
        assert_eq!(r.percent, Some(75));
        assert_eq!(r.charging, Some(false));

        // 4186 mV carregando via USB (flags = 0x80)
        let chg_frame = [0x11, 0x01, 0x06, 0x0D, 0x10, 0x5A, 0x80];
        let r_chg = hid_protocol::parse_hidpp_battery_voltage(&chg_frame);
        assert_eq!(r_chg.percent, Some(100));
        assert_eq!(r_chg.charging, Some(true));

        // Erro térmico (flags = 0x83) -> indisponível
        let err_frame = [0x11, 0x01, 0x06, 0x0D, 0x0F, 0x00, 0x83];
        assert_eq!(
            hid_protocol::parse_hidpp_battery_voltage(&err_frame).percent,
            None
        );

        // Tensão absurda -> rejeita
        let invalid_mv = [0x11, 0x01, 0x06, 0x0D, 0x00, 0x00, 0x00];
        assert_eq!(
            hid_protocol::parse_hidpp_battery_voltage(&invalid_mv).percent,
            None
        );

        // Classificação de respostas e erros HID++ 1.0/2.0 (incl. 0x09 recurso/dormindo)
        let err_20 = [0x11, 0x01, 0xFF, 0x00, 0x1D, 0x09, 0x00];
        assert_eq!(
            hid_protocol::classify_hidpp_reply(&err_20, 0x01, 0x00, 0x01, 0x0D),
            hid_protocol::HidppReply::Error(0x09)
        );
        let err_10 = [0x10, 0x02, 0x8F, 0x00, 0x0D, 0x08, 0x00];
        assert_eq!(
            hid_protocol::classify_hidpp_reply(&err_10, 0x02, 0x00, 0x00, 0x0D),
            hid_protocol::HidppReply::Error(0x08)
        );
    }

    #[test]
    fn deduplicates_logitech_lightspeed_c53f_collections_per_usb_instance() {
        let col1 = r"\\?\hid#vid_046d&pid_c53f&mi_02&col01#7&11223344&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let col2 = r"\\?\hid#vid_046d&pid_c53f&mi_02&col02#7&11223344&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let col2_other_port = r"\\?\hid#vid_046d&pid_c53f&mi_02&col02#7&99887766&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(
            hid_protocol::logitech_hidpp_receiver_key(col1, 0x046D, 0xC53F),
            hid_protocol::logitech_hidpp_receiver_key(col2, 0x046D, 0xC53F)
        );
        assert_ne!(
            hid_protocol::logitech_hidpp_receiver_key(col2, 0x046D, 0xC53F),
            hid_protocol::logitech_hidpp_receiver_key(col2_other_port, 0x046D, 0xC53F)
        );
        assert_eq!(hid_protocol::anonymize_hid_path_label(col2), "MI_02&COL02");
    }

    #[test]
    fn parses_hyperx_stinger_core_wireless_dts_03f0_0a8e() {
        let spec = hid_protocol::lookup_hyperx_model(0x03F0, 0x0A8E).expect("03F0:0A8E mapeado");
        assert_eq!(spec.protocol, hid_protocol::HyperXProtocol::StingerCoreHp);
        assert!(hid_protocol::hyperx_accepts_collection(
            spec, 0xFF00, 0x0001, 0, 0, 64
        ));
        assert!(!hid_protocol::hyperx_accepts_collection(
            spec, 0x000C, 0x0001, 4, 0, 0
        ));

        // Passo 1: verificação de conexão
        let mut conn_ok = [0u8; 32];
        conn_ok[0] = 0xFF;
        conn_ok[11] = 0x01;
        assert_eq!(
            hid_protocol::parse_stinger_core_conn_response(&conn_ok),
            Some(true)
        );
        let mut conn_off = [0u8; 32];
        conn_off[0] = 0xFF;
        conn_off[11] = 0x00;
        assert_eq!(
            hid_protocol::parse_stinger_core_conn_response(&conn_off),
            Some(false)
        );

        // Passo 2: pacote desatualizado (sem FF 12 05) precisa ser ignorado no strict mode
        let mut stale = [0u8; 32];
        stale[0] = 0xFF;
        stale[1] = 0x07;
        stale[9] = 3;
        stale[11..13].copy_from_slice(&3800i16.to_le_bytes());
        assert_eq!(
            hid_protocol::parse_stinger_core_batt_response(&stale, true),
            None
        );

        // Em uso (charge=3), 3905 mV -> ~70% na curva de descarga
        let mut discharging = [0u8; 32];
        discharging[0] = 0xFF;
        discharging[1] = 0x12;
        discharging[2] = 0x05;
        discharging[9] = 3;
        discharging[11..13].copy_from_slice(&3905i16.to_le_bytes());
        let r_dis = hid_protocol::parse_stinger_core_batt_response(&discharging, true)
            .expect("leitura válida");
        assert_eq!(r_dis.percent, Some(69));
        assert_eq!(r_dis.charging, Some(false));

        // Carregando (charge=5), 3967 mV -> ~59-60% na curva de carga
        let mut charging = discharging;
        charging[9] = 5;
        charging[11..13].copy_from_slice(&3967i16.to_le_bytes());
        let r_chg =
            hid_protocol::parse_stinger_core_batt_response(&charging, true).expect("carga válida");
        assert_eq!(r_chg.charging, Some(true));
        assert!(matches!(r_chg.percent, Some(58..=61)));

        // Carga completa (charge=6) -> 100%, não carregando
        let mut full = discharging;
        full[9] = 6;
        full[11..13].copy_from_slice(&4180i16.to_le_bytes());
        let r_full =
            hid_protocol::parse_stinger_core_batt_response(&full, true).expect("carga cheia");
        assert_eq!(r_full.percent, Some(100));
        assert_eq!(r_full.charging, Some(false));

        // Tensão inválida (0 mV) ou código de carga inválido -> None
        let mut invalid = discharging;
        invalid[11..13].copy_from_slice(&0i16.to_le_bytes());
        assert_eq!(
            hid_protocol::parse_stinger_core_batt_response(&invalid, true),
            None
        );
    }

    #[test]
    fn parses_other_hyperx_razer_steelseries_corsair_sony_protocols() {
        // HyperX Cloud III
        let c3 = [0x66, 0x89, 0x01, 0x0F, 84, 0x00];
        assert_eq!(hid_protocol::parse_hyperx_cloud3_battery(&c3), Some(84));
        let c3_zero = [0x66, 0x89, 0x00, 0x00, 0, 0];
        assert_eq!(hid_protocol::parse_hyperx_cloud3_battery(&c3_zero), None);

        // HyperX Cloud Flight (tensão big-endian -> ~84% ou charging > 0x100B)
        let cf = [0x21, 0xFF, 0x05, 0x0F, 0x20];
        let cf_r = hid_protocol::parse_hyperx_cloud_flight_battery(&cf).expect("Cloud Flight ok");
        assert!(cf_r.percent.is_some());
        assert_eq!(cf_r.charging, Some(false));

        // Razer (0x1532) — escala 0..=255 e proteção contra zero em repouso
        let req = hid_protocol::build_razer_feature_request(91, 0x07, 0x80, 0x02);
        assert_eq!(req[2], 0x1F);
        let mut r_resp = [0u8; 91];
        r_resp[1] = 0x02;
        r_resp[7] = 0x07;
        r_resp[8] = 0x80;
        r_resp[10] = 128;
        assert_eq!(
            hid_protocol::parse_razer_battery_response(&r_resp),
            Some(50)
        );
        let r_idle = hid_protocol::interpret_razer_reading(Some(0), Some(false));
        assert!(!r_idle.connected);
        assert_eq!(r_idle.percent, None);

        // SteelSeries Nova Percent com prefixo 0x00 do Win32 ReadFile
        let ss = [0x00, 0xB0, 0x00, 77, 0x03];
        let ss_r = hid_protocol::parse_steelseries_response(
            hid_protocol::SteelSeriesProtocol::NovaPercent,
            &ss,
        )
        .expect("Nova Percent ok");
        assert_eq!(ss_r.percent, Some(77));
        assert_eq!(ss_r.charging, Some(false));

        // Corsair Void offline vs conectado com bit de microfone (0x80)
        let c_void = [100, 0, 0x80 | 64, 177, 1];
        let c_r = hid_protocol::parse_corsair_void_response(&c_void).expect("Void ok");
        assert_eq!(c_r.percent, Some(64));
        assert_eq!(c_r.charging, Some(false));

        // Sony DualSense USB
        let mut ds = [0u8; 64];
        ds[0] = 0x01;
        ds[53] = 0x17; // 70%, carregando
        let ds_r = hid_protocol::parse_sony_battery_report(&ds, true).expect("DualSense ok");
        assert_eq!(ds_r.percent, Some(70));
        assert_eq!(ds_r.charging, Some(true));
    }

    #[test]
    fn sanitize_alias_trims_strips_control_chars_and_limits_length() {
        assert_eq!(sanitize_alias("   "), None);
        assert_eq!(sanitize_alias("\n\t\r"), None);
        assert_eq!(
            sanitize_alias("  Mouse\tPrincipal\n "),
            Some("Mouse Principal".to_owned())
        );

        let long_name = "Á".repeat(80);
        let sanitized = sanitize_alias(&long_name).expect("alias longo sanitizado");
        assert_eq!(sanitized.chars().count(), MAX_ALIAS_CHARS);
    }

    #[test]
    fn aliases_persist_apply_update_and_restore_hardware_name() {
        let unique = format!(
            "pulse-test-aliases-{}-{}",
            std::process::id(),
            now_unix_ms()
        );
        let temp_dir = std::env::temp_dir().join(unique);
        let path = temp_dir.join("peripheral-aliases.json");

        let mut snapshot = PeripheralBatterySnapshot {
            devices: vec![
                PeripheralBatteryDevice {
                    id: "hid-logi-1".into(),
                    name: "Logitech G PRO X Superlight".into(),
                    hardware_name: None,
                    custom_name: None,
                    kind: PeripheralKind::Mouse,
                    transport: PeripheralTransport::UsbHid,
                    connected: true,
                    charging: Some(false),
                    battery_percent: measured_percent(64),
                    source: "Logitech HID++ 2.0".into(),
                },
                PeripheralBatteryDevice {
                    id: "ble-kb-1".into(),
                    name: "Keychron K2".into(),
                    hardware_name: None,
                    custom_name: None,
                    kind: PeripheralKind::Keyboard,
                    transport: PeripheralTransport::BluetoothLe,
                    connected: true,
                    charging: Some(true),
                    battery_percent: measured_percent(82),
                    source: "Bluetooth LE".into(),
                },
            ],
            unavailable_reason: None,
            platform_note: None,
            collected_at_unix_ms: 1_700_000_000_000,
        };

        // 1. Salva alias e aplica no snapshot
        let aliases = update_alias_in_path(&path, "hid-logi-1", "  Mouse do Escritório  ")
            .expect("salva alias");
        assert_eq!(
            aliases.get("hid-logi-1").map(String::as_str),
            Some("Mouse do Escritório")
        );

        let reloaded = load_aliases_from(&path);
        assert_eq!(reloaded, aliases);

        apply_aliases_to_snapshot(&mut snapshot, &reloaded);
        assert_eq!(snapshot.devices[0].name, "Mouse do Escritório");
        assert_eq!(
            snapshot.devices[0].hardware_name.as_deref(),
            Some("Logitech G PRO X Superlight")
        );
        assert_eq!(
            snapshot.devices[0].custom_name.as_deref(),
            Some("Mouse do Escritório")
        );
        // Outro dispositivo sem alias segue intacto
        assert_eq!(snapshot.devices[1].name, "Keychron K2");
        assert_eq!(snapshot.devices[1].hardware_name, None);
        assert_eq!(snapshot.devices[1].custom_name, None);

        // 2. Atualiza alias no snapshot já modificado e confirma que hardware_name é preservado
        let aliases =
            update_alias_in_path(&path, "hid-logi-1", "Mouse Principal").expect("atualiza alias");
        apply_aliases_to_snapshot(&mut snapshot, &aliases);
        assert_eq!(snapshot.devices[0].name, "Mouse Principal");
        assert_eq!(
            snapshot.devices[0].hardware_name.as_deref(),
            Some("Logitech G PRO X Superlight")
        );
        assert_eq!(
            snapshot.devices[0].custom_name.as_deref(),
            Some("Mouse Principal")
        );

        // 3. String vazia remove o alias e restaura nome original do hardware
        let aliases = update_alias_in_path(&path, "hid-logi-1", "   ").expect("remove alias");
        assert!(!aliases.contains_key("hid-logi-1"));
        apply_aliases_to_snapshot(&mut snapshot, &aliases);
        assert_eq!(snapshot.devices[0].name, "Logitech G PRO X Superlight");
        assert_eq!(snapshot.devices[0].hardware_name, None);
        assert_eq!(snapshot.devices[0].custom_name, None);

        // 4. ID vazio é rejeitado
        assert!(update_alias_in_path(&path, "  ", "Algo").is_err());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
