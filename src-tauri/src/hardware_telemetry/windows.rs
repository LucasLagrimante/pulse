//! Coleta nativa de telemetria expandida no Windows (100% user-mode).
//!
//! - CPU: Registry (`ProcessorNameString`, `~MHz`), `GetTickCount64`,
//!   `K32GetPerformanceInfo`, `CallNtPowerInformation(ProcessorInformation)` e
//!   contadores PDH em inglês (`PdhAddEnglishCounterW`) para uso por núcleo e
//!   `% Processor Performance`.
//! - RAM: `GlobalMemoryStatusEx` + `K32GetPerformanceInfo` (available, cache,
//!   commit) + `Win32_PhysicalMemory` cacheado uma única vez por sessão.
//! - GPU: PDH `\GPU Adapter Memory(*)\Dedicated Usage` e `Shared Usage` (uso
//!   global do adaptador) + `IDXGIFactory1` (nome, VRAM total e LUID) +
//!   `nvidia-smi` estendido cacheado (com cache negativo de sessão quando ausente).
//! - Disco I/O: contadores PDH em inglês para `PhysicalDisk(_Total)` e por
//!   disco físico (`\PhysicalDisk(*)\Disk Read/Write Bytes/sec`).
//! - Sensores extras: PDH `\Thermal Zone Information(*)\Temperature`,
//!   `Global\` / `Local\HWiNFO_SENS_SM2` (Shared Memory zero-overhead) + WMI
//!   `root/LibreHardwareMonitor` e `root/OpenHardwareMonitor` (cache negativo de 60s).

use super::{
    build_metric_catalog, kelvin_to_valid_celsius, now_unix_ms, parse_hwinfo_shared_memory,
    parse_lhm_sensors_json, parse_nvidia_extended_csv, round1, round2, sanitize_id,
    select_gpu_memory_usage_gb, CpuHardwareInfo, DiskIoStats, ExternalSensorReading,
    GpuHardwareInfo, HardwareTelemetrySnapshot, NvidiaExtendedSample, PhysicalDiskIo,
    RamHardwareInfo, SensorCategory,
};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MIB: f64 = 1024.0 * 1024.0;
const NVIDIA_CACHE_TTL: Duration = Duration::from_secs(4);
const EXTERNAL_WMI_ACTIVE_TTL: Duration = Duration::from_secs(5);
const EXTERNAL_WMI_NEGATIVE_TTL: Duration = Duration::from_secs(60);

struct StaticCpuMeta {
    model: Option<String>,
    base_clock_mhz: Option<u32>,
    logical_cores: u32,
}

static STATIC_CPU_META: OnceLock<StaticCpuMeta> = OnceLock::new();

fn read_static_cpu_meta() -> &'static StaticCpuMeta {
    STATIC_CPU_META.get_or_init(|| {
        let logical_cores = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
        let (model, base_clock_mhz) = read_cpu_registry();
        StaticCpuMeta {
            model,
            base_clock_mhz,
            logical_cores,
        }
    })
}

fn read_cpu_registry() -> (Option<String>, Option<u32>) {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
        REG_DWORD, REG_SZ, REG_VALUE_TYPE,
    };

    unsafe {
        let mut hkey = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
            Some(0),
            KEY_READ,
            &mut hkey,
        );
        if status.is_err() {
            return (None, None);
        }

        // ProcessorNameString
        let mut name_buf = [0u16; 256];
        let mut name_bytes = (name_buf.len() * 2) as u32;
        let mut val_type = REG_VALUE_TYPE::default();
        let model = if RegQueryValueExW(
            hkey,
            w!("ProcessorNameString"),
            None,
            Some(&mut val_type),
            Some(name_buf.as_mut_ptr() as *mut u8),
            Some(&mut name_bytes),
        )
        .is_ok()
            && val_type == REG_SZ
        {
            let slice_len = (name_bytes as usize / 2).min(name_buf.len());
            let end = name_buf[..slice_len]
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(slice_len);
            let text = String::from_utf16_lossy(&name_buf[..end]).trim().to_owned();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        } else {
            None
        };

        // ~MHz
        let mut mhz: u32 = 0;
        let mut mhz_bytes = std::mem::size_of::<u32>() as u32;
        let mut mhz_type = REG_VALUE_TYPE::default();
        let base_clock = if RegQueryValueExW(
            hkey,
            w!("~MHz"),
            None,
            Some(&mut mhz_type),
            Some(&mut mhz as *mut u32 as *mut u8),
            Some(&mut mhz_bytes),
        )
        .is_ok()
            && mhz_type == REG_DWORD
            && mhz > 0
        {
            Some(mhz)
        } else {
            None
        };

        let _ = RegCloseKey(hkey);
        (model, base_clock)
    }
}

/// Lê frequência atual média e máxima reportadas pelo power manager do kernel
/// (`CallNtPowerInformation(ProcessorInformation)`). Nunca inventa valor sem leitura.
fn read_cpu_power_info(logical_cores: u32) -> (Option<f64>, Option<u32>) {
    use windows_sys::Win32::System::Power::{
        CallNtPowerInformation, ProcessorInformation, PROCESSOR_POWER_INFORMATION,
    };

    let count = logical_cores.max(1) as usize;
    let mut entries = vec![PROCESSOR_POWER_INFORMATION::default(); count];
    let buf_bytes = (entries.len() * std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()) as u32;
    let status = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            std::ptr::null(),
            0,
            entries.as_mut_ptr().cast(),
            buf_bytes,
        )
    };
    if status != 0 {
        return (None, None);
    }

    let mut current_sum = 0.0f64;
    let mut current_count = 0usize;
    let mut max_mhz = 0u32;
    for entry in &entries {
        if entry.CurrentMhz > 0 {
            current_sum += entry.CurrentMhz as f64;
            current_count += 1;
        }
        if entry.MaxMhz > max_mhz {
            max_mhz = entry.MaxMhz;
        }
    }
    let current = if current_count > 0 {
        Some(round1(current_sum / current_count as f64))
    } else {
        None
    };
    let max_clock = if max_mhz > 0 { Some(max_mhz) } else { None };
    (current, max_clock)
}

struct PerfInfoSnapshot {
    cached_gb: Option<f64>,
    commit_used_gb: Option<f64>,
    commit_limit_gb: Option<f64>,
    process_count: Option<u32>,
    thread_count: Option<u32>,
    handle_count: Option<u32>,
}

fn read_performance_info() -> PerfInfoSnapshot {
    use windows::Win32::System::ProcessStatus::{K32GetPerformanceInfo, PERFORMANCE_INFORMATION};

    let mut info = PERFORMANCE_INFORMATION::default();
    let cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
    let ok = unsafe { K32GetPerformanceInfo(&mut info, cb) };
    if !ok.as_bool() || info.PageSize == 0 {
        return PerfInfoSnapshot {
            cached_gb: None,
            commit_used_gb: None,
            commit_limit_gb: None,
            process_count: None,
            thread_count: None,
            handle_count: None,
        };
    }
    let page = info.PageSize as f64;
    PerfInfoSnapshot {
        cached_gb: Some(round2((info.SystemCache as f64 * page) / GIB)),
        commit_used_gb: Some(round2((info.CommitTotal as f64 * page) / GIB)),
        commit_limit_gb: Some(round2((info.CommitLimit as f64 * page) / GIB)),
        process_count: Some(info.ProcessCount),
        thread_count: Some(info.ThreadCount),
        handle_count: Some(info.HandleCount),
    }
}

fn read_memory_status() -> (Option<f64>, Option<f64>, Option<f64>) {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() || status.ullTotalPhys == 0 {
        return (None, None, None);
    }
    let total = round2(status.ullTotalPhys as f64 / GIB);
    let avail = round2(status.ullAvailPhys as f64 / GIB);
    let used = round2(status.ullTotalPhys.saturating_sub(status.ullAvailPhys) as f64 / GIB);
    (Some(used), Some(total), Some(avail))
}

fn read_uptime_seconds() -> Option<u64> {
    use windows::Win32::System::SystemInformation::GetTickCount64;
    let ms = unsafe { GetTickCount64() };
    Some(ms / 1000)
}

// ============================================================================
// PDH persistente (contadores em inglês, zero localização pt-BR)
// ============================================================================

struct PdhHandleBundle {
    query: isize,
    cpu_cores: Option<isize>,
    cpu_perf_pct: Option<isize>,
    disk_read_bps: Option<isize>,
    disk_write_bps: Option<isize>,
    disk_active_pct: Option<isize>,
    disk_queue: Option<isize>,
    disk_read_iops: Option<isize>,
    disk_write_iops: Option<isize>,
    disk_per_read_bps: Option<isize>,
    disk_per_write_bps: Option<isize>,
    gpu_dedicated_used: Option<isize>,
    gpu_shared_used: Option<isize>,
    thermal_zone_temp: Option<isize>,
}

static PDH_BUNDLE: Mutex<Option<PdhHandleBundle>> = Mutex::new(None);

#[derive(Default)]
struct PdhReading {
    per_core_percent: Vec<f64>,
    cpu_perf_percent: Option<f64>,
    disk_io: DiskIoStats,
    gpu_dedicated_samples: Vec<(String, f64)>,
    gpu_shared_samples: Vec<(String, f64)>,
    thermal_sensors: Vec<ExternalSensorReading>,
}

unsafe fn read_pdh_array(counter_opt: Option<isize>) -> Vec<(String, f64)> {
    use windows::Win32::System::Performance::{
        PdhGetFormattedCounterArrayW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_MORE_DATA,
    };

    let Some(raw) = counter_opt else {
        return Vec::new();
    };
    let counter = PDH_HCOUNTER(raw as *mut core::ffi::c_void);
    let mut size = 0u32;
    let mut count = 0u32;
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
    };
    if (status != PDH_MORE_DATA && status != 0) || count == 0 || size == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut size,
            &mut count,
            Some(buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    let items = unsafe {
        std::slice::from_raw_parts(
            buffer.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
            count as usize,
        )
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.FmtValue.CStatus != PDH_CSTATUS_VALID_DATA
            && item.FmtValue.CStatus != PDH_CSTATUS_NEW_DATA
        {
            continue;
        }
        let Ok(name) = item.szName.to_string() else {
            continue;
        };
        let value = item.FmtValue.Anonymous.doubleValue;
        if value.is_finite() {
            out.push((name.trim().to_owned(), value));
        }
    }
    out
}

fn merge_per_disk_io(reads: Vec<(String, f64)>, writes: Vec<(String, f64)>) -> Vec<PhysicalDiskIo> {
    let mut map: BTreeMap<String, (Option<f64>, Option<f64>)> = BTreeMap::new();
    for (name, bps) in reads {
        if name.is_empty() || name.eq_ignore_ascii_case("_Total") || bps < 0.0 {
            continue;
        }
        map.entry(name).or_default().0 = Some(round1(bps / MIB));
    }
    for (name, bps) in writes {
        if name.is_empty() || name.eq_ignore_ascii_case("_Total") || bps < 0.0 {
            continue;
        }
        map.entry(name).or_default().1 = Some(round1(bps / MIB));
    }
    map.into_iter()
        .map(|(name, (read_mbps, write_mbps))| PhysicalDiskIo {
            name,
            read_mbps,
            write_mbps,
        })
        .collect()
}

fn clean_thermal_zone_label(raw: &str) -> String {
    let tail = raw.rsplit('\\').next().unwrap_or(raw).trim();
    let stripped = tail.trim_start_matches('_').trim();
    if stripped.is_empty() {
        "ACPI".to_owned()
    } else {
        stripped.to_owned()
    }
}

fn build_pdh_thermal_sensors(samples: Vec<(String, f64)>) -> Vec<ExternalSensorReading> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (raw_name, kelvin) in samples {
        if raw_name.is_empty() || raw_name.eq_ignore_ascii_case("_Total") {
            continue;
        }
        let Some(celsius) = kelvin_to_valid_celsius(kelvin) else {
            continue;
        };
        let zone = clean_thermal_zone_label(&raw_name);
        let id = format!("pdh-tz-{}", sanitize_id(&zone));
        if !seen.insert(id.clone()) {
            continue;
        }
        out.push(ExternalSensorReading {
            id,
            label: format!("Zona térmica {zone}"),
            category: SensorCategory::Temperature,
            value: celsius,
            unit: "°C".into(),
            source: "ACPI (PDH)".into(),
            group_hint: Some("sensors".into()),
        });
    }
    out
}

fn read_pdh_telemetry() -> PdhReading {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
        PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
        PDH_HCOUNTER, PDH_HQUERY,
    };

    let Ok(mut guard) = PDH_BUNDLE.lock() else {
        return PdhReading::default();
    };

    if guard.is_none() {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return PdhReading::default();
            }
            let add = |path: PCWSTR| -> Option<isize> {
                let mut h = PDH_HCOUNTER::default();
                if PdhAddEnglishCounterW(query, path, 0, &mut h) == 0 {
                    Some(h.0 as isize)
                } else {
                    None
                }
            };
            let bundle = PdhHandleBundle {
                query: query.0 as isize,
                cpu_cores: add(w!("\\Processor(*)\\% Processor Time")),
                cpu_perf_pct: add(w!(
                    "\\Processor Information(_Total)\\% Processor Performance"
                )),
                disk_read_bps: add(w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec")),
                disk_write_bps: add(w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec")),
                disk_active_pct: add(w!("\\PhysicalDisk(_Total)\\% Disk Time")),
                disk_queue: add(w!("\\PhysicalDisk(_Total)\\Avg. Disk Queue Length")),
                disk_read_iops: add(w!("\\PhysicalDisk(_Total)\\Disk Reads/sec")),
                disk_write_iops: add(w!("\\PhysicalDisk(_Total)\\Disk Writes/sec")),
                disk_per_read_bps: add(w!("\\PhysicalDisk(*)\\Disk Read Bytes/sec")),
                disk_per_write_bps: add(w!("\\PhysicalDisk(*)\\Disk Write Bytes/sec")),
                gpu_dedicated_used: add(w!("\\GPU Adapter Memory(*)\\Dedicated Usage")),
                gpu_shared_used: add(w!("\\GPU Adapter Memory(*)\\Shared Usage")),
                thermal_zone_temp: add(w!("\\Thermal Zone Information(*)\\Temperature")),
            };
            // Prime inicial + pequena pausa para o 1º snapshot já obter delta real
            // de CPU por núcleo e Disco I/O.
            let _ = PdhCollectQueryData(query);
            std::thread::sleep(Duration::from_millis(80));
            *guard = Some(bundle);
        }
    }

    let Some(bundle) = guard.as_mut() else {
        return PdhReading::default();
    };

    unsafe {
        let query = PDH_HQUERY(bundle.query as *mut core::ffi::c_void);
        if PdhCollectQueryData(query) != 0 {
            return PdhReading::default();
        }

        let read_scalar = |counter_opt: Option<isize>| -> Option<f64> {
            let raw = counter_opt?;
            let counter = PDH_HCOUNTER(raw as *mut core::ffi::c_void);
            let mut val = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut val) != 0 {
                return None;
            }
            if val.CStatus != PDH_CSTATUS_VALID_DATA && val.CStatus != PDH_CSTATUS_NEW_DATA {
                return None;
            }
            let n = val.Anonymous.doubleValue;
            if n.is_finite() && n >= 0.0 {
                Some(n)
            } else {
                None
            }
        };

        let mut per_core: Vec<(u32, f64)> = read_pdh_array(bundle.cpu_cores)
            .into_iter()
            .filter_map(|(name, v)| {
                let core_idx = name.parse::<u32>().ok()?;
                Some((core_idx, round1(v.clamp(0.0, 100.0))))
            })
            .collect();
        per_core.sort_by_key(|(idx, _)| *idx);

        let per_disk = merge_per_disk_io(
            read_pdh_array(bundle.disk_per_read_bps),
            read_pdh_array(bundle.disk_per_write_bps),
        );
        let gpu_dedicated_samples = read_pdh_array(bundle.gpu_dedicated_used);
        let gpu_shared_samples = read_pdh_array(bundle.gpu_shared_used);
        let thermal_sensors = build_pdh_thermal_sensors(read_pdh_array(bundle.thermal_zone_temp));

        PdhReading {
            per_core_percent: per_core.into_iter().map(|(_, v)| v).collect(),
            cpu_perf_percent: read_scalar(bundle.cpu_perf_pct),
            disk_io: DiskIoStats {
                read_mbps: read_scalar(bundle.disk_read_bps).map(|b| round1(b / MIB)),
                write_mbps: read_scalar(bundle.disk_write_bps).map(|b| round1(b / MIB)),
                active_percent: read_scalar(bundle.disk_active_pct)
                    .map(|p| round1(p.clamp(0.0, 100.0))),
                queue_length: read_scalar(bundle.disk_queue).map(round2),
                read_iops: read_scalar(bundle.disk_read_iops).map(round1),
                write_iops: read_scalar(bundle.disk_write_iops).map(round1),
                per_disk,
            },
            gpu_dedicated_samples,
            gpu_shared_samples,
            thermal_sensors,
        }
    }
}

// ============================================================================
// GPU via DXGI (nome, VRAM total, LUID) + PDH (VRAM global) + nvidia-smi
// ============================================================================

#[derive(Default)]
struct DxgiGpuSample {
    name: Option<String>,
    vram_total_gb: Option<f64>,
    adapter_luid: Option<String>,
}

fn read_dxgi_gpu() -> DxgiGpuSample {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
    };

    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return DxgiGpuSample::default();
        };
        let mut best: Option<(u64, DxgiGpuSample)> = None;
        let mut idx = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(idx) {
            idx += 1;
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let end = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end])
                .trim()
                .to_owned();
            let dedicated_bytes = desc.DedicatedVideoMemory as u64;
            let luid = desc.AdapterLuid;
            let adapter_luid = Some(format!(
                "luid_0x{:08x}_0x{:08x}",
                luid.HighPart as u32, luid.LowPart
            ));

            let sample = DxgiGpuSample {
                name: if name.is_empty() { None } else { Some(name) },
                vram_total_gb: if dedicated_bytes > 0 {
                    Some(round2(dedicated_bytes as f64 / GIB))
                } else {
                    None
                },
                adapter_luid,
            };

            let should_replace = best
                .as_ref()
                .map(|(best_bytes, _)| dedicated_bytes > *best_bytes)
                .unwrap_or(true);
            if should_replace {
                best = Some((dedicated_bytes, sample));
            }
        }
        best.map(|(_, s)| s).unwrap_or_default()
    }
}

static NVIDIA_CACHE: Mutex<Option<(Instant, Option<NvidiaExtendedSample>)>> = Mutex::new(None);
static NVIDIA_SMI_MISSING: AtomicBool = AtomicBool::new(false);

fn read_nvidia_extended_cached() -> Option<NvidiaExtendedSample> {
    if NVIDIA_SMI_MISSING.load(Ordering::Relaxed) {
        return None;
    }

    if let Ok(guard) = NVIDIA_CACHE.lock() {
        if let Some((at, cached)) = guard.as_ref() {
            if at.elapsed() < NVIDIA_CACHE_TTL {
                return cached.clone();
            }
        }
    }

    let output = match crate::process::hidden_command("nvidia-smi")
        .args([
            "--query-gpu=index,name,temperature.gpu,fan.speed,clocks.gr,clocks.mem,power.draw,power.limit,memory.used,memory.total,utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        Ok(out) if out.status.success() => Some(out),
        Ok(_) => None,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                NVIDIA_SMI_MISSING.store(true, Ordering::Relaxed);
            }
            None
        }
    };

    let parsed = output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|text| parse_nvidia_extended_csv(&text).into_iter().next());

    if let Ok(mut guard) = NVIDIA_CACHE.lock() {
        *guard = Some((Instant::now(), parsed.clone()));
    }
    parsed
}

// ============================================================================
// HWiNFO Shared Memory (Global + Local) + WMI estático (1x/sessão) + LHM/OHM
// ============================================================================

unsafe fn open_hwinfo_mapping() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::w;
    use windows::Win32::System::Memory::{OpenFileMappingW, FILE_MAP_READ};

    for name in [w!("Global\\HWiNFO_SENS_SM2"), w!("Local\\HWiNFO_SENS_SM2")] {
        if let Ok(handle) = unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, name) } {
            if !handle.is_invalid() {
                return Some(handle);
            }
        }
    }
    None
}

fn read_hwinfo_shared_memory() -> Vec<ExternalSensorReading> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Memory::{MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ};

    unsafe {
        let Some(handle) = open_hwinfo_mapping() else {
            return Vec::new();
        };
        // Mapeia primeiro o cabeçalho (44 bytes) para saber o tamanho real do buffer.
        let header_view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 44);
        if header_view.Value.is_null() {
            let _ = CloseHandle(handle);
            return Vec::new();
        }
        let header_slice = std::slice::from_raw_parts(header_view.Value as *const u8, 44);
        let reading_offset = u32::from_le_bytes([
            header_slice[32],
            header_slice[33],
            header_slice[34],
            header_slice[35],
        ]) as usize;
        let reading_size = u32::from_le_bytes([
            header_slice[36],
            header_slice[37],
            header_slice[38],
            header_slice[39],
        ]) as usize;
        let reading_count = u32::from_le_bytes([
            header_slice[40],
            header_slice[41],
            header_slice[42],
            header_slice[43],
        ])
        .min(512) as usize;
        let _ = UnmapViewOfFile(header_view);

        let total_size = reading_offset
            .saturating_add(reading_size.saturating_mul(reading_count))
            .min(2 * 1024 * 1024);
        if total_size < 44 {
            let _ = CloseHandle(handle);
            return Vec::new();
        }

        let full_view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, total_size);
        let readings = if !full_view.Value.is_null() {
            let bytes = std::slice::from_raw_parts(full_view.Value as *const u8, total_size);
            let parsed = parse_hwinfo_shared_memory(bytes);
            let _ = UnmapViewOfFile(full_view);
            parsed
        } else {
            Vec::new()
        };
        let _ = CloseHandle(handle);
        readings
    }
}

#[derive(Clone, Default)]
struct StaticWmiMeta {
    ram_speed_mhz: Option<u32>,
    ram_slots_used: Option<u32>,
    physical_cores: Option<u32>,
}

static STATIC_WMI_META: OnceLock<StaticWmiMeta> = OnceLock::new();

fn read_static_wmi_meta() -> &'static StaticWmiMeta {
    STATIC_WMI_META.get_or_init(|| {
        let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$dimms = @(Get-CimInstance Win32_PhysicalMemory | Select-Object ConfiguredClockSpeed, Speed)
$cpuCores = (Get-CimInstance Win32_Processor | Measure-Object -Property NumberOfCores -Sum).Sum
@{ dimms = @($dimms); physicalCores = $cpuCores } | ConvertTo-Json -Compress -Depth 3
"#;
        let mut meta = StaticWmiMeta::default();
        if let Ok(output) = crate::process::hidden_command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
        {
            if output.status.success() {
                if let Ok(text) = std::str::from_utf8(&output.stdout) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text.trim()) {
                        let dimms: Vec<&serde_json::Value> = match json.get("dimms") {
                            Some(serde_json::Value::Array(arr)) => arr.iter().collect(),
                            Some(single @ serde_json::Value::Object(_)) => vec![single],
                            _ => Vec::new(),
                        };
                        meta.ram_slots_used = if dimms.is_empty() {
                            None
                        } else {
                            Some(dimms.len() as u32)
                        };
                        meta.ram_speed_mhz = dimms
                            .iter()
                            .filter_map(|d| {
                                d.get("ConfiguredClockSpeed")
                                    .and_then(|v| v.as_u64())
                                    .filter(|v| *v > 0)
                                    .or_else(|| {
                                        d.get("Speed").and_then(|v| v.as_u64()).filter(|v| *v > 0)
                                    })
                            })
                            .max()
                            .map(|v| v as u32);
                        meta.physical_cores = json
                            .get("physicalCores")
                            .and_then(|v| v.as_u64())
                            .filter(|v| *v > 0)
                            .map(|v| v as u32);
                    }
                }
            }
        }
        meta
    })
}

struct ExternalWmiCacheEntry {
    sampled_at: Instant,
    sensors: Vec<ExternalSensorReading>,
    found_provider: bool,
}

static EXTERNAL_WMI_CACHE: Mutex<Option<ExternalWmiCacheEntry>> = Mutex::new(None);

fn read_external_wmi_sensors_cached() -> Vec<ExternalSensorReading> {
    if let Ok(guard) = EXTERNAL_WMI_CACHE.lock() {
        if let Some(entry) = guard.as_ref() {
            let ttl = if entry.found_provider {
                EXTERNAL_WMI_ACTIVE_TTL
            } else {
                EXTERNAL_WMI_NEGATIVE_TTL
            };
            if entry.sampled_at.elapsed() < ttl {
                return entry.sensors.clone();
            }
        }
    }

    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$lhm = @(Get-CimInstance -Namespace root/LibreHardwareMonitor -ClassName Sensor | Where-Object { $_.SensorType -in @('Temperature','Fan','Clock','Power','Voltage','Control','Load') } | Select-Object Name, Identifier, SensorType, Value)
$ohm = @(Get-CimInstance -Namespace root/OpenHardwareMonitor -ClassName Sensor | Where-Object { $_.SensorType -in @('Temperature','Fan','Clock','Power','Voltage','Control','Load') } | Select-Object Name, Identifier, SensorType, Value)
@{ lhm = @($lhm); ohm = @($ohm) } | ConvertTo-Json -Compress -Depth 4
"#;

    let mut sensors = Vec::new();
    if let Ok(output) = crate::process::hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    {
        if output.status.success() {
            if let Ok(text) = std::str::from_utf8(&output.stdout) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(text.trim()) {
                    sensors.extend(parse_lhm_sensors_json(
                        &json,
                        "lhm",
                        "LibreHardwareMonitor",
                        "lhm",
                    ));
                    sensors.extend(parse_lhm_sensors_json(
                        &json,
                        "ohm",
                        "OpenHardwareMonitor",
                        "ohm",
                    ));
                }
            }
        }
    }

    let found_provider = !sensors.is_empty();
    if let Ok(mut guard) = EXTERNAL_WMI_CACHE.lock() {
        *guard = Some(ExternalWmiCacheEntry {
            sampled_at: Instant::now(),
            sensors: sensors.clone(),
            found_provider,
        });
    }
    sensors
}

pub fn collect_snapshot() -> HardwareTelemetrySnapshot {
    let cpu_meta = read_static_cpu_meta();
    let static_wmi = read_static_wmi_meta();
    let (power_current_mhz, power_max_mhz) = read_cpu_power_info(cpu_meta.logical_cores);
    let perf_info = read_performance_info();
    let (ram_used_gb, ram_total_gb, ram_available_gb) = read_memory_status();
    let pdh = read_pdh_telemetry();
    let dxgi = read_dxgi_gpu();
    let nvidia = read_nvidia_extended_cached();

    let mut sensors = read_hwinfo_shared_memory();
    sensors.extend(read_external_wmi_sensors_cached());

    let mut provider_set: HashSet<String> = HashSet::new();
    for s in &sensors {
        provider_set.insert(s.source.clone());
    }
    let mut external_providers: Vec<String> = provider_set.into_iter().collect();
    external_providers.sort();

    // Zonas térmicas ACPI via PDH são nativas do Windows (não provedor externo).
    sensors.extend(pdh.thermal_sensors);

    let base_clock_mhz = cpu_meta.base_clock_mhz.or(power_max_mhz);
    let current_clock_mhz = match (base_clock_mhz, pdh.cpu_perf_percent) {
        (Some(base), Some(perf_pct)) if perf_pct > 0.0 => {
            Some(round1(base as f64 * (perf_pct / 100.0)))
        }
        _ => power_current_mhz,
    };

    let cpu = CpuHardwareInfo {
        model: cpu_meta.model.clone(),
        physical_cores: static_wmi.physical_cores,
        logical_cores: cpu_meta.logical_cores,
        base_clock_mhz,
        current_clock_mhz,
        per_core_percent: pdh.per_core_percent,
        uptime_seconds: read_uptime_seconds(),
        process_count: perf_info.process_count,
        thread_count: perf_info.thread_count,
        handle_count: perf_info.handle_count,
    };

    let ram = RamHardwareInfo {
        used_gb: ram_used_gb,
        total_gb: ram_total_gb,
        available_gb: ram_available_gb,
        cached_gb: perf_info.cached_gb,
        commit_used_gb: perf_info.commit_used_gb,
        commit_limit_gb: perf_info.commit_limit_gb,
        speed_mhz: static_wmi.ram_speed_mhz,
        slots_used: static_wmi.ram_slots_used,
    };

    let gpu_name = nvidia
        .as_ref()
        .map(|n| n.name.clone())
        .filter(|s| !s.is_empty())
        .or(dxgi.name);
    let pdh_vram_used_gb =
        select_gpu_memory_usage_gb(&pdh.gpu_dedicated_samples, dxgi.adapter_luid.as_deref());
    let pdh_shared_used_gb =
        select_gpu_memory_usage_gb(&pdh.gpu_shared_samples, dxgi.adapter_luid.as_deref());
    let nvidia_vram_used_gb = nvidia
        .as_ref()
        .and_then(|n| n.vram_used_mib)
        .map(|m| round2(m / 1024.0));
    let vram_used_gb = nvidia_vram_used_gb.or(pdh_vram_used_gb);
    let vram_total_gb = dxgi.vram_total_gb.or_else(|| {
        nvidia
            .as_ref()
            .and_then(|n| n.vram_total_mib)
            .map(|m| round2(m / 1024.0))
    });
    let gpu_source = match (
        &nvidia,
        dxgi.vram_total_gb.is_some(),
        pdh_vram_used_gb.is_some(),
    ) {
        (Some(_), true, _) => Some("DXGI + nvidia-smi".into()),
        (Some(_), false, _) => Some("nvidia-smi".into()),
        (None, true, true) => Some("DXGI + PDH".into()),
        (None, true, false) => Some("DXGI".into()),
        (None, false, true) => Some("PDH".into()),
        (None, false, false) => None,
    };

    let gpu = GpuHardwareInfo {
        name: gpu_name,
        usage_percent: nvidia.as_ref().and_then(|n| n.utilization_percent),
        vram_used_gb,
        vram_total_gb,
        shared_used_gb: pdh_shared_used_gb,
        temperature_celsius: nvidia.as_ref().and_then(|n| n.temperature_celsius),
        fan_percent: nvidia.as_ref().and_then(|n| n.fan_percent),
        fan_rpm: None,
        core_clock_mhz: nvidia.as_ref().and_then(|n| n.core_clock_mhz),
        memory_clock_mhz: nvidia.as_ref().and_then(|n| n.memory_clock_mhz),
        power_watts: nvidia.as_ref().and_then(|n| n.power_watts),
        power_limit_watts: nvidia.as_ref().and_then(|n| n.power_limit_watts),
        source: gpu_source,
    };

    let metrics = build_metric_catalog(&cpu, &ram, &gpu, &pdh.disk_io, &sensors);

    HardwareTelemetrySnapshot {
        cpu,
        ram,
        gpu,
        disk_io: pdh.disk_io,
        metrics,
        external_providers,
        sampled_at_unix_ms: now_unix_ms(),
    }
}
