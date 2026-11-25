//! Telemetria expandida de hardware (CPU, RAM, GPU, Disco I/O e Sensores)
//! e preferências persistidas de visualização (Desempenho + Tray Flyout).
//!
//! Princípios:
//! - 100% user-mode (sem driver kernel no Pulse): coleta tudo o que o SO e
//!   drivers expõem nativamente e integra automaticamente sensores de
//!   placa-mãe/fans quando LibreHardwareMonitor, OpenHardwareMonitor ou HWiNFO
//!   (Shared Memory) estiverem ativos no Windows, ou via `hwmon` no Linux.
//! - Zero dados fictícios: qualquer leitura ausente vira `None` + motivo.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Emitter;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

const SNAPSHOT_CACHE_TTL: Duration = Duration::from_millis(1800);
pub const MAX_TRAY_METRICS: usize = 8;

/// Métrica individual do catálogo unificado (usada nos cards expansíveis de
/// Desempenho e nos 4 slots personalizáveis do Tray Flyout).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryMetric {
    pub id: String,
    /// Grupo de subsistema: `cpu`, `ram`, `gpu`, `network`, `storage`, `sensors`.
    pub group: String,
    pub label: String,
    pub short_label: String,
    pub value: Option<f64>,
    pub unit: String,
    /// Tipo semântico para cor/escala: `percent`, `temperature`, `rate`,
    /// `clock`, `fan`, `power`, `voltage`, `memory`, `count`.
    pub kind: String,
    pub max_value: Option<f64>,
    pub detail: Option<String>,
    pub source: String,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CpuHardwareInfo {
    pub model: Option<String>,
    pub physical_cores: Option<u32>,
    pub logical_cores: u32,
    pub base_clock_mhz: Option<u32>,
    pub current_clock_mhz: Option<f64>,
    pub per_core_percent: Vec<f64>,
    pub uptime_seconds: Option<u64>,
    pub process_count: Option<u32>,
    pub thread_count: Option<u32>,
    pub handle_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RamHardwareInfo {
    pub used_gb: Option<f64>,
    pub total_gb: Option<f64>,
    pub available_gb: Option<f64>,
    pub cached_gb: Option<f64>,
    pub commit_used_gb: Option<f64>,
    pub commit_limit_gb: Option<f64>,
    pub speed_mhz: Option<u32>,
    pub slots_used: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GpuHardwareInfo {
    pub name: Option<String>,
    pub usage_percent: Option<f64>,
    pub vram_used_gb: Option<f64>,
    pub vram_total_gb: Option<f64>,
    pub shared_used_gb: Option<f64>,
    pub temperature_celsius: Option<f64>,
    pub fan_percent: Option<f64>,
    pub fan_rpm: Option<f64>,
    pub core_clock_mhz: Option<f64>,
    pub memory_clock_mhz: Option<f64>,
    pub power_watts: Option<f64>,
    pub power_limit_watts: Option<f64>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalDiskIo {
    pub name: String,
    pub read_mbps: Option<f64>,
    pub write_mbps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiskIoStats {
    pub read_mbps: Option<f64>,
    pub write_mbps: Option<f64>,
    pub active_percent: Option<f64>,
    pub queue_length: Option<f64>,
    pub read_iops: Option<f64>,
    pub write_iops: Option<f64>,
    pub per_disk: Vec<PhysicalDiskIo>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HardwareTelemetrySnapshot {
    pub cpu: CpuHardwareInfo,
    pub ram: RamHardwareInfo,
    pub gpu: GpuHardwareInfo,
    pub disk_io: DiskIoStats,
    pub metrics: Vec<TelemetryMetric>,
    pub external_providers: Vec<String>,
    pub sampled_at_unix_ms: u64,
}

pub const CARD_ORDER_GROUPS: [&str; 5] = ["cpu", "ram", "gpu", "network", "storage"];

pub fn default_card_order() -> Vec<String> {
    CARD_ORDER_GROUPS.iter().map(|s| (*s).to_owned()).collect()
}

/// Preferências do usuário para os 4 slots do Tray Flyout e métricas fixadas
/// nos cards compactos/expandidos da aba Desempenho.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryPreferences {
    pub tray_metrics: Vec<String>,
    pub card_metrics: HashMap<String, Vec<String>>,
    #[serde(default = "default_card_order")]
    pub card_order: Vec<String>,
}

impl Default for TelemetryPreferences {
    fn default() -> Self {
        let mut card_metrics = HashMap::new();
        card_metrics.insert(
            "cpu".into(),
            vec!["cpu.clock_ghz".into(), "cpu.temp".into()],
        );
        card_metrics.insert(
            "ram".into(),
            vec!["ram.available_gb".into(), "ram.cached_gb".into()],
        );
        card_metrics.insert(
            "gpu".into(),
            vec![
                "gpu.vram_used_gb".into(),
                "gpu.temp".into(),
                "gpu.fan_percent".into(),
                "gpu.power_watts".into(),
            ],
        );
        card_metrics.insert(
            "network".into(),
            vec!["net.download".into(), "net.upload".into()],
        );
        card_metrics.insert(
            "storage".into(),
            vec!["disk.read_mbps".into(), "disk.write_mbps".into()],
        );
        Self {
            tray_metrics: vec![
                "cpu.usage".into(),
                "ram.usage".into(),
                "storage.system_disk".into(),
                "gpu.usage".into(),
            ],
            card_metrics,
            card_order: default_card_order(),
        }
    }
}

/// Leitura crua de um sensor externo (LHM, OHM, HWiNFO, hwmon).
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalSensorReading {
    pub id: String,
    pub label: String,
    pub category: SensorCategory,
    pub value: f64,
    pub unit: String,
    pub source: String,
    pub group_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorCategory {
    Temperature,
    Fan,
    Clock,
    Power,
    Voltage,
    Control,
    Load,
}

impl SensorCategory {
    pub fn kind_str(self) -> &'static str {
        match self {
            Self::Temperature => "temperature",
            Self::Fan => "fan",
            Self::Clock => "clock",
            Self::Power => "power",
            Self::Voltage => "voltage",
            Self::Control | Self::Load => "percent",
        }
    }

    pub fn default_unit(self) -> &'static str {
        match self {
            Self::Temperature => "°C",
            Self::Fan => "RPM",
            Self::Clock => "MHz",
            Self::Power => "W",
            Self::Voltage => "V",
            Self::Control | Self::Load => "%",
        }
    }
}

/// Dados estendidos de uma GPU NVIDIA via `nvidia-smi`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NvidiaExtendedSample {
    pub index: u32,
    pub name: String,
    pub temperature_celsius: Option<f64>,
    pub fan_percent: Option<f64>,
    pub core_clock_mhz: Option<f64>,
    pub memory_clock_mhz: Option<f64>,
    pub power_watts: Option<f64>,
    pub power_limit_watts: Option<f64>,
    pub vram_used_mib: Option<f64>,
    pub vram_total_mib: Option<f64>,
    pub utilization_percent: Option<f64>,
}

pub fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

pub fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Converte temperatura em Kelvin (ex.: contador PDH `\Thermal Zone Information(*)\Temperature`)
/// para °C quando estiver na faixa física plausível (`10°C..=120°C`).
pub fn kelvin_to_valid_celsius(kelvin: f64) -> Option<f64> {
    if !kelvin.is_finite() {
        return None;
    }
    let celsius = kelvin - 273.15;
    if (10.0..=120.0).contains(&celsius) {
        Some(round1(celsius))
    } else {
        None
    }
}

/// Seleciona uso de memória de vídeo em GB a partir de amostras PDH
/// `\GPU Adapter Memory(*)\Dedicated Usage` ou `Shared Usage` (em bytes).
/// Prioriza o LUID do adaptador principal; sem correspondência, usa o maior valor válido.
pub fn select_gpu_memory_usage_gb(
    samples: &[(String, f64)],
    preferred_luid: Option<&str>,
) -> Option<f64> {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let valid: Vec<(&str, f64)> = samples
        .iter()
        .filter_map(|(name, bytes)| {
            if bytes.is_finite() && *bytes >= 0.0 {
                Some((name.as_str(), *bytes))
            } else {
                None
            }
        })
        .collect();
    if valid.is_empty() {
        return None;
    }
    if let Some(luid) = preferred_luid.map(str::trim).filter(|s| !s.is_empty()) {
        let luid_lower = luid.to_ascii_lowercase();
        if let Some(matched_max) = valid
            .iter()
            .filter(|(name, _)| name.to_ascii_lowercase().contains(&luid_lower))
            .map(|(_, bytes)| *bytes)
            .reduce(f64::max)
        {
            return Some(round2(matched_max / GIB));
        }
    }
    valid
        .into_iter()
        .map(|(_, bytes)| bytes)
        .reduce(f64::max)
        .map(|bytes| round2(bytes / GIB))
}

pub fn sanitize_id(raw: &str) -> String {
    let cleaned: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let collapsed = cleaned
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "sensor".into()
    } else {
        collapsed
    }
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn parse_optional_f64(raw: &str) -> Option<f64> {
    let cleaned = raw.trim();
    if cleaned.is_empty()
        || cleaned.eq_ignore_ascii_case("n/a")
        || cleaned.contains("Not Supported")
    {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Interpreta saída CSV de:
/// `nvidia-smi --query-gpu=index,name,temperature.gpu,fan.speed,clocks.gr,clocks.mem,power.draw,power.limit,memory.used,memory.total,utilization.gpu --format=csv,noheader,nounits`
pub fn parse_nvidia_extended_csv(text: &str) -> Vec<NvidiaExtendedSample> {
    let mut samples = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() < 11 {
            continue;
        }
        let Ok(index) = fields[0].parse::<u32>() else {
            continue;
        };
        // O nome pode conter vírgula; os 9 últimos campos são sempre numéricos/N/A.
        let tail = &fields[fields.len() - 9..];
        let name = fields[1..fields.len() - 9].join(",").trim().to_owned();
        samples.push(NvidiaExtendedSample {
            index,
            name,
            temperature_celsius: parse_optional_f64(tail[0])
                .filter(|v| (0.0..=150.0).contains(v))
                .map(round1),
            fan_percent: parse_optional_f64(tail[1])
                .filter(|v| (0.0..=100.0).contains(v))
                .map(round1),
            core_clock_mhz: parse_optional_f64(tail[2]).filter(|v| *v > 0.0).map(round1),
            memory_clock_mhz: parse_optional_f64(tail[3]).filter(|v| *v > 0.0).map(round1),
            power_watts: parse_optional_f64(tail[4])
                .filter(|v| *v >= 0.0)
                .map(round1),
            power_limit_watts: parse_optional_f64(tail[5]).filter(|v| *v > 0.0).map(round1),
            vram_used_mib: parse_optional_f64(tail[6]).filter(|v| *v >= 0.0),
            vram_total_mib: parse_optional_f64(tail[7]).filter(|v| *v > 0.0),
            utilization_percent: parse_optional_f64(tail[8])
                .filter(|v| (0.0..=100.0).contains(v))
                .map(round1),
        });
    }
    samples
}

/// Interpreta o array JSON de sensores WMI do LibreHardwareMonitor / OpenHardwareMonitor
/// com múltiplos `SensorType` (`Temperature`, `Fan`, `Clock`, `Power`, `Voltage`, `Control`, `Load`).
pub fn parse_lhm_sensors_json(
    value: &serde_json::Value,
    key: &str,
    source: &str,
    prefix: &str,
) -> Vec<ExternalSensorReading> {
    let items: Vec<&serde_json::Value> = match value.get(key) {
        Some(serde_json::Value::Array(arr)) => arr.iter().collect(),
        Some(single @ serde_json::Value::Object(_)) => vec![single],
        _ => return Vec::new(),
    };

    let mut out = Vec::new();
    for item in items {
        let name = item
            .get("Name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let identifier = item
            .get("Identifier")
            .and_then(|v| v.as_str())
            .unwrap_or(name);
        let sensor_type = item
            .get("SensorType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(raw_val) = item.get("Value").and_then(|v| v.as_f64()) else {
            continue;
        };
        if !raw_val.is_finite() {
            continue;
        }
        let category = match sensor_type {
            "Temperature" if (0.0..=150.0).contains(&raw_val) => SensorCategory::Temperature,
            "Fan" if (0.0..=20_000.0).contains(&raw_val) => SensorCategory::Fan,
            "Clock" if (1.0..=20_000.0).contains(&raw_val) => SensorCategory::Clock,
            "Power" if (0.0..=2_500.0).contains(&raw_val) => SensorCategory::Power,
            "Voltage" if (0.0..=30.0).contains(&raw_val) => SensorCategory::Voltage,
            "Control" if (0.0..=100.0).contains(&raw_val) => SensorCategory::Control,
            "Load" if (0.0..=100.0).contains(&raw_val) => SensorCategory::Load,
            _ => continue,
        };
        let rounded = match category {
            SensorCategory::Voltage => round2(raw_val),
            SensorCategory::Fan => raw_val.round(),
            _ => round1(raw_val),
        };
        let group_hint = infer_group_hint(identifier, name);
        out.push(ExternalSensorReading {
            id: format!("{prefix}-{}", sanitize_id(identifier)),
            label: name.to_owned(),
            category,
            value: rounded,
            unit: category.default_unit().to_owned(),
            source: source.to_owned(),
            group_hint,
        });
    }
    out
}

/// Infere a qual subsistema um sensor pertence pelo identificador/rótulo.
pub fn infer_group_hint(identifier: &str, label: &str) -> Option<String> {
    let hay = format!("{} {}", identifier.to_lowercase(), label.to_lowercase());
    if hay.contains("cpu")
        || hay.contains("core")
        || hay.contains("package")
        || hay.contains("tctl")
        || hay.contains("tdie")
    {
        Some("cpu".into())
    } else if hay.contains("gpu")
        || hay.contains("nvidia")
        || hay.contains("amdgpu")
        || hay.contains("radeon")
        || hay.contains("vram")
    {
        Some("gpu".into())
    } else if hay.contains("nvme")
        || hay.contains("hdd")
        || hay.contains("ssd")
        || hay.contains("disk")
        || hay.contains("drive")
    {
        Some("storage".into())
    } else if hay.contains("ram") || hay.contains("dimm") || hay.contains("memory") {
        Some("ram".into())
    } else if hay.contains("nic")
        || hay.contains("net")
        || hay.contains("eth")
        || hay.contains("wifi")
    {
        Some("network".into())
    } else {
        Some("sensors".into())
    }
}

/// Parser puro do buffer `Global\HWiNFO_SENS_SM2` (Shared Memory do HWiNFO64).
///
/// Layout canônico (packed little-endian):
/// - Cabeçalho (`HWiNFO_SENSORS_SHARED_MEM2`, 44 bytes):
///   - `0..4`: assinatura `0x53695748` (`"HWiS"`)
///   - `4..8`: versão (`u32`)
///   - `8..12`: revisão (`u32`)
///   - `12..20`: `poll_time` (`i64`)
///   - `20..24`: `offset_of_sensor_section` (`u32`)
///   - `24..28`: `size_of_sensor_element` (`u32`)
///   - `28..32`: `num_sensor_elements` (`u32`)
///   - `32..36`: `offset_of_reading_section` (`u32`)
///   - `36..40`: `size_of_reading_element` (`u32`)
///   - `40..44`: `num_reading_elements` (`u32`)
/// - Cada `HWiNFO_SENSORS_READING_ELEMENT` (min 312 bytes):
///   - `0..4`: `tReading` (`u32`: 1=Temp, 2=Volt, 3=Fan, 4=Current, 5=Power, 6=Clock, 7=Usage, 8=Other)
///   - `4..8`: `dwSensorIndex` (`u32`)
///   - `8..12`: `dwReadingID` (`u32`)
///   - `12..140`: `szLabelOrig[128]`
///   - `140..268`: `szLabelUser[128]`
///   - `268..284`: `szUnit[16]`
///   - `284..292`: `Value` (`f64`)
pub fn parse_hwinfo_shared_memory(bytes: &[u8]) -> Vec<ExternalSensorReading> {
    const HWINFO_SIGNATURE: u32 = 0x5369_5748; // "HWiS"
    const HEADER_SIZE: usize = 44;
    const MIN_READING_SIZE: usize = 292;

    if bytes.len() < HEADER_SIZE {
        return Vec::new();
    }
    let read_u32 = |offset: usize| -> Option<u32> {
        bytes
            .get(offset..offset + 4)
            .and_then(|s| s.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let read_f64 = |offset: usize| -> Option<f64> {
        bytes
            .get(offset..offset + 8)
            .and_then(|s| s.try_into().ok())
            .map(f64::from_le_bytes)
    };
    let read_cstr = |slice: &[u8]| -> String {
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        String::from_utf8_lossy(&slice[..end]).trim().to_owned()
    };

    if read_u32(0) != Some(HWINFO_SIGNATURE) {
        return Vec::new();
    }

    let reading_offset = read_u32(32).unwrap_or(0) as usize;
    let reading_size = read_u32(36).unwrap_or(0) as usize;
    let reading_count = read_u32(40).unwrap_or(0).min(512) as usize;

    if reading_size < MIN_READING_SIZE || reading_count == 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for idx in 0..reading_count {
        let Some(start) = reading_offset.checked_add(idx.saturating_mul(reading_size)) else {
            break;
        };
        let Some(end) = start.checked_add(MIN_READING_SIZE) else {
            break;
        };
        if end > bytes.len() {
            break;
        }
        let kind_code = read_u32(start).unwrap_or(0);
        let sensor_index = read_u32(start + 4).unwrap_or(0);
        let reading_id = read_u32(start + 8).unwrap_or(0);
        let label_orig = read_cstr(&bytes[start + 12..start + 140]);
        let label_user = read_cstr(&bytes[start + 140..start + 268]);
        let unit_raw = read_cstr(&bytes[start + 268..start + 284]);
        let Some(val) = read_f64(start + 284) else {
            continue;
        };
        if !val.is_finite() {
            continue;
        }
        let category = match kind_code {
            1 if (0.0..=150.0).contains(&val) => SensorCategory::Temperature,
            2 if (0.0..=30.0).contains(&val) => SensorCategory::Voltage,
            3 if (0.0..=20_000.0).contains(&val) => SensorCategory::Fan,
            5 if (0.0..=2_500.0).contains(&val) => SensorCategory::Power,
            6 if (1.0..=20_000.0).contains(&val) => SensorCategory::Clock,
            7 if (0.0..=100.0).contains(&val) => SensorCategory::Load,
            _ => continue,
        };
        let label = if !label_user.is_empty() {
            label_user
        } else {
            label_orig
        };
        if label.is_empty() {
            continue;
        }
        let unit = if unit_raw.is_empty() || unit_raw.contains('\u{FFFD}') {
            category.default_unit().to_owned()
        } else {
            unit_raw
        };
        let rounded = match category {
            SensorCategory::Voltage => round2(val),
            SensorCategory::Fan => val.round(),
            _ => round1(val),
        };
        let group_hint = infer_group_hint("", &label);
        out.push(ExternalSensorReading {
            id: format!("hwinfo-{sensor_index}-{reading_id:x}"),
            label,
            category,
            value: rounded,
            unit,
            source: "HWiNFO".into(),
            group_hint,
        });
    }
    out
}

/// Constrói o catálogo unificado de `TelemetryMetric` a partir dos subsistemas
/// coletados + sensores externos, sem duplicar IDs.
pub fn build_metric_catalog(
    cpu: &CpuHardwareInfo,
    ram: &RamHardwareInfo,
    gpu: &GpuHardwareInfo,
    disk_io: &DiskIoStats,
    external_sensors: &[ExternalSensorReading],
) -> Vec<TelemetryMetric> {
    let mut metrics = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    let mut push_unique = |metric: TelemetryMetric| {
        if seen_ids.insert(metric.id.clone()) {
            metrics.push(metric);
        }
    };

    // --- CPU ---
    if let Some(clock_mhz) = cpu.current_clock_mhz {
        let ghz = round2(clock_mhz / 1000.0);
        let max_ghz = cpu
            .base_clock_mhz
            .map(|base| round2((base as f64 / 1000.0) * 1.45).max(5.0));
        push_unique(TelemetryMetric {
            id: "cpu.clock_ghz".into(),
            group: "cpu".into(),
            label: "Frequência da CPU".into(),
            short_label: "CLOCK CPU".into(),
            value: Some(ghz),
            unit: "GHz".into(),
            kind: "clock".into(),
            max_value: max_ghz,
            detail: cpu
                .base_clock_mhz
                .map(|b| format!("Base {:.2} GHz", b as f64 / 1000.0)),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    if let Some(threads) = cpu.thread_count {
        push_unique(TelemetryMetric {
            id: "cpu.threads".into(),
            group: "cpu".into(),
            label: "Threads ativas".into(),
            short_label: "THREADS".into(),
            value: Some(threads as f64),
            unit: "".into(),
            kind: "count".into(),
            max_value: None,
            detail: cpu.process_count.map(|p| format!("{p} processos")),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    if let Some(handles) = cpu.handle_count {
        push_unique(TelemetryMetric {
            id: "cpu.handles".into(),
            group: "cpu".into(),
            label: "Handles do sistema".into(),
            short_label: "HANDLES".into(),
            value: Some(handles as f64),
            unit: "".into(),
            kind: "count".into(),
            max_value: None,
            detail: None,
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    // --- RAM ---
    if let Some(avail) = ram.available_gb {
        push_unique(TelemetryMetric {
            id: "ram.available_gb".into(),
            group: "ram".into(),
            label: "RAM disponível".into(),
            short_label: "RAM LIVRE".into(),
            value: Some(round1(avail)),
            unit: "GB".into(),
            kind: "memory".into(),
            max_value: ram.total_gb.map(round1),
            detail: ram.total_gb.map(|t| format!("de {:.1} GB", t)),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    if let Some(cached) = ram.cached_gb {
        push_unique(TelemetryMetric {
            id: "ram.cached_gb".into(),
            group: "ram".into(),
            label: "RAM em cache".into(),
            short_label: "CACHE RAM".into(),
            value: Some(round1(cached)),
            unit: "GB".into(),
            kind: "memory".into(),
            max_value: ram.total_gb.map(round1),
            detail: Some("Standby / cache de páginas".into()),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    if let Some(commit) = ram.commit_used_gb {
        push_unique(TelemetryMetric {
            id: "ram.commit_gb".into(),
            group: "ram".into(),
            label: "Memória comprometida".into(),
            short_label: "COMMIT".into(),
            value: Some(round1(commit)),
            unit: "GB".into(),
            kind: "memory".into(),
            max_value: ram.commit_limit_gb.map(round1),
            detail: ram.commit_limit_gb.map(|l| format!("Limite {:.1} GB", l)),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    // --- GPU ---
    if let Some(vram_used) = gpu.vram_used_gb {
        let total = gpu.vram_total_gb;
        push_unique(TelemetryMetric {
            id: "gpu.vram_used_gb".into(),
            group: "gpu".into(),
            label: "VRAM dedicada em uso".into(),
            short_label: "VRAM".into(),
            value: Some(round2(vram_used)),
            unit: "GB".into(),
            kind: "memory".into(),
            max_value: total.map(round2),
            detail: total.map(|t| format!("{:.1} / {:.1} GB", vram_used, t)),
            source: gpu.source.clone().unwrap_or_else(|| "PDH".into()),
            unavailable_reason: None,
        });
    }

    if let Some(shared_used) = gpu.shared_used_gb {
        push_unique(TelemetryMetric {
            id: "gpu.shared_used_gb".into(),
            group: "gpu".into(),
            label: "Memória GPU compartilhada".into(),
            short_label: "VRAM COMP.".into(),
            value: Some(round2(shared_used)),
            unit: "GB".into(),
            kind: "memory".into(),
            max_value: None,
            detail: gpu.name.clone(),
            source: gpu.source.clone().unwrap_or_else(|| "PDH".into()),
            unavailable_reason: None,
        });
    }

    if let Some(temp) = gpu.temperature_celsius {
        push_unique(TelemetryMetric {
            id: "gpu.temp".into(),
            group: "gpu".into(),
            label: "Temperatura da GPU".into(),
            short_label: "TEMP GPU".into(),
            value: Some(round1(temp)),
            unit: "°C".into(),
            kind: "temperature".into(),
            max_value: Some(100.0),
            detail: gpu.name.clone(),
            source: gpu.source.clone().unwrap_or_else(|| "Driver".into()),
            unavailable_reason: None,
        });
    }

    if let Some(fan_pct) = gpu.fan_percent {
        push_unique(TelemetryMetric {
            id: "gpu.fan_percent".into(),
            group: "gpu".into(),
            label: "Ventoinha da GPU".into(),
            short_label: "FAN GPU".into(),
            value: Some(round1(fan_pct)),
            unit: "%".into(),
            kind: "percent".into(),
            max_value: Some(100.0),
            detail: gpu.fan_rpm.map(|rpm| format!("{} RPM", rpm.round() as u32)),
            source: gpu.source.clone().unwrap_or_else(|| "Driver".into()),
            unavailable_reason: None,
        });
    } else if let Some(fan_rpm) = gpu.fan_rpm {
        push_unique(TelemetryMetric {
            id: "gpu.fan_rpm".into(),
            group: "gpu".into(),
            label: "Ventoinha da GPU (RPM)".into(),
            short_label: "FAN GPU".into(),
            value: Some(fan_rpm.round()),
            unit: "RPM".into(),
            kind: "fan".into(),
            max_value: None,
            detail: gpu.name.clone(),
            source: gpu.source.clone().unwrap_or_else(|| "Driver".into()),
            unavailable_reason: None,
        });
    }

    if let Some(clock) = gpu.core_clock_mhz {
        push_unique(TelemetryMetric {
            id: "gpu.core_clock_mhz".into(),
            group: "gpu".into(),
            label: "Clock do núcleo GPU".into(),
            short_label: "CLOCK GPU".into(),
            value: Some(round1(clock)),
            unit: "MHz".into(),
            kind: "clock".into(),
            max_value: None,
            detail: gpu.memory_clock_mhz.map(|m| format!("VRAM {:.0} MHz", m)),
            source: gpu.source.clone().unwrap_or_else(|| "Driver".into()),
            unavailable_reason: None,
        });
    }

    if let Some(watts) = gpu.power_watts {
        push_unique(TelemetryMetric {
            id: "gpu.power_watts".into(),
            group: "gpu".into(),
            label: "Consumo da GPU".into(),
            short_label: "ENERGIA GPU".into(),
            value: Some(round1(watts)),
            unit: "W".into(),
            kind: "power".into(),
            max_value: gpu.power_limit_watts.map(round1),
            detail: gpu.power_limit_watts.map(|l| format!("Limite {:.0} W", l)),
            source: gpu.source.clone().unwrap_or_else(|| "Driver".into()),
            unavailable_reason: None,
        });
    }

    // --- Disco I/O ---
    if let Some(read_mbps) = disk_io.read_mbps {
        push_unique(TelemetryMetric {
            id: "disk.read_mbps".into(),
            group: "storage".into(),
            label: "Leitura de disco".into(),
            short_label: "DISCO LER".into(),
            value: Some(round1(read_mbps)),
            unit: "MB/s".into(),
            kind: "rate".into(),
            max_value: None,
            detail: disk_io.read_iops.map(|iops| format!("{:.0} IOPS", iops)),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    if let Some(write_mbps) = disk_io.write_mbps {
        push_unique(TelemetryMetric {
            id: "disk.write_mbps".into(),
            group: "storage".into(),
            label: "Escrita de disco".into(),
            short_label: "DISCO ESCR.".into(),
            value: Some(round1(write_mbps)),
            unit: "MB/s".into(),
            kind: "rate".into(),
            max_value: None,
            detail: disk_io.write_iops.map(|iops| format!("{:.0} IOPS", iops)),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    if let Some(active_pct) = disk_io.active_percent {
        push_unique(TelemetryMetric {
            id: "disk.active_percent".into(),
            group: "storage".into(),
            label: "Tempo ativo do disco".into(),
            short_label: "DISCO ATIVO".into(),
            value: Some(round1(active_pct.clamp(0.0, 100.0))),
            unit: "%".into(),
            kind: "percent".into(),
            max_value: Some(100.0),
            detail: disk_io.queue_length.map(|q| format!("Fila {:.1}", q)),
            source: "Sistema".into(),
            unavailable_reason: None,
        });
    }

    for disk in &disk_io.per_disk {
        let disk_slug = sanitize_id(&disk.name);
        if let Some(read_mbps) = disk.read_mbps {
            push_unique(TelemetryMetric {
                id: format!("disk.dev.{disk_slug}.read_mbps"),
                group: "storage".into(),
                label: format!("Leitura ({})", disk.name),
                short_label: shorten_sensor_label(&format!("LER {}", disk.name), 14),
                value: Some(round1(read_mbps)),
                unit: "MB/s".into(),
                kind: "rate".into(),
                max_value: None,
                detail: Some(disk.name.clone()),
                source: "Sistema".into(),
                unavailable_reason: None,
            });
        }
        if let Some(write_mbps) = disk.write_mbps {
            push_unique(TelemetryMetric {
                id: format!("disk.dev.{disk_slug}.write_mbps"),
                group: "storage".into(),
                label: format!("Escrita ({})", disk.name),
                short_label: shorten_sensor_label(&format!("ESCR {}", disk.name), 14),
                value: Some(round1(write_mbps)),
                unit: "MB/s".into(),
                kind: "rate".into(),
                max_value: None,
                detail: Some(disk.name.clone()),
                source: "Sistema".into(),
                unavailable_reason: None,
            });
        }
    }

    // --- Sensores externos (LHM / OHM / HWiNFO / hwmon) ---
    // Promove primeiro sensor térmico de CPU encontrado para o ID canônico `cpu.temp`
    if let Some(cpu_temp) = external_sensors.iter().find(|s| {
        s.category == SensorCategory::Temperature && s.group_hint.as_deref() == Some("cpu")
    }) {
        push_unique(TelemetryMetric {
            id: "cpu.temp".into(),
            group: "cpu".into(),
            label: format!("Temperatura CPU ({})", cpu_temp.label),
            short_label: "TEMP CPU".into(),
            value: Some(cpu_temp.value),
            unit: "°C".into(),
            kind: "temperature".into(),
            max_value: Some(100.0),
            detail: Some(cpu_temp.source.clone()),
            source: cpu_temp.source.clone(),
            unavailable_reason: None,
        });
    }

    // Promove consumo de CPU (Package Power) para `cpu.power_watts` se existir
    if let Some(cpu_power) = external_sensors
        .iter()
        .find(|s| s.category == SensorCategory::Power && s.group_hint.as_deref() == Some("cpu"))
    {
        push_unique(TelemetryMetric {
            id: "cpu.power_watts".into(),
            group: "cpu".into(),
            label: format!("Consumo CPU ({})", cpu_power.label),
            short_label: "ENERGIA CPU".into(),
            value: Some(cpu_power.value),
            unit: "W".into(),
            kind: "power".into(),
            max_value: None,
            detail: Some(cpu_power.source.clone()),
            source: cpu_power.source.clone(),
            unavailable_reason: None,
        });
    }

    // Adiciona todos os sensores individuais no catálogo (limite seguro para UI leve)
    for sensor in external_sensors.iter().take(48) {
        let group = sensor
            .group_hint
            .clone()
            .unwrap_or_else(|| "sensors".into());
        let short = shorten_sensor_label(&sensor.label, 14);
        let max_value = match sensor.category {
            SensorCategory::Temperature => Some(100.0),
            SensorCategory::Control | SensorCategory::Load => Some(100.0),
            _ => None,
        };
        push_unique(TelemetryMetric {
            id: format!("sensor.{}", sensor.id),
            group,
            label: sensor.label.clone(),
            short_label: short,
            value: Some(sensor.value),
            unit: sensor.unit.clone(),
            kind: sensor.category.kind_str().into(),
            max_value,
            detail: Some(sensor.source.clone()),
            source: sensor.source.clone(),
            unavailable_reason: None,
        });
    }

    metrics
}

fn shorten_sensor_label(label: &str, max_chars: usize) -> String {
    let trimmed = label.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_uppercase();
    }
    let shortened: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", shortened.trim_end().to_uppercase())
}

// ============================================================================
// Persistência de preferências (`telemetry-preferences.json`)
// ============================================================================

fn preferences_file() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join("telemetry-preferences.json")
}

pub fn sanitize_preferences(mut prefs: TelemetryPreferences) -> TelemetryPreferences {
    let mut seen = HashSet::new();
    prefs.tray_metrics.retain(|id| {
        let trimmed = id.trim();
        !trimmed.is_empty() && seen.insert(trimmed.to_owned())
    });
    prefs.tray_metrics.truncate(MAX_TRAY_METRICS);
    if prefs.tray_metrics.is_empty() {
        prefs.tray_metrics = TelemetryPreferences::default().tray_metrics;
    }

    for list in prefs.card_metrics.values_mut() {
        let mut group_seen = HashSet::new();
        list.retain(|id| {
            let trimmed = id.trim();
            !trimmed.is_empty() && group_seen.insert(trimmed.to_owned())
        });
        list.truncate(12);
    }

    let mut order_seen = HashSet::new();
    let mut clean_order = Vec::with_capacity(CARD_ORDER_GROUPS.len());
    for group in prefs.card_order {
        let trimmed = group.trim().to_ascii_lowercase();
        if CARD_ORDER_GROUPS.contains(&trimmed.as_str()) && order_seen.insert(trimmed.clone()) {
            clean_order.push(trimmed);
        }
    }
    for default_group in CARD_ORDER_GROUPS {
        if order_seen.insert(default_group.to_owned()) {
            clean_order.push(default_group.to_owned());
        }
    }
    prefs.card_order = clean_order;

    prefs
}

pub fn load_preferences() -> TelemetryPreferences {
    fs::read_to_string(preferences_file())
        .ok()
        .and_then(|content| serde_json::from_str::<TelemetryPreferences>(&content).ok())
        .map(sanitize_preferences)
        .unwrap_or_default()
}

pub fn save_preferences(
    preferences: &TelemetryPreferences,
) -> Result<TelemetryPreferences, String> {
    let clean = sanitize_preferences(preferences.clone());
    let file = preferences_file();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    let content = serde_json::to_string_pretty(&clean).map_err(|error| error.to_string())?;
    fs::write(file, content).map_err(|error| error.to_string())?;
    Ok(clean)
}

#[tauri::command]
pub fn get_telemetry_preferences() -> TelemetryPreferences {
    load_preferences()
}

#[tauri::command]
pub fn set_telemetry_preferences(
    app: tauri::AppHandle,
    preferences: TelemetryPreferences,
) -> Result<TelemetryPreferences, String> {
    let saved = save_preferences(&preferences)?;
    let _ = app.emit("pulse://telemetry-preferences-updated", &saved);
    Ok(saved)
}

// ============================================================================
// Coleta com cache curto (evita trabalho duplicado entre janela principal e tray)
// ============================================================================

static SNAPSHOT_CACHE: Mutex<Option<(Instant, HardwareTelemetrySnapshot)>> = Mutex::new(None);

pub fn collect_hardware_telemetry_sync() -> HardwareTelemetrySnapshot {
    if let Ok(guard) = SNAPSHOT_CACHE.lock() {
        if let Some((at, cached)) = guard.as_ref() {
            if at.elapsed() < SNAPSHOT_CACHE_TTL {
                return cached.clone();
            }
        }
    }

    let snapshot = collect_platform_snapshot();
    if let Ok(mut guard) = SNAPSHOT_CACHE.lock() {
        *guard = Some((Instant::now(), snapshot.clone()));
    }
    snapshot
}

fn collect_platform_snapshot() -> HardwareTelemetrySnapshot {
    #[cfg(windows)]
    {
        windows::collect_snapshot()
    }
    #[cfg(target_os = "linux")]
    {
        linux::collect_snapshot()
    }
    #[cfg(target_os = "macos")]
    {
        macos::collect_snapshot()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        HardwareTelemetrySnapshot {
            cpu: CpuHardwareInfo::default(),
            ram: RamHardwareInfo::default(),
            gpu: GpuHardwareInfo::default(),
            disk_io: DiskIoStats::default(),
            metrics: Vec::new(),
            external_providers: Vec::new(),
            sampled_at_unix_ms: now_unix_ms(),
        }
    }
}

#[tauri::command]
pub async fn get_hardware_telemetry() -> HardwareTelemetrySnapshot {
    let started = Instant::now();
    let snapshot = tauri::async_runtime::spawn_blocking(collect_hardware_telemetry_sync)
        .await
        .unwrap_or_else(|_| HardwareTelemetrySnapshot {
            cpu: CpuHardwareInfo::default(),
            ram: RamHardwareInfo::default(),
            gpu: GpuHardwareInfo::default(),
            disk_io: DiskIoStats::default(),
            metrics: Vec::new(),
            external_providers: Vec::new(),
            sampled_at_unix_ms: now_unix_ms(),
        });
    crate::debug::record("hardware_telemetry", started.elapsed().as_millis(), Ok(()));
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_extended_csv_with_missing_fields() {
        let csv = "0, NVIDIA GeForce RTX 4070 SUPER, 62, 48, 2475, 10501, 142.35, 220.00, 4096, 12288, 73\n\
                   1, NVIDIA Quadro P400, 51, [Not Supported], 1227, 2003, N/A, N/A, 512, 2048, 12\n";
        let samples = parse_nvidia_extended_csv(csv);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].name, "NVIDIA GeForce RTX 4070 SUPER");
        assert_eq!(samples[0].temperature_celsius, Some(62.0));
        assert_eq!(samples[0].fan_percent, Some(48.0));
        assert_eq!(samples[0].core_clock_mhz, Some(2475.0));
        assert_eq!(samples[0].power_watts, Some(142.4));
        assert_eq!(samples[0].vram_used_mib, Some(4096.0));
        assert_eq!(samples[0].vram_total_mib, Some(12288.0));
        assert_eq!(samples[0].utilization_percent, Some(73.0));

        assert_eq!(samples[1].fan_percent, None);
        assert_eq!(samples[1].power_watts, None);
        assert_eq!(samples[1].temperature_celsius, Some(51.0));
    }

    #[test]
    fn parses_lhm_multiple_sensor_types() {
        let json = serde_json::json!({
            "lhm": [
                { "Name": "CPU Package", "Identifier": "/intelcpu/0/temperature/0", "SensorType": "Temperature", "Value": 58.4 },
                { "Name": "Fan #1", "Identifier": "/lpc/nct6798d/fan/0", "SensorType": "Fan", "Value": 1245.2 },
                { "Name": "CPU Package", "Identifier": "/intelcpu/0/power/0", "SensorType": "Power", "Value": 44.8 },
                { "Name": "Vcore", "Identifier": "/lpc/nct6798d/voltage/0", "SensorType": "Voltage", "Value": 1.184 },
                { "Name": "Broken", "Identifier": "/bad", "SensorType": "Temperature", "Value": 999.0 }
            ]
        });
        let sensors = parse_lhm_sensors_json(&json, "lhm", "LibreHardwareMonitor", "lhm");
        assert_eq!(sensors.len(), 4);
        assert_eq!(sensors[0].category, SensorCategory::Temperature);
        assert_eq!(sensors[1].category, SensorCategory::Fan);
        assert_eq!(sensors[1].value, 1245.0);
        assert_eq!(sensors[2].category, SensorCategory::Power);
        assert_eq!(sensors[3].category, SensorCategory::Voltage);
        assert_eq!(sensors[3].value, 1.18);
    }

    #[test]
    fn parses_hwinfo_shared_memory_buffer() {
        let reading_size = 312usize;
        let reading_offset = 64usize;
        let mut buf = vec![0u8; reading_offset + reading_size * 2];
        // Header
        buf[0..4].copy_from_slice(&0x5369_5748u32.to_le_bytes()); // HWiS
        buf[32..36].copy_from_slice(&(reading_offset as u32).to_le_bytes());
        buf[36..40].copy_from_slice(&(reading_size as u32).to_le_bytes());
        buf[40..44].copy_from_slice(&2u32.to_le_bytes());

        // Reading 0: Fan RPM
        let r0 = reading_offset;
        buf[r0..r0 + 4].copy_from_slice(&3u32.to_le_bytes()); // Fan
        buf[r0 + 4..r0 + 8].copy_from_slice(&1u32.to_le_bytes());
        buf[r0 + 8..r0 + 12].copy_from_slice(&0x10u32.to_le_bytes());
        buf[r0 + 12..r0 + 12 + 7].copy_from_slice(b"CPU Fan");
        buf[r0 + 268..r0 + 268 + 3].copy_from_slice(b"RPM");
        buf[r0 + 284..r0 + 292].copy_from_slice(&1380.4f64.to_le_bytes());

        // Reading 1: CPU Temp
        let r1 = reading_offset + reading_size;
        buf[r1..r1 + 4].copy_from_slice(&1u32.to_le_bytes()); // Temp
        buf[r1 + 4..r1 + 8].copy_from_slice(&0u32.to_le_bytes());
        buf[r1 + 8..r1 + 12].copy_from_slice(&0x20u32.to_le_bytes());
        buf[r1 + 140..r1 + 140 + 11].copy_from_slice(b"CPU Package");
        buf[r1 + 268..r1 + 268 + 2].copy_from_slice(b"C\0");
        buf[r1 + 284..r1 + 292].copy_from_slice(&61.75f64.to_le_bytes());

        let parsed = parse_hwinfo_shared_memory(&buf);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].label, "CPU Fan");
        assert_eq!(parsed[0].value, 1380.0);
        assert_eq!(parsed[0].unit, "RPM");
        assert_eq!(parsed[1].label, "CPU Package");
        assert_eq!(parsed[1].value, 61.8);
    }

    #[test]
    fn sanitizes_preferences_enforcing_eight_slot_limit_and_dedup() {
        let prefs = TelemetryPreferences {
            tray_metrics: vec![
                "cpu.usage".into(),
                "cpu.usage".into(),
                "".into(),
                "gpu.temp".into(),
                "gpu.fan_percent".into(),
                "disk.read_mbps".into(),
                "net.download".into(),
                "net.upload".into(),
                "ram.usage".into(),
                "cpu.temp".into(),
                "disk.write_mbps".into(),
                "gpu.usage".into(),
            ],
            card_metrics: HashMap::new(),
            card_order: vec!["gpu".into(), "gpu".into(), "invalid".into(), "cpu".into()],
        };
        let clean = sanitize_preferences(prefs);
        assert_eq!(
            clean.tray_metrics,
            vec![
                "cpu.usage",
                "gpu.temp",
                "gpu.fan_percent",
                "disk.read_mbps",
                "net.download",
                "net.upload",
                "ram.usage",
                "cpu.temp",
            ]
        );
        assert_eq!(
            clean.card_order,
            vec!["gpu", "cpu", "ram", "network", "storage"]
        );
    }

    #[test]
    fn deserializes_legacy_preferences_without_card_order() {
        let raw = r#"{"trayMetrics":["cpu.usage"],"cardMetrics":{}}"#;
        let parsed: TelemetryPreferences = serde_json::from_str(raw).unwrap();
        let clean = sanitize_preferences(parsed);
        assert_eq!(
            clean.card_order,
            vec!["cpu", "ram", "gpu", "network", "storage"]
        );
    }

    #[test]
    fn converts_pdh_kelvin_and_selects_gpu_memory_by_luid() {
        assert_eq!(kelvin_to_valid_celsius(315.15), Some(42.0));
        assert_eq!(kelvin_to_valid_celsius(283.15), Some(10.0));
        assert_eq!(kelvin_to_valid_celsius(393.15), Some(120.0));
        assert_eq!(kelvin_to_valid_celsius(200.0), None);
        assert_eq!(kelvin_to_valid_celsius(500.0), None);
        assert_eq!(kelvin_to_valid_celsius(f64::NAN), None);

        let samples = vec![
            (
                "luid_0x00000000_0x00001111_phys_0".into(),
                256.0 * 1024.0 * 1024.0,
            ),
            (
                "luid_0x00000000_0x00002222_phys_0".into(),
                4.0 * 1024.0 * 1024.0 * 1024.0,
            ),
        ];
        assert_eq!(
            select_gpu_memory_usage_gb(&samples, Some("luid_0x00000000_0x00001111")),
            Some(0.25)
        );
        assert_eq!(
            select_gpu_memory_usage_gb(&samples, Some("luid_0x00000000_0x00009999")),
            Some(4.0)
        );
        assert_eq!(select_gpu_memory_usage_gb(&[], None), None);
    }

    #[test]
    fn builds_catalog_with_promoted_cpu_temp_and_gpu_metrics() {
        let cpu = CpuHardwareInfo {
            model: Some("Ryzen 7".into()),
            logical_cores: 16,
            base_clock_mhz: Some(3800),
            current_clock_mhz: Some(4450.0),
            ..Default::default()
        };
        let gpu = GpuHardwareInfo {
            name: Some("RTX 4070".into()),
            vram_used_gb: Some(4.2),
            vram_total_gb: Some(12.0),
            temperature_celsius: Some(59.0),
            fan_percent: Some(42.0),
            power_watts: Some(110.5),
            ..Default::default()
        };
        let external = vec![ExternalSensorReading {
            id: "lhm-cpu-pkg".into(),
            label: "CPU Package".into(),
            category: SensorCategory::Temperature,
            value: 55.2,
            unit: "°C".into(),
            source: "LibreHardwareMonitor".into(),
            group_hint: Some("cpu".into()),
        }];
        let catalog = build_metric_catalog(
            &cpu,
            &RamHardwareInfo::default(),
            &gpu,
            &DiskIoStats::default(),
            &external,
        );
        assert!(catalog.iter().any(|m| m.id == "cpu.clock_ghz"));
        assert!(catalog
            .iter()
            .any(|m| m.id == "cpu.temp" && m.value == Some(55.2)));
        assert!(catalog.iter().any(|m| m.id == "gpu.vram_used_gb"));
        assert!(catalog.iter().any(|m| m.id == "gpu.fan_percent"));
        assert!(catalog.iter().any(|m| m.id == "gpu.power_watts"));
    }
}
