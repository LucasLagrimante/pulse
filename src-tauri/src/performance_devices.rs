//! Inventário de rede e armazenamento físico do módulo Desempenho.
//!
//! - Rede: `GetIfTable2` lê os contadores de octetos por interface sem
//!   subprocessos e sem PDH (cujos nomes de contador são localizados no
//!   Windows em português). As taxas por segundo derivam do delta entre
//!   coletas consecutivas.
//! - Discos físicos: inventário via `Get-PhysicalDisk` em subprocesso oculto
//!   com cache de 60 s — modelo, tipo de mídia, barramento e saúde não mudam
//!   entre os ciclos de telemetria de 5 s.

use crate::performance::Capability;
use serde::Serialize;
use std::time::{Duration, Instant};

const DISK_CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_ADAPTERS: usize = 6;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAdapter {
    pub name: String,
    pub description: String,
    pub status: String,
    pub hardware_interface: bool,
    pub link_speed_bps: Capability<u64>,
    pub received_bps: Capability<f64>,
    pub sent_bps: Capability<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalDiskInfo {
    pub index: u32,
    pub model: String,
    pub media_type: String,
    pub bus_type: String,
    pub size_bytes: Capability<u64>,
    pub health: Capability<String>,
    pub temperature_celsius: Capability<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceDevices {
    pub adapters: Capability<Vec<NetworkAdapter>>,
    pub disks: Capability<Vec<PhysicalDiskInfo>>,
    pub volumes: Vec<crate::disk_analyzer::DriveInfo>,
}

pub fn get_performance_devices_sync() -> PerformanceDevices {
    PerformanceDevices {
        adapters: collect_network_adapters(),
        disks: collect_physical_disks(),
        volumes: crate::disk_analyzer::get_system_drives(),
    }
}

#[tauri::command]
pub async fn get_performance_devices() -> PerformanceDevices {
    let started = Instant::now();
    let devices = tauri::async_runtime::spawn_blocking(get_performance_devices_sync)
        .await
        .unwrap_or_else(|_| PerformanceDevices {
            adapters: unavailable_capability("Falha na execução em segundo plano"),
            disks: unavailable_capability("Falha na execução em segundo plano"),
            volumes: Vec::new(),
        });
    crate::debug::record("performance_devices", started.elapsed().as_millis(), Ok(()));
    devices
}

fn unavailable_capability<T>(reason: &str) -> Capability<T> {
    Capability {
        value: None,
        unavailable_reason: Some(reason.to_owned()),
    }
}

/// Converte um valor medido em `Capability`, arredondado a uma casa decimal.
/// Valores negativos ou não finitos viram indisponibilidade explícita.
fn measured(value: f64) -> Capability<f64> {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.is_finite() && rounded >= 0.0 {
        Capability {
            value: Some(rounded),
            unavailable_reason: None,
        }
    } else {
        unavailable_capability("Medição inválida reportada pela interface")
    }
}

fn capability_number<T>(value: Option<T>, reason: &str) -> Capability<T> {
    match value {
        Some(value) => Capability {
            value: Some(value),
            unavailable_reason: None,
        },
        None => unavailable_capability(reason),
    }
}

fn normalize_enumerated(raw: Option<&str>) -> String {
    match raw.map(str::trim) {
        None | Some("") | Some("Unspecified") | Some("Unknown") => "Não especificado".to_owned(),
        Some(value) => value.to_owned(),
    }
}

fn health_capability(raw: Option<&str>) -> Capability<String> {
    match raw.map(str::trim) {
        Some("Healthy") => Capability {
            value: Some("Saudável".to_owned()),
            unavailable_reason: None,
        },
        Some("Warning") => Capability {
            value: Some("Atenção".to_owned()),
            unavailable_reason: None,
        },
        Some("Unhealthy") => Capability {
            value: Some("Falha reportada".to_owned()),
            unavailable_reason: None,
        },
        _ => unavailable_capability("Saúde não reportada pelo Windows"),
    }
}

fn temperature_capability(value: Option<f64>) -> Capability<f64> {
    match value.filter(|v| v.is_finite() && (0.0..=150.0).contains(v)) {
        Some(value) => Capability {
            value: Some((value * 10.0).round() / 10.0),
            unavailable_reason: None,
        },
        None => unavailable_capability("Sensor de temperatura não exposto pelo disco"),
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{
        capability_number, health_capability, measured, normalize_enumerated,
        temperature_capability, unavailable_capability, NetworkAdapter, PhysicalDiskInfo,
        DISK_CACHE_TTL, MAX_ADAPTERS,
    };
    use crate::performance::Capability;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;

    pub fn collect_network_adapters() -> Capability<Vec<NetworkAdapter>> {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::NetworkManagement::IpHelper::{
            FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL, MIB_IF_ROW2,
            MIB_IF_TABLE2,
        };
        use windows::Win32::NetworkManagement::Ndis::{IfOperStatusUp, IF_OPER_STATUS};

        struct OctetSample {
            in_octets: u64,
            out_octets: u64,
            at: Instant,
        }

        // Amostras por interface mantidas entre coletas para o delta de octetos.
        static OCTET_SAMPLES: Mutex<Option<HashMap<u32, OctetSample>>> = Mutex::new(None);

        fn wide_text(value: &[u16]) -> String {
            let len = value.iter().position(|&c| c == 0).unwrap_or(value.len());
            String::from_utf16_lossy(&value[..len]).trim().to_string()
        }

        fn status_label(status: IF_OPER_STATUS) -> &'static str {
            if status == IfOperStatusUp {
                "Ativo"
            } else {
                "Inativo"
            }
        }

        fn link_speed(receive: u64, transmit: u64) -> Capability<u64> {
            capability_number(
                Some(receive.max(transmit)).filter(|v| *v > 0),
                "Velocidade de link não reportada pela interface",
            )
        }

        fn traffic(adapter: &NetworkAdapter) -> f64 {
            adapter.received_bps.value.unwrap_or(0.0) + adapter.sent_bps.value.unwrap_or(0.0)
        }

        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if unsafe { GetIfTable2(&mut table) } != ERROR_SUCCESS || table.is_null() {
            return unavailable_capability(
                "A pilha de rede do Windows não expôs as interfaces do sistema",
            );
        }

        let now = Instant::now();
        let mut current: HashMap<u32, OctetSample> = HashMap::new();
        let mut collected: Vec<(u32, NetworkAdapter)> = Vec::new();

        // A tabela retornada garante `NumEntries` linhas contíguas válidas.
        unsafe {
            let entries = (*table).NumEntries as usize;
            let rows = std::slice::from_raw_parts(
                std::ptr::addr_of!((*table).Table).cast::<MIB_IF_ROW2>(),
                entries,
            );
            for row in rows {
                if row.Type == IF_TYPE_SOFTWARE_LOOPBACK || row.Type == IF_TYPE_TUNNEL {
                    continue;
                }
                let alias = wide_text(&row.Alias);
                let description = wide_text(&row.Description);
                if alias.is_empty() && description.is_empty() {
                    continue;
                }
                let index = row.InterfaceIndex;
                current.insert(
                    index,
                    OctetSample {
                        in_octets: row.InOctets,
                        out_octets: row.OutOctets,
                        at: now,
                    },
                );
                collected.push((
                    index,
                    NetworkAdapter {
                        name: if alias.is_empty() {
                            description.clone()
                        } else {
                            alias
                        },
                        description,
                        status: status_label(row.OperStatus).to_owned(),
                        hardware_interface: row.InterfaceAndOperStatusFlags._bitfield & 0x01 != 0,
                        link_speed_bps: link_speed(row.ReceiveLinkSpeed, row.TransmitLinkSpeed),
                        received_bps: unavailable_capability(
                            "Aguardando a primeira janela de medição",
                        ),
                        sent_bps: unavailable_capability("Aguardando a primeira janela de medição"),
                    },
                ));
            }
            FreeMibTable(table as *const core::ffi::c_void);
        }

        if let Ok(mut guard) = OCTET_SAMPLES.lock() {
            if let Some(previous) = guard.as_ref() {
                for (index, adapter) in collected.iter_mut() {
                    let (Some(sample), Some(old)) = (current.get(index), previous.get(index))
                    else {
                        continue;
                    };
                    let elapsed = now.duration_since(old.at).as_secs_f64();
                    if elapsed <= 0.0 {
                        continue;
                    }
                    adapter.received_bps =
                        measured(sample.in_octets.saturating_sub(old.in_octets) as f64 / elapsed);
                    adapter.sent_bps =
                        measured(sample.out_octets.saturating_sub(old.out_octets) as f64 / elapsed);
                }
            }
            // O mapa atual substitui o anterior e poda interfaces removidas.
            *guard = Some(current);
        }

        collected.sort_by(|a, b| {
            let active = |adapter: &NetworkAdapter| adapter.status == "Ativo";
            active(&b.1)
                .cmp(&active(&a.1))
                .then_with(|| b.1.hardware_interface.cmp(&a.1.hardware_interface))
                .then_with(|| {
                    traffic(&b.1)
                        .partial_cmp(&traffic(&a.1))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.1.name.cmp(&b.1.name))
        });
        collected.truncate(MAX_ADAPTERS);

        Capability {
            value: Some(collected.into_iter().map(|(_, adapter)| adapter).collect()),
            unavailable_reason: None,
        }
    }

    pub fn collect_physical_disks() -> Capability<Vec<PhysicalDiskInfo>> {
        static CACHE: Mutex<Option<(Instant, Capability<Vec<PhysicalDiskInfo>>)>> =
            Mutex::new(None);

        if let Ok(guard) = CACHE.lock() {
            if let Some((collected_at, cached)) = guard.as_ref() {
                if collected_at.elapsed() < DISK_CACHE_TTL {
                    return cached.clone();
                }
            }
        }

        let collected = query_physical_disks();
        if let Ok(mut guard) = CACHE.lock() {
            *guard = Some((Instant::now(), collected.clone()));
        }
        collected
    }

    fn query_physical_disks() -> Capability<Vec<PhysicalDiskInfo>> {
        let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$temperatures = @{}
foreach ($counter in @(Get-StorageReliabilityCounter)) {
  if ($null -ne $counter.Temperature -and [int]$counter.Temperature -ge 0) {
    $temperatures[[int]$counter.DeviceId] = [double]$counter.Temperature
  }
}
$disks = @()
foreach ($disk in @(Get-PhysicalDisk)) {
  $index = [int]$disk.DeviceId
  $disks += [PSCustomObject]@{
    index = $index
    model = [string]$disk.FriendlyName
    mediaType = [string]$disk.MediaType
    busType = [string]$disk.BusType
    sizeBytes = [uint64]$disk.Size
    health = [string]$disk.HealthStatus
    temperatureCelsius = $temperatures[$index]
  }
}
ConvertTo-Json -InputObject @($disks) -Compress
"#;

        let result = crate::process::hidden_command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output();
        let Ok(output) = result else {
            return unavailable_capability("PowerShell não está disponível");
        };
        if !output.status.success() {
            return unavailable_capability("O Windows não retornou o inventário de discos físicos");
        }
        let Ok(text) = std::str::from_utf8(&output.stdout) else {
            return unavailable_capability("A resposta de armazenamento não é texto válido");
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Capability {
                value: Some(Vec::new()),
                unavailable_reason: None,
            };
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            return unavailable_capability("O inventário de discos não pôde ser interpretado");
        };
        // `ConvertTo-Json` serializa um array de um item como objeto único.
        let items: Vec<&serde_json::Value> = match &value {
            serde_json::Value::Array(items) => items.iter().collect(),
            serde_json::Value::Object(_) => vec![&value],
            _ => Vec::new(),
        };

        let disks = items
            .into_iter()
            .map(|item| PhysicalDiskInfo {
                index: item.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                model: item
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .unwrap_or("Modelo não reportado")
                    .to_owned(),
                media_type: normalize_enumerated(item.get("mediaType").and_then(|v| v.as_str())),
                bus_type: normalize_enumerated(item.get("busType").and_then(|v| v.as_str())),
                size_bytes: capability_number(
                    item.get("sizeBytes")
                        .and_then(|v| v.as_u64())
                        .filter(|v| *v > 0),
                    "Capacidade não reportada pelo disco",
                ),
                health: health_capability(item.get("health").and_then(|v| v.as_str())),
                temperature_celsius: temperature_capability(
                    item.get("temperatureCelsius").and_then(|v| v.as_f64()),
                ),
            })
            .collect();

        Capability {
            value: Some(disks),
            unavailable_reason: None,
        }
    }
}

#[cfg(windows)]
use windows_impl::{collect_network_adapters, collect_physical_disks};

#[cfg(not(windows))]
fn collect_network_adapters() -> Capability<Vec<NetworkAdapter>> {
    unavailable_capability("Telemetria de rede disponível apenas no Windows")
}

#[cfg(not(windows))]
fn collect_physical_disks() -> Capability<Vec<PhysicalDiskInfo>> {
    unavailable_capability("Inventário de discos disponível apenas no Windows")
}

#[cfg(test)]
mod tests {
    use super::{health_capability, normalize_enumerated, temperature_capability};

    #[test]
    fn normalizes_enumerated_types() {
        assert_eq!(normalize_enumerated(Some("SSD")), "SSD");
        assert_eq!(
            normalize_enumerated(Some("Unspecified")),
            "Não especificado"
        );
        assert_eq!(normalize_enumerated(None), "Não especificado");
    }

    #[test]
    fn maps_health_status_to_labels() {
        assert_eq!(
            health_capability(Some("Healthy")).value.as_deref(),
            Some("Saudável")
        );
        assert!(health_capability(None).value.is_none());
    }

    #[test]
    fn rejects_out_of_range_temperatures() {
        assert!(temperature_capability(Some(42.5)).value.is_some());
        assert!(temperature_capability(Some(-1.0)).value.is_none());
        assert!(temperature_capability(Some(200.0)).value.is_none());
    }
}
