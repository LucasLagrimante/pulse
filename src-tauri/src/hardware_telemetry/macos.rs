//! Coleta de telemetria expandida no macOS (via `sysinfo`, sem sensores SMC privados).

use super::{
    build_metric_catalog, now_unix_ms, round1, round2, CpuHardwareInfo, DiskIoStats,
    GpuHardwareInfo, HardwareTelemetrySnapshot, RamHardwareInfo,
};
use sysinfo::System;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub fn collect_snapshot() -> HardwareTelemetrySnapshot {
    let mut sys = System::new_all();
    sys.refresh_cpu_all();
    sys.refresh_memory();

    let cpus = sys.cpus();
    let model = cpus
        .first()
        .map(|c| c.brand().trim().to_owned())
        .filter(|s| !s.is_empty());
    let current_clock_mhz = cpus
        .first()
        .map(|c| c.frequency() as f64)
        .filter(|v| *v > 0.0);
    let per_core_percent: Vec<f64> = cpus
        .iter()
        .map(|c| round1((c.cpu_usage() as f64).clamp(0.0, 100.0)))
        .collect();

    let cpu = CpuHardwareInfo {
        model,
        physical_cores: System::physical_core_count().map(|n| n as u32),
        logical_cores: cpus.len() as u32,
        base_clock_mhz: current_clock_mhz.map(|v| v as u32),
        current_clock_mhz,
        per_core_percent,
        uptime_seconds: Some(System::uptime()),
        process_count: Some(sys.processes().len() as u32),
        thread_count: None,
        handle_count: None,
    };

    let total_gb = Some(round2(sys.total_memory() as f64 / GIB));
    let used_gb = Some(round2(sys.used_memory() as f64 / GIB));
    let available_gb = Some(round2(sys.available_memory() as f64 / GIB));

    let ram = RamHardwareInfo {
        used_gb,
        total_gb,
        available_gb,
        cached_gb: None,
        commit_used_gb: None,
        commit_limit_gb: None,
        speed_mhz: None,
        slots_used: None,
    };

    let gpu = GpuHardwareInfo::default();
    let disk_io = DiskIoStats::default();
    let metrics = build_metric_catalog(&cpu, &ram, &gpu, &disk_io, &[]);

    HardwareTelemetrySnapshot {
        cpu,
        ram,
        gpu,
        disk_io,
        metrics,
        external_providers: Vec::new(),
        sampled_at_unix_ms: now_unix_ms(),
    }
}
