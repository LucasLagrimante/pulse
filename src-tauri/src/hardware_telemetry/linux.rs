//! Coleta nativa de telemetria expandida no Linux (`/proc`, `/sys/class/hwmon`,
//! `/sys/class/drm` e `nvidia-smi` quando acordada).

use super::{
    build_metric_catalog, infer_group_hint, now_unix_ms, parse_nvidia_extended_csv, round1, round2,
    sanitize_id, CpuHardwareInfo, DiskIoStats, ExternalSensorReading, GpuHardwareInfo,
    HardwareTelemetrySnapshot, RamHardwareInfo, SensorCategory,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

const KIB_TO_GIB: f64 = 1024.0 * 1024.0;
const MIB: f64 = 1024.0 * 1024.0;

#[derive(Clone, Copy)]
struct CpuTimes {
    busy: u64,
    total: u64,
}

static PREV_CORE_TIMES: Mutex<Option<Vec<CpuTimes>>> = Mutex::new(None);

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn read_per_core_percent() -> Vec<f64> {
    let Ok(text) = fs::read_to_string("/proc/stat") else {
        return Vec::new();
    };
    let mut current = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("cpu") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let Some(idx_str) = parts.next() else {
            continue;
        };
        if idx_str.is_empty() || !idx_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let nums: Vec<u64> = parts.filter_map(|p| p.parse::<u64>().ok()).collect();
        if nums.len() < 4 {
            continue;
        }
        let idle = nums[3].saturating_add(*nums.get(4).unwrap_or(&0));
        let total: u64 = nums.iter().copied().sum();
        let busy = total.saturating_sub(idle);
        current.push(CpuTimes { busy, total });
    }

    let mut out = Vec::new();
    if let Ok(mut guard) = PREV_CORE_TIMES.lock() {
        if let Some(prev) = guard.as_ref() {
            if prev.len() == current.len() {
                for (old, now) in prev.iter().zip(current.iter()) {
                    let dt = now.total.saturating_sub(old.total);
                    let db = now.busy.saturating_sub(old.busy);
                    if dt > 0 {
                        out.push(round1(((db as f64 / dt as f64) * 100.0).clamp(0.0, 100.0)));
                    }
                }
            }
        }
        *guard = Some(current);
    }
    out
}

fn read_cpu_info() -> CpuHardwareInfo {
    let logical_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);

    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpuinfo.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if k.trim() == "model name" {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
        None
    });

    let physical_cores = {
        let mut pairs = HashSet::new();
        let mut cur_phys = None::<String>;
        let mut cur_core = None::<String>;
        for line in cpuinfo.lines() {
            if line.trim().is_empty() {
                if let (Some(p), Some(c)) = (cur_phys.take(), cur_core.take()) {
                    pairs.insert((p, c));
                }
            } else if let Some((k, v)) = line.split_once(':') {
                match k.trim() {
                    "physical id" => cur_phys = Some(v.trim().to_owned()),
                    "core id" => cur_core = Some(v.trim().to_owned()),
                    _ => {}
                }
            }
        }
        if let (Some(p), Some(c)) = (cur_phys, cur_core) {
            pairs.insert((p, c));
        }
        if pairs.is_empty() {
            None
        } else {
            Some(pairs.len() as u32)
        }
    };

    // Frequência real média via sysfs cpufreq (fallback /proc/cpuinfo cpu MHz)
    let mut freqs_mhz = Vec::new();
    for i in 0..logical_cores {
        let path = format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq");
        if let Some(khz) = read_trimmed(Path::new(&path)).and_then(|s| s.parse::<f64>().ok()) {
            if khz > 0.0 {
                freqs_mhz.push(khz / 1000.0);
            }
        }
    }
    if freqs_mhz.is_empty() {
        for line in cpuinfo.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim() == "cpu MHz" {
                    if let Ok(mhz) = v.trim().parse::<f64>() {
                        if mhz > 0.0 {
                            freqs_mhz.push(mhz);
                        }
                    }
                }
            }
        }
    }
    let current_clock_mhz = if freqs_mhz.is_empty() {
        None
    } else {
        Some(round1(
            freqs_mhz.iter().sum::<f64>() / freqs_mhz.len() as f64,
        ))
    };

    let base_clock_mhz = read_trimmed(Path::new(
        "/sys/devices/system/cpu/cpu0/cpufreq/base_frequency",
    ))
    .or_else(|| {
        read_trimmed(Path::new(
            "/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq",
        ))
    })
    .and_then(|s| s.parse::<u64>().ok())
    .filter(|khz| *khz > 0)
    .map(|khz| (khz / 1000) as u32);

    let uptime_seconds = read_trimmed(Path::new("/proc/uptime"))
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v as u64);

    let thread_count = read_trimmed(Path::new("/proc/loadavg")).and_then(|s| {
        s.split_whitespace()
            .nth(3)?
            .split('/')
            .nth(1)?
            .parse::<u32>()
            .ok()
    });

    CpuHardwareInfo {
        model,
        physical_cores,
        logical_cores,
        base_clock_mhz,
        current_clock_mhz,
        per_core_percent: read_per_core_percent(),
        uptime_seconds,
        process_count: None,
        thread_count,
        handle_count: None,
    }
}

fn read_ram_info() -> RamHardwareInfo {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return RamHardwareInfo::default();
    };
    let mut get_kib = |key: &str| -> Option<f64> {
        text.lines().find_map(|line| {
            let (k, rest) = line.split_once(':')?;
            if k.trim() == key {
                rest.split_whitespace().next()?.parse::<f64>().ok()
            } else {
                None
            }
        })
    };
    let total = get_kib("MemTotal").map(|k| round2(k / KIB_TO_GIB));
    let avail = get_kib("MemAvailable").map(|k| round2(k / KIB_TO_GIB));
    let used = match (total, avail) {
        (Some(t), Some(a)) => Some(round2((t - a).max(0.0))),
        _ => None,
    };
    let cached = match (get_kib("Cached"), get_kib("Buffers")) {
        (Some(c), Some(b)) => Some(round2((c + b) / KIB_TO_GIB)),
        (Some(c), None) => Some(round2(c / KIB_TO_GIB)),
        _ => None,
    };
    let commit_used = get_kib("Committed_AS").map(|k| round2(k / KIB_TO_GIB));
    let commit_limit = get_kib("CommitLimit").map(|k| round2(k / KIB_TO_GIB));

    RamHardwareInfo {
        used_gb: used,
        total_gb: total,
        available_gb: avail,
        cached_gb: cached,
        commit_used_gb: commit_used,
        commit_limit_gb: commit_limit,
        speed_mhz: None,
        slots_used: None,
    }
}

#[derive(Clone, Copy)]
struct DiskStatSample {
    read_sectors: u64,
    write_sectors: u64,
    reads_completed: u64,
    writes_completed: u64,
    io_ms: u64,
    at: Instant,
}

static PREV_DISK_STAT: Mutex<Option<DiskStatSample>> = Mutex::new(None);

fn is_whole_physical_disk(name: &str) -> bool {
    if name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("zram")
        || name.starts_with("dm-")
        || name.starts_with("md")
        || name.starts_with("sr")
    {
        return false;
    }
    PathBuf::from("/sys/block").join(name).is_dir()
}

fn read_disk_io() -> DiskIoStats {
    let Ok(text) = fs::read_to_string("/proc/diskstats") else {
        return DiskIoStats::default();
    };
    let mut read_sectors = 0u64;
    let mut write_sectors = 0u64;
    let mut reads_completed = 0u64;
    let mut writes_completed = 0u64;
    let mut io_ms = 0u64;

    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 14 {
            continue;
        }
        if !is_whole_physical_disk(cols[2]) {
            continue;
        }
        reads_completed = reads_completed.saturating_add(cols[3].parse().unwrap_or(0));
        read_sectors = read_sectors.saturating_add(cols[5].parse().unwrap_or(0));
        writes_completed = writes_completed.saturating_add(cols[7].parse().unwrap_or(0));
        write_sectors = write_sectors.saturating_add(cols[9].parse().unwrap_or(0));
        io_ms = io_ms.saturating_add(cols[12].parse().unwrap_or(0));
    }

    let now = Instant::now();
    let sample = DiskStatSample {
        read_sectors,
        write_sectors,
        reads_completed,
        writes_completed,
        io_ms,
        at: now,
    };

    let mut stats = DiskIoStats::default();
    if let Ok(mut guard) = PREV_DISK_STAT.lock() {
        if let Some(old) = *guard {
            let elapsed = now.duration_since(old.at).as_secs_f64();
            if elapsed > 0.05 {
                let d_read_bytes =
                    sample.read_sectors.saturating_sub(old.read_sectors) as f64 * 512.0;
                let d_write_bytes =
                    sample.write_sectors.saturating_sub(old.write_sectors) as f64 * 512.0;
                let d_reads = sample.reads_completed.saturating_sub(old.reads_completed) as f64;
                let d_writes = sample.writes_completed.saturating_sub(old.writes_completed) as f64;
                let d_io_ms = sample.io_ms.saturating_sub(old.io_ms) as f64;
                stats = DiskIoStats {
                    read_mbps: Some(round1((d_read_bytes / elapsed) / MIB)),
                    write_mbps: Some(round1((d_write_bytes / elapsed) / MIB)),
                    active_percent: Some(round1(
                        ((d_io_ms / (elapsed * 1000.0)) * 100.0).clamp(0.0, 100.0),
                    )),
                    queue_length: None,
                    read_iops: Some(round1(d_reads / elapsed)),
                    write_iops: Some(round1(d_writes / elapsed)),
                    per_disk: Vec::new(),
                };
            }
        }
        *guard = Some(sample);
    }
    stats
}

fn read_hwmon_sensors() -> Vec<ExternalSensorReading> {
    let Ok(entries) = fs::read_dir("/sys/class/hwmon") else {
        return Vec::new();
    };
    let mut sensors = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let chip = read_trimmed(&dir.join("name")).unwrap_or_else(|| "hwmon".into());
        let Ok(files) = fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = files
            .flatten()
            .map(|f| f.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();

        for file in names {
            if let Some(stem) = file.strip_suffix("_input") {
                let raw_val = read_trimmed(&dir.join(&file)).and_then(|v| v.parse::<f64>().ok());
                let Some(raw_val) = raw_val else {
                    continue;
                };
                let custom_label = read_trimmed(&dir.join(format!("{stem}_label")));
                let label = match custom_label {
                    Some(lbl) => format!("{chip} · {lbl}"),
                    None => format!("{chip} · {stem}"),
                };

                let (category, val, unit) = if stem.starts_with("temp") {
                    let c = raw_val / 1000.0;
                    if !(0.0..=150.0).contains(&c) {
                        continue;
                    }
                    (SensorCategory::Temperature, round1(c), "°C")
                } else if stem.starts_with("fan") {
                    if !(0.0..=20_000.0).contains(&raw_val) {
                        continue;
                    }
                    (SensorCategory::Fan, raw_val.round(), "RPM")
                } else if stem.starts_with("power") {
                    let w = raw_val / 1_000_000.0;
                    if !(0.0..=2_500.0).contains(&w) {
                        continue;
                    }
                    (SensorCategory::Power, round1(w), "W")
                } else if stem.starts_with("in") {
                    let v = raw_val / 1000.0;
                    if !(0.0..=30.0).contains(&v) {
                        continue;
                    }
                    (SensorCategory::Voltage, round2(v), "V")
                } else {
                    continue;
                };

                let hint = infer_group_hint(&chip, &label);
                sensors.push(ExternalSensorReading {
                    id: format!("hwmon-{}-{}", sanitize_id(&chip), sanitize_id(stem)),
                    label,
                    category,
                    value: val,
                    unit: unit.into(),
                    source: "hwmon".into(),
                    group_hint: hint,
                });
            } else if let Some(stem) = file.strip_suffix("_average") {
                if !stem.starts_with("power") {
                    continue;
                }
                let Some(raw_val) =
                    read_trimmed(&dir.join(&file)).and_then(|v| v.parse::<f64>().ok())
                else {
                    continue;
                };
                let w = raw_val / 1_000_000.0;
                if !(0.0..=2_500.0).contains(&w) {
                    continue;
                }
                let custom_label = read_trimmed(&dir.join(format!("{stem}_label")));
                let label = match custom_label {
                    Some(lbl) => format!("{chip} · {lbl}"),
                    None => format!("{chip} · {stem}"),
                };
                let hint = infer_group_hint(&chip, &label);
                sensors.push(ExternalSensorReading {
                    id: format!("hwmon-{}-{}", sanitize_id(&chip), sanitize_id(stem)),
                    label,
                    category: SensorCategory::Power,
                    value: round1(w),
                    unit: "W".into(),
                    source: "hwmon".into(),
                    group_hint: hint,
                });
            }
        }
    }
    sensors
}

fn read_gpu_info() -> GpuHardwareInfo {
    if crate::performance::linux::nvidia_power_state()
        == crate::performance::linux::NvidiaPower::Awake
    {
        if let Some(csv) = crate::performance::linux::run_nvidia_smi(&[
            "--query-gpu=index,name,temperature.gpu,fan.speed,clocks.gr,clocks.mem,power.draw,power.limit,memory.used,memory.total,utilization.gpu",
            "--format=csv,noheader,nounits",
        ]) {
            if let Some(sample) = parse_nvidia_extended_csv(&csv).into_iter().next() {
                return GpuHardwareInfo {
                    name: if sample.name.is_empty() { None } else { Some(sample.name) },
                    usage_percent: sample.utilization_percent,
                    vram_used_gb: sample.vram_used_mib.map(|m| round2(m / 1024.0)),
                    vram_total_gb: sample.vram_total_mib.map(|m| round2(m / 1024.0)),
                    shared_used_gb: None,
                    temperature_celsius: sample.temperature_celsius,
                    fan_percent: sample.fan_percent,
                    fan_rpm: None,
                    core_clock_mhz: sample.core_clock_mhz,
                    memory_clock_mhz: sample.memory_clock_mhz,
                    power_watts: sample.power_watts,
                    power_limit_watts: sample.power_limit_watts,
                    source: Some("nvidia-smi".into()),
                };
            }
        }
    }

    // Fallback DRM/amdgpu via sysfs
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return GpuHardwareInfo::default();
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = entry.path().join("device");
        let vram_total = read_trimmed(&dev.join("mem_info_vram_total"))
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| *v > 0.0)
            .map(|b| round2(b / (1024.0 * 1024.0 * 1024.0)));
        let vram_used = read_trimmed(&dev.join("mem_info_vram_used"))
            .and_then(|s| s.parse::<f64>().ok())
            .map(|b| round2(b / (1024.0 * 1024.0 * 1024.0)));
        let busy = read_trimmed(&dev.join("gpu_busy_percent"))
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| (0.0..=100.0).contains(v))
            .map(round1);
        if vram_total.is_some() || busy.is_some() {
            return GpuHardwareInfo {
                name: None,
                usage_percent: busy,
                vram_used_gb: vram_used,
                vram_total_gb: vram_total,
                shared_used_gb: None,
                temperature_celsius: None,
                fan_percent: None,
                fan_rpm: None,
                core_clock_mhz: None,
                memory_clock_mhz: None,
                power_watts: None,
                power_limit_watts: None,
                source: Some("sysfs/drm".into()),
            };
        }
    }
    GpuHardwareInfo::default()
}

pub fn collect_snapshot() -> HardwareTelemetrySnapshot {
    let cpu = read_cpu_info();
    let ram = read_ram_info();
    let gpu = read_gpu_info();
    let disk_io = read_disk_io();
    let hwmon = read_hwmon_sensors();

    let mut providers = Vec::new();
    if !hwmon.is_empty() {
        providers.push("hwmon".into());
    }

    let metrics = build_metric_catalog(&cpu, &ram, &gpu, &disk_io, &hwmon);

    HardwareTelemetrySnapshot {
        cpu,
        ram,
        gpu,
        disk_io,
        metrics,
        external_providers: providers,
        sampled_at_unix_ms: now_unix_ms(),
    }
}
