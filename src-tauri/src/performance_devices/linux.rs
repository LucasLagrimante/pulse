//! Rede e discos físicos do módulo Desempenho no Linux.
//!
//! - Rede: contadores de `/proc/net/dev` (delta entre coletas = taxa por
//!   segundo) e metadados de `/sys/class/net/<if>` (estado, velocidade,
//!   driver, tipo). Loopback e túneis ficam de fora, como no Windows.
//! - Discos: `/sys/block` sem dispositivos virtuais (loop, zram, dm, md…);
//!   tipo pela flag `rotational`, barramento pelo caminho do dispositivo e
//!   temperatura pelo hwmon do próprio disco (NVMe nativo; SATA com o
//!   módulo `drivetemp`). Saúde SMART exige root e fica indisponível.

use super::{
    capability_number, measured, temperature_capability, unavailable_capability, NetworkAdapter,
    PhysicalDiskInfo, MAX_ADAPTERS,
};
use crate::performance::Capability;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// Contadores de bytes de uma interface em `/proc/net/dev`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceCounters {
    pub received: u64,
    pub sent: u64,
}

/// Interpreta `/proc/net/dev` (2 linhas de cabeçalho; `iface: rx… tx…`).
pub fn parse_proc_net_dev(text: &str) -> Vec<(String, InterfaceCounters)> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || name.contains('|') {
                return None;
            }
            let fields: Vec<u64> = rest
                .split_whitespace()
                .map(|field| field.parse::<u64>().ok())
                .collect::<Option<Vec<_>>>()?;
            if fields.len() < 16 {
                return None;
            }
            Some((
                name.to_owned(),
                InterfaceCounters {
                    received: fields[0],
                    sent: fields[8],
                },
            ))
        })
        .collect()
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn link_name(path: &Path) -> Option<String> {
    std::fs::read_link(path).ok().and_then(|link| {
        link.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })
}

/// `ARPHRD_*` que não são interfaces de dados do usuário: loopback e túneis
/// (IPIP, SIT, GRE, IP6 túnel). WireGuard/tun usam `NONE` (65534).
const SKIPPED_TYPES: [u32; 7] = [772, 768, 769, 776, 778, 823, 65534];

struct Sample {
    counters: InterfaceCounters,
    at: Instant,
}

static SAMPLES: Mutex<Option<HashMap<String, Sample>>> = Mutex::new(None);

fn describe_interface(dir: &Path, hardware: bool) -> String {
    let uevent = std::fs::read_to_string(dir.join("uevent")).unwrap_or_default();
    let devtype = uevent
        .lines()
        .find_map(|line| line.strip_prefix("DEVTYPE="))
        .unwrap_or("");
    let kind = if dir.join("wireless").exists() || devtype == "wlan" {
        "Wi-Fi"
    } else {
        match devtype {
            "bridge" => "Bridge virtual",
            "vlan" => "VLAN",
            "wireguard" => "WireGuard",
            "bond" => "Agregação de links",
            _ if hardware => "Ethernet",
            _ if dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("veth")) =>
            {
                "Par virtual (contêiner)"
            }
            _ => "Interface virtual",
        }
    };
    match link_name(&dir.join("device/driver")) {
        Some(driver) if hardware => format!("{kind} · {driver}"),
        _ => kind.to_owned(),
    }
}

pub fn collect_network_adapters() -> Capability<Vec<NetworkAdapter>> {
    let Ok(text) = std::fs::read_to_string("/proc/net/dev") else {
        return unavailable_capability("O kernel não expôs /proc/net/dev");
    };
    let now = Instant::now();
    let mut current: HashMap<String, Sample> = HashMap::new();
    let mut collected: Vec<NetworkAdapter> = Vec::new();

    for (name, counters) in parse_proc_net_dev(&text) {
        let dir = PathBuf::from("/sys/class/net").join(&name);
        let kind = read_trimmed(&dir.join("type")).and_then(|v| v.parse::<u32>().ok());
        if kind.is_none_or(|kind| SKIPPED_TYPES.contains(&kind)) {
            continue;
        }
        let hardware = dir.join("device").exists();
        let operstate = read_trimmed(&dir.join("operstate")).unwrap_or_default();
        // `speed` é Mb/s; -1 ou ausente (Wi-Fi) = não reportado.
        let speed = read_trimmed(&dir.join("speed"))
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .map(|mbps| mbps as u64 * 1_000_000);
        current.insert(name.clone(), Sample { counters, at: now });
        collected.push(NetworkAdapter {
            description: describe_interface(&dir, hardware),
            name,
            status: if operstate == "up" {
                "Ativo"
            } else {
                "Inativo"
            }
            .to_owned(),
            hardware_interface: hardware,
            link_speed_bps: capability_number(
                speed,
                "Velocidade de link não reportada pela interface",
            ),
            received_bps: unavailable_capability("Aguardando a primeira janela de medição"),
            sent_bps: unavailable_capability("Aguardando a primeira janela de medição"),
        });
    }

    if let Ok(mut guard) = SAMPLES.lock() {
        if let Some(previous) = guard.as_ref() {
            for adapter in collected.iter_mut() {
                let (Some(sample), Some(old)) =
                    (current.get(&adapter.name), previous.get(&adapter.name))
                else {
                    continue;
                };
                let elapsed = now.duration_since(old.at).as_secs_f64();
                if elapsed <= 0.0 {
                    continue;
                }
                // Contador que voltou (interface recriada) não vira taxa.
                if sample.counters.received < old.counters.received
                    || sample.counters.sent < old.counters.sent
                {
                    continue;
                }
                adapter.received_bps =
                    measured((sample.counters.received - old.counters.received) as f64 / elapsed);
                adapter.sent_bps =
                    measured((sample.counters.sent - old.counters.sent) as f64 / elapsed);
            }
        }
        *guard = Some(current);
    }

    sort_adapters(&mut collected);
    collected.truncate(MAX_ADAPTERS);
    Capability {
        value: Some(collected),
        unavailable_reason: None,
    }
}

/// Mesma ordem do Windows: ativas, físicas, mais tráfego, nome.
fn sort_adapters(adapters: &mut [NetworkAdapter]) {
    let traffic = |adapter: &NetworkAdapter| {
        adapter.received_bps.value.unwrap_or(0.0) + adapter.sent_bps.value.unwrap_or(0.0)
    };
    adapters.sort_by(|a, b| {
        let active = |adapter: &NetworkAdapter| adapter.status == "Ativo";
        active(b)
            .cmp(&active(a))
            .then_with(|| b.hardware_interface.cmp(&a.hardware_interface))
            .then_with(|| {
                traffic(b)
                    .partial_cmp(&traffic(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Dispositivos de bloco que não são discos físicos.
pub fn is_virtual_block(name: &str) -> bool {
    [
        "loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd", "drbd",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// Barramento a partir do nome e do caminho canônico do dispositivo.
pub fn bus_type(name: &str, device_path: &str) -> String {
    let path = device_path.to_lowercase();
    if name.starts_with("nvme") {
        "NVMe"
    } else if path.contains("/usb") {
        "USB"
    } else if name.starts_with("mmcblk") {
        "SD/MMC"
    } else if path.contains("/virtio") || name.starts_with("vd") {
        "Virtio"
    } else if path.contains("/ata") {
        "SATA"
    } else if path.contains("/host") {
        "SCSI"
    } else {
        "Não especificado"
    }
    .to_owned()
}

fn disk_temperature(device_dir: &Path) -> Option<f64> {
    // NVMe: `device/hwmonN`; SATA com drivetemp: `device/hwmon/hwmonN`.
    let mut roots = vec![device_dir.to_path_buf(), device_dir.join("hwmon")];
    roots.retain(|root| root.is_dir());
    roots.iter().find_map(|root| {
        std::fs::read_dir(root).ok()?.flatten().find_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("hwmon") || name == "hwmon" {
                return None;
            }
            read_trimmed(&entry.path().join("temp1_input"))
                .and_then(|v| v.parse::<i64>().ok())
                .map(|milli| milli as f64 / 1000.0)
        })
    })
}

pub fn collect_physical_disks() -> Capability<Vec<PhysicalDiskInfo>> {
    let Ok(entries) = std::fs::read_dir("/sys/block") else {
        return unavailable_capability("O kernel não expôs /sys/block");
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !is_virtual_block(name))
        .collect();
    names.sort();

    let disks = names
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let dir = PathBuf::from("/sys/block").join(&name);
            let device_dir = dir.join("device");
            let canonical = std::fs::canonicalize(&device_dir)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let rotational = read_trimmed(&dir.join("queue/rotational"));
            let media_type = if name.starts_with("mmcblk") {
                "Flash (SD/eMMC)".to_owned()
            } else {
                match rotational.as_deref() {
                    Some("0") => "SSD".to_owned(),
                    Some("1") => "HDD".to_owned(),
                    _ => "Não especificado".to_owned(),
                }
            };
            let size = read_trimmed(&dir.join("size"))
                .and_then(|v| v.parse::<u64>().ok())
                .map(|sectors| sectors.saturating_mul(512))
                .filter(|bytes| *bytes > 0);
            PhysicalDiskInfo {
                index: index as u32,
                model: read_trimmed(&device_dir.join("model"))
                    .or_else(|| read_trimmed(&device_dir.join("name")))
                    .unwrap_or_else(|| "Modelo não reportado".to_owned()),
                media_type,
                bus_type: bus_type(&name, &canonical),
                size_bytes: capability_number(size, "Capacidade não reportada pelo disco"),
                health: unavailable_capability(
                    "Saúde SMART exige privilégio de administrador (smartctl)",
                ),
                temperature_celsius: temperature_capability(disk_temperature(&device_dir)),
                wear_percent: None,
                power_on_hours: None,
            }
        })
        .collect();

    Capability {
        value: Some(disks),
        unavailable_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_DEV: &str = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 1624021978 1124379    0    0    0     0          0         0 1624021978 1124379    0    0    0     0       0          0\nenp4s0: 9839570789 9356427    0    0    0     0          0     72331 3305842251 5882052    0    0    0     0       0          0\n";

    #[test]
    fn interpreta_proc_net_dev() {
        let parsed = parse_proc_net_dev(NET_DEV);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].0, "enp4s0");
        assert_eq!(
            parsed[1].1,
            InterfaceCounters {
                received: 9_839_570_789,
                sent: 3_305_842_251
            }
        );
    }

    #[test]
    fn classifica_blocos_e_barramentos() {
        assert!(is_virtual_block("loop3"));
        assert!(is_virtual_block("zram0"));
        assert!(is_virtual_block("dm-1"));
        assert!(!is_virtual_block("nvme0n1"));
        assert!(!is_virtual_block("sda"));
        assert_eq!(
            bus_type("nvme0n1", "/sys/devices/pci0000:00/nvme/nvme0"),
            "NVMe"
        );
        assert_eq!(
            bus_type("sdb", "/sys/devices/pci0000:00/usb2/2-1/host6/target6"),
            "USB"
        );
        assert_eq!(
            bus_type("sda", "/sys/devices/pci0000:00/ata1/host0/target0:0:0"),
            "SATA"
        );
    }

    #[test]
    fn coleta_real_tem_forma_sana() {
        let adapters = collect_network_adapters().value.expect("rede");
        assert!(adapters.len() <= MAX_ADAPTERS);
        assert!(adapters.iter().all(|a| a.name != "lo"));
        std::thread::sleep(std::time::Duration::from_millis(300));
        let second = collect_network_adapters().value.expect("rede");
        for adapter in &second {
            if let Some(rate) = adapter.received_bps.value {
                assert!(rate >= 0.0);
            }
        }
        let disks = collect_physical_disks().value.expect("discos");
        for disk in &disks {
            assert!(
                disk.size_bytes.value.is_some() || disk.size_bytes.unavailable_reason.is_some()
            );
            assert!(disk.health.unavailable_reason.is_some());
        }
        eprintln!(
            "rede={:?} discos={:?}",
            second
                .iter()
                .map(|a| (&a.name, &a.description, &a.status))
                .collect::<Vec<_>>(),
            disks
                .iter()
                .map(|d| (
                    &d.model,
                    &d.bus_type,
                    &d.media_type,
                    d.temperature_celsius.value
                ))
                .collect::<Vec<_>>()
        );
    }
}
