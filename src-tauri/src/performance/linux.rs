//! Telemetria de hardware do módulo Desempenho no Linux.
//!
//! - GPU: placas em `/sys/class/drm/card*`; nome via `nvidia-smi` (NVIDIA) ou
//!   `lspci -vmm` (demais), com cache — o hardware não muda em execução.
//! - VRAM: `nvidia-smi` (NVIDIA) ou `mem_info_vram_total` (amdgpu). GPU
//!   integrada usa a memória do sistema: indisponível com motivo.
//! - Temperaturas: `/sys/class/hwmon/*/temp*_input` com rótulo honesto da
//!   origem, mais a GPU NVIDIA pelo `nvidia-smi`.
//! - GPU dedicada suspensa (runtime PM de notebooks híbridos) não é acordada:
//!   o `nvidia-smi` só roda com a placa ativa.

use super::{
    hottest_sensor, sanitize_id, valid_celsius, Capability, PerformanceDetails, ThermalSensor,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_SENSORS: usize = 16;
const MIB: u64 = 1024 * 1024;

/// Estado de energia das GPUs NVIDIA ligadas ao driver proprietário.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvidiaPower {
    /// Sem placa no driver `nvidia` (ou driver não carregado).
    Absent,
    /// Todas as placas em runtime suspend: consultar acordaria a GPU.
    Suspended,
    Awake,
}

fn nvidia_pci_devices() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/sys/bus/pci/drivers/nvidia") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            // Só os links de dispositivo PCI (`0000:01:00.0`).
            entry.file_name().to_string_lossy().matches(':').count() == 2
        })
        .map(|entry| entry.path())
        .collect()
}

pub fn nvidia_power_state() -> NvidiaPower {
    let devices = nvidia_pci_devices();
    if devices.is_empty() {
        return NvidiaPower::Absent;
    }
    let all_suspended = devices.iter().all(|device| {
        std::fs::read_to_string(device.join("power/runtime_status"))
            .map(|status| status.trim() == "suspended")
            .unwrap_or(false)
    });
    if all_suspended {
        NvidiaPower::Suspended
    } else {
        NvidiaPower::Awake
    }
}

/// Executa `nvidia-smi` com argumentos fixos e devolve o stdout em caso de
/// sucesso. `None` quando o binário não existe ou o driver não respondeu.
pub fn run_nvidia_smi(args: &[&str]) -> Option<String> {
    let output = crate::process::c_locale_command("nvidia-smi")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    String::from_utf8(output.stdout).ok()
}

/// Uma linha de `nvidia-smi --query-gpu=index,pci.bus_id,name,memory.total,temperature.gpu`.
#[derive(Debug, Clone, PartialEq)]
pub struct NvidiaGpu {
    pub index: u32,
    /// Endereço PCI normalizado como no sysfs (`0000:01:00.0`).
    pub slot: String,
    pub name: String,
    pub memory_total_mib: Option<u64>,
    pub temperature: Option<f64>,
}

fn numeric_field(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// O `nvidia-smi` usa domínio PCI de 8 dígitos (`00000000:01:00.0`); o sysfs
/// usa 4 (`0000:01:00.0`).
pub fn normalize_pci_slot(raw: &str) -> String {
    let raw = raw.trim().to_lowercase();
    match raw.split_once(':') {
        Some((domain, rest)) if domain.len() > 4 => {
            format!("{}:{rest}", &domain[domain.len() - 4..])
        }
        _ => raw,
    }
}

/// Interpreta a consulta CSV (`noheader,nounits`). Campos `[N/A]` ou
/// `[Not Supported]` viram `None`, nunca zero.
pub fn parse_nvidia_query(text: &str) -> Vec<NvidiaGpu> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            if fields.len() < 5 {
                return None;
            }
            let index = fields[0].parse::<u32>().ok()?;
            // O nome pode conter vírgula: tudo entre o bus id e os 2 últimos.
            let name = fields[2..fields.len() - 2].join(",").trim().to_owned();
            Some(NvidiaGpu {
                index,
                slot: normalize_pci_slot(fields[1]),
                name,
                memory_total_mib: numeric_field(fields[fields.len() - 2])
                    .filter(|v| *v > 0.0)
                    .map(|v| v as u64),
                temperature: numeric_field(fields[fields.len() - 1]).and_then(valid_celsius),
            })
        })
        .collect()
}

type NvidiaStaticCache = HashMap<String, (String, Option<u64>)>;

/// Nome e VRAM da NVIDIA não mudam: guardados para os momentos em que a
/// placa está suspensa e não deve ser acordada.
static NVIDIA_STATIC: Mutex<Option<NvidiaStaticCache>> = Mutex::new(None);

fn query_nvidia() -> Vec<NvidiaGpu> {
    if nvidia_power_state() != NvidiaPower::Awake {
        return Vec::new();
    }
    let Some(text) = run_nvidia_smi(&[
        "--query-gpu=index,pci.bus_id,name,memory.total,temperature.gpu",
        "--format=csv,noheader,nounits",
    ]) else {
        return Vec::new();
    };
    let gpus = parse_nvidia_query(&text);
    if let Ok(mut guard) = NVIDIA_STATIC.lock() {
        let cache = guard.get_or_insert_with(HashMap::new);
        for gpu in &gpus {
            cache.insert(gpu.slot.clone(), (gpu.name.clone(), gpu.memory_total_mib));
        }
    }
    gpus
}

fn cached_nvidia(slot: &str) -> Option<(String, Option<u64>)> {
    NVIDIA_STATIC.lock().ok()?.as_ref()?.get(slot).cloned()
}

/// Monta um nome legível a partir de `lspci -vmm -s <slot>`:
/// `Device: GA107M [GeForce RTX 3050 Mobile]` + `Vendor: NVIDIA Corporation`
/// → `NVIDIA GeForce RTX 3050 Mobile`.
pub fn parse_lspci_name(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let device = field("Device:")?;
    let vendor = field("Vendor:");
    let device = match (device.find('['), device.rfind(']')) {
        (Some(start), Some(end)) if end > start + 1 => device[start + 1..end].to_owned(),
        _ => device,
    };
    let vendor_short = vendor.map(|vendor| {
        if vendor.contains("AMD") || vendor.starts_with("Advanced Micro Devices") {
            "AMD".to_owned()
        } else {
            vendor
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_end_matches(',')
                .to_owned()
        }
    });
    Some(match vendor_short {
        Some(vendor)
            if !vendor.is_empty() && !device.to_lowercase().starts_with(&vendor.to_lowercase()) =>
        {
            format!("{vendor} {device}")
        }
        _ => device,
    })
}

static LSPCI_NAMES: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

fn lspci_name(slot: &str) -> Option<String> {
    if let Ok(guard) = LSPCI_NAMES.lock() {
        if let Some(name) = guard.as_ref().and_then(|cache| cache.get(slot)) {
            return name.clone();
        }
    }
    // O slot vem do sysfs; ainda assim só aceita o formato PCI esperado.
    let valid = slot.len() <= 16
        && slot
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    let name = if valid {
        crate::process::c_locale_command("lspci")
            .args(["-vmm", "-s", slot])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| parse_lspci_name(&text))
    } else {
        None
    };
    if let Ok(mut guard) = LSPCI_NAMES.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(slot.to_owned(), name.clone());
    }
    name
}

#[derive(Debug, Clone)]
struct GpuAdapter {
    slot: String,
    driver: String,
    device_dir: PathBuf,
    discrete: bool,
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn list_gpu_adapters() -> Vec<GpuAdapter> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut adapters: Vec<GpuAdapter> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // `card0`, `card1`… (conectores são `card0-eDP-1`).
        if !name.starts_with("card") || !name[4..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(device_dir) = std::fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(slot) = device_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            continue;
        };
        if adapters.iter().any(|adapter| adapter.slot == slot) {
            continue;
        }
        let driver = std::fs::read_link(device_dir.join("driver"))
            .ok()
            .and_then(|link| link.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let vendor = read_trimmed(&device_dir.join("vendor")).unwrap_or_default();
        let boot_vga = read_trimmed(&device_dir.join("boot_vga"));
        let discrete = vendor == "0x10de"
            || (vendor == "0x1002" && boot_vga.as_deref() != Some("1"))
            || driver == "nvidia";
        adapters.push(GpuAdapter {
            slot,
            driver,
            device_dir,
            discrete,
        });
    }
    // Dedicada primeiro (mesma escolha do Windows, que prefere a de mais VRAM).
    adapters.sort_by(|a, b| {
        b.discrete
            .cmp(&a.discrete)
            .then_with(|| a.slot.cmp(&b.slot))
    });
    adapters
}

/// Uma leitura crua de `temp*_input` do hwmon.
#[derive(Debug, Clone, PartialEq)]
pub struct HwmonReading {
    /// Conteúdo de `name` (ex.: `coretemp`, `nvme`).
    pub chip: String,
    /// Diretório (`hwmon3`), para ids únicos.
    pub hwmon: String,
    /// Prefixo do arquivo (`temp1`).
    pub input: String,
    pub label: Option<String>,
    pub millidegrees: i64,
    /// Modelo do dispositivo dono (disco), quando exposto.
    pub device_model: Option<String>,
}

fn read_hwmon() -> Vec<HwmonReading> {
    let mut readings = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") else {
        return readings;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let hwmon = entry.file_name().to_string_lossy().into_owned();
        let Some(chip) = read_trimmed(&dir.join("name")) else {
            continue;
        };
        let device_model = read_trimmed(&dir.join("device/model"));
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in files.flatten() {
            let file_name = file.file_name().to_string_lossy().into_owned();
            let Some(input) = file_name.strip_suffix("_input") else {
                continue;
            };
            if !input.starts_with("temp") {
                continue;
            }
            // Sensores que falham na leitura (EIO/ENODATA) somem, sem zero.
            let Some(value) = read_trimmed(&file.path()).and_then(|v| v.parse::<i64>().ok()) else {
                continue;
            };
            readings.push(HwmonReading {
                chip: chip.clone(),
                hwmon: hwmon.clone(),
                input: input.to_owned(),
                label: read_trimmed(&dir.join(format!("{input}_label"))),
                millidegrees: value,
                device_model: device_model.clone(),
            });
        }
    }
    readings
}

/// Rótulo honesto e prioridade de exibição (menor = mais importante). A CPU
/// só é chamada de CPU quando o driver é de CPU ou o firmware a rotula assim.
fn describe_hwmon(reading: &HwmonReading, ram_index: usize) -> (String, u8) {
    let chip = reading.chip.as_str();
    let label = reading
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    let number = reading.input.trim_start_matches("temp");
    match chip {
        "coretemp" => match label {
            Some(l) if l.starts_with("Package id") => {
                let id = l.trim_start_matches("Package id").trim();
                if id == "0" {
                    ("CPU Package".into(), 0)
                } else {
                    (format!("CPU Package {id}"), 0)
                }
            }
            Some(l) if l.starts_with("Core") => (format!("CPU {l}"), 6),
            Some(l) => (format!("CPU {l}"), 6),
            None => (format!("CPU sensor {number}"), 6),
        },
        "k10temp" | "zenpower" => match label {
            Some(l @ ("Tctl" | "Tdie")) => (format!("CPU ({l})"), 0),
            Some(l) => (format!("CPU {l}"), 6),
            None => ("CPU".into(), 0),
        },
        "nvme" => {
            let disk = reading
                .device_model
                .as_deref()
                .map(|model| format!("SSD {model}"))
                .unwrap_or_else(|| "SSD NVMe".into());
            match label {
                Some("Composite") | None => (disk, 2),
                Some(l) => (format!("{disk} · {l}"), 5),
            }
        }
        "drivetemp" => (
            reading
                .device_model
                .as_deref()
                .map(|model| format!("Disco {model}"))
                .unwrap_or_else(|| "Disco SATA".into()),
            2,
        ),
        "amdgpu" | "radeon" | "nouveau" => {
            (format!("GPU {} ({})", chip, label.unwrap_or("edge")), 1)
        }
        "spd5118" | "jc42" => (format!("Memória RAM · módulo {ram_index}"), 4),
        "acpitz" => (format!("Zona térmica ACPI {number}"), 5),
        _ if chip.starts_with("iwlwifi") => ("Wi-Fi".into(), 4),
        _ if chip.starts_with("pch_") => ("Chipset (PCH)".into(), 4),
        _ if chip.starts_with("BAT") || chip.contains("battery") => ("Bateria".into(), 4),
        _ => match label {
            Some(l) if !l.eq_ignore_ascii_case("unknown") => (format!("{l} ({chip})"), 3),
            _ => (format!("Sensor {number} ({chip})"), 7),
        },
    }
}

/// Converte leituras do hwmon em sensores ordenados por relevância.
/// Função pura para testes com fixtures.
pub fn hwmon_sensors(readings: &[HwmonReading]) -> Vec<(u8, ThermalSensor)> {
    let mut sorted: Vec<&HwmonReading> = readings.iter().collect();
    sorted.sort_by(|a, b| {
        a.hwmon
            .len()
            .cmp(&b.hwmon.len())
            .then_with(|| a.hwmon.cmp(&b.hwmon))
            .then_with(|| a.input.len().cmp(&b.input.len()))
            .then_with(|| a.input.cmp(&b.input))
    });
    let mut ram_modules: Vec<&str> = Vec::new();
    let mut sensors = Vec::new();
    for reading in sorted {
        let Some(celsius) = valid_celsius(reading.millidegrees as f64 / 1000.0) else {
            continue;
        };
        let ram_index = if matches!(reading.chip.as_str(), "spd5118" | "jc42") {
            if !ram_modules.contains(&reading.hwmon.as_str()) {
                ram_modules.push(&reading.hwmon);
            }
            ram_modules.len()
        } else {
            0
        };
        let (label, priority) = describe_hwmon(reading, ram_index);
        let prefix = if priority == 1 { "gpu-hwmon" } else { "hwmon" };
        sensors.push((
            priority,
            ThermalSensor {
                id: format!(
                    "{prefix}-{}",
                    sanitize_id(&format!(
                        "{}-{}-{}",
                        reading.chip, reading.hwmon, reading.input
                    ))
                ),
                label,
                celsius,
                source: format!("hwmon ({})", reading.chip),
            },
        ));
    }
    sensors
}

fn capability<T>(value: Option<T>, reason: &str) -> Capability<T> {
    match value {
        Some(value) => Capability {
            value: Some(value),
            unavailable_reason: None,
        },
        None => Capability {
            value: None,
            unavailable_reason: Some(reason.to_owned()),
        },
    }
}

pub fn get_performance_details() -> PerformanceDetails {
    let adapters = list_gpu_adapters();
    let power = nvidia_power_state();
    let nvidia = query_nvidia();

    let best = adapters.first();
    let gpu = match best {
        None => capability(None, "Nenhuma GPU encontrada em /sys/class/drm"),
        Some(adapter) => {
            let name = nvidia
                .iter()
                .find(|gpu| gpu.slot == adapter.slot)
                .map(|gpu| gpu.name.clone())
                .or_else(|| cached_nvidia(&adapter.slot).map(|(name, _)| name))
                .or_else(|| lspci_name(&adapter.slot));
            capability(
                name,
                "O nome da GPU não foi exposto pelo driver nem pelo lspci",
            )
        }
    };

    let vram_bytes = match best {
        None => capability(None, "Nenhuma GPU encontrada em /sys/class/drm"),
        Some(adapter) if adapter.driver == "nvidia" => {
            let mib = nvidia
                .iter()
                .find(|gpu| gpu.slot == adapter.slot)
                .and_then(|gpu| gpu.memory_total_mib)
                .or_else(|| cached_nvidia(&adapter.slot).and_then(|(_, mib)| mib));
            let reason = if power == NvidiaPower::Suspended {
                "GPU dedicada em economia de energia; a VRAM é lida quando ela estiver ativa"
            } else {
                "nvidia-smi não informou a VRAM da GPU"
            };
            capability(mib.map(|mib| mib * MIB), reason)
        }
        Some(adapter) if adapter.driver == "amdgpu" => capability(
            read_trimmed(&adapter.device_dir.join("mem_info_vram_total"))
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0),
            "O driver amdgpu não expôs mem_info_vram_total",
        ),
        Some(adapter) if !adapter.discrete => capability(
            None,
            "GPU integrada usa a memória do sistema; não há VRAM dedicada",
        ),
        Some(_) => capability(None, "O driver desta GPU não expõe a VRAM no sysfs"),
    };

    let mut ranked: Vec<(u8, ThermalSensor)> = nvidia
        .iter()
        .filter_map(|gpu| {
            gpu.temperature.map(|celsius| {
                (
                    1,
                    ThermalSensor {
                        id: format!("gpu-{}", gpu.index),
                        label: gpu.name.clone(),
                        celsius,
                        source: "nvidia-smi".into(),
                    },
                )
            })
        })
        .collect();
    ranked.extend(hwmon_sensors(&read_hwmon()));
    ranked.sort_by_key(|(priority, _)| *priority);
    let mut thermal_sensors: Vec<ThermalSensor> =
        ranked.into_iter().map(|(_, sensor)| sensor).collect();
    thermal_sensors.truncate(MAX_SENSORS);

    let temperature_celsius = if thermal_sensors.is_empty() {
        capability(
            None,
            "Nenhum sensor térmico respondeu (sem /sys/class/hwmon legível e sem nvidia-smi)",
        )
    } else {
        hottest_sensor(&thermal_sensors)
    };

    PerformanceDetails {
        gpu,
        vram_bytes,
        temperature_celsius,
        thermal_sensors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(
        chip: &str,
        hwmon: &str,
        input: &str,
        label: Option<&str>,
        milli: i64,
    ) -> HwmonReading {
        HwmonReading {
            chip: chip.into(),
            hwmon: hwmon.into(),
            input: input.into(),
            label: label.map(str::to_owned),
            millidegrees: milli,
            device_model: None,
        }
    }

    #[test]
    fn interpreta_consulta_nvidia() {
        let gpus = parse_nvidia_query(
            "0, 00000000:01:00.0, NVIDIA GeForce RTX 3050 6GB Laptop GPU, 6144, 54\n1, 00000000:02:00.0, Tesla, [N/A], [Not Supported]\n",
        );
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].slot, "0000:01:00.0");
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 3050 6GB Laptop GPU");
        assert_eq!(gpus[0].memory_total_mib, Some(6144));
        assert_eq!(gpus[0].temperature, Some(54.0));
        assert_eq!(gpus[1].memory_total_mib, None);
        assert_eq!(gpus[1].temperature, None);
    }

    #[test]
    fn monta_nome_pelo_lspci() {
        let intel = "Slot:\t00:02.0\nClass:\tVGA compatible controller\nVendor:\tIntel Corporation\nDevice:\tRaptor Lake-S UHD Graphics\nSVendor:\tDell\n";
        assert_eq!(
            parse_lspci_name(intel).as_deref(),
            Some("Intel Raptor Lake-S UHD Graphics")
        );
        let nvidia = "Vendor:\tNVIDIA Corporation\nDevice:\tGA107M [GeForce RTX 3050 Mobile]\n";
        assert_eq!(
            parse_lspci_name(nvidia).as_deref(),
            Some("NVIDIA GeForce RTX 3050 Mobile")
        );
        let amd =
            "Vendor:\tAdvanced Micro Devices, Inc. [AMD/ATI]\nDevice:\tNavi 23 [Radeon RX 6600]\n";
        assert_eq!(parse_lspci_name(amd).as_deref(), Some("AMD Radeon RX 6600"));
        assert_eq!(parse_lspci_name("Vendor:\tX\n"), None);
    }

    #[test]
    fn rotula_e_prioriza_sensores_hwmon() {
        let mut nvme = reading("nvme", "hwmon1", "temp1", Some("Composite"), 38850);
        nvme.device_model = Some("KINGSTON SNV3S1000G".into());
        let readings = vec![
            reading("coretemp", "hwmon10", "temp2", Some("Core 0"), 75000),
            reading("coretemp", "hwmon10", "temp1", Some("Package id 0"), 72000),
            nvme,
            reading("dell_smm", "hwmon5", "temp1", None, 66000),
            reading("dell_ddv", "hwmon6", "temp2", Some("Unknown"), 55000),
            reading("alienware_wmi", "hwmon7", "temp2", Some("GPU"), 54000),
            reading("spd5118", "hwmon8", "temp1", None, 56000),
            reading("spd5118", "hwmon9", "temp1", None, 55000),
            reading("iwlwifi_1", "hwmon11", "temp1", None, 47000),
            reading("acpitz", "hwmon0", "temp1", None, -273000),
        ];
        let mut sensors = hwmon_sensors(&readings);
        sensors.sort_by_key(|(priority, _)| *priority);
        let labels: Vec<&str> = sensors.iter().map(|(_, s)| s.label.as_str()).collect();
        assert_eq!(labels[0], "CPU Package");
        assert_eq!(labels[1], "SSD KINGSTON SNV3S1000G");
        assert!(labels.contains(&"GPU (alienware_wmi)"));
        assert!(labels.contains(&"Memória RAM · módulo 1"));
        assert!(labels.contains(&"Memória RAM · módulo 2"));
        assert!(labels.contains(&"Wi-Fi"));
        assert_eq!(
            &labels[labels.len() - 2..],
            &["Sensor 1 (dell_smm)", "Sensor 2 (dell_ddv)"]
        );
        // Leitura implausível some em vez de virar 0 °C.
        assert!(!labels.iter().any(|l| l.contains("ACPI")));
        assert_eq!(sensors[0].1.celsius, 72.0);
        assert_eq!(sensors[0].1.id, "hwmon-coretemp-hwmon10-temp1");
    }

    #[test]
    fn normaliza_slot_pci() {
        assert_eq!(normalize_pci_slot("00000000:01:00.0"), "0000:01:00.0");
        assert_eq!(normalize_pci_slot("0000:01:00.0"), "0000:01:00.0");
    }

    #[test]
    fn coleta_real_tem_forma_sana() {
        let details = get_performance_details();
        assert!(details.gpu.value.is_some() || details.gpu.unavailable_reason.is_some());
        assert!(
            details.vram_bytes.value.is_some() || details.vram_bytes.unavailable_reason.is_some()
        );
        assert!(details.thermal_sensors.len() <= MAX_SENSORS);
        for sensor in &details.thermal_sensors {
            assert!((0.0..=150.0).contains(&sensor.celsius));
        }
        if let Some(hottest) = details.temperature_celsius.value {
            assert!(details.thermal_sensors.iter().any(|s| s.celsius == hottest));
        }
        // Nesta máquina (Intel + NVIDIA ativa) a dedicada é a escolhida.
        eprintln!(
            "gpu={:?} vram={:?} sensores={:?}",
            details.gpu.value,
            details.vram_bytes.value,
            details
                .thermal_sensors
                .iter()
                .map(|s| (&s.label, s.celsius))
                .collect::<Vec<_>>()
        );
    }
}
