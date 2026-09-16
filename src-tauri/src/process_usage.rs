//! Uso de CPU e GPU por processo, para agregação por grupo de aplicativos.
//!
//! - CPU: delta de `GetProcessTimes` entre amostras consecutivas, normalizado
//!   pelo número de processadores lógicos (100% representa a máquina inteira,
//!   mesma semântica do Gerenciador de Tarefas).
//! - GPU: contadores PDH `\GPU Engine(*)\Utilization Percentage`, somados por
//!   PID. Quando o driver não expõe os contadores, a indisponibilidade é
//!   propagada como `None` em vez de fabricar zero.

#[cfg(not(windows))]
use std::collections::HashMap;

/// Extrai o PID de uma instância PDH de engine gráfico:
/// `pid_4520_luid_0x00000000_0x0000A1B2_phys_0_eng_0_engtype_3D` → `4520`.
pub fn parse_gpu_instance_pid(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    let digits = rest.split('_').next()?;
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

pub fn round_usage(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(windows)]
mod windows_impl {
    use super::{parse_gpu_instance_pid, round_usage};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;

    #[repr(C)]
    struct FileTime {
        low_date_time: u32,
        high_date_time: u32,
    }

    extern "system" {
        fn OpenProcess(
            desired_access: u32,
            inherit_handle: i32,
            process_id: u32,
        ) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetProcessTimes(
            handle: *mut core::ffi::c_void,
            creation_time: *mut FileTime,
            exit_time: *mut FileTime,
            kernel_time: *mut FileTime,
            user_time: *mut FileTime,
        ) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    struct CpuSample {
        cpu_100ns: u64,
        at: Instant,
    }

    // Amostras por PID mantidas entre coletas para calcular o delta de CPU.
    static CPU_SAMPLES: Mutex<Option<HashMap<u32, CpuSample>>> = Mutex::new(None);

    fn filetime_to_u64(value: FileTime) -> u64 {
        ((value.high_date_time as u64) << 32) | (value.low_date_time as u64)
    }

    fn read_cpu_100ns(pid: u32) -> Option<u64> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }

        let mut creation = FileTime {
            low_date_time: 0,
            high_date_time: 0,
        };
        let mut exit = FileTime {
            low_date_time: 0,
            high_date_time: 0,
        };
        let mut kernel = FileTime {
            low_date_time: 0,
            high_date_time: 0,
        };
        let mut user = FileTime {
            low_date_time: 0,
            high_date_time: 0,
        };
        let ok =
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        unsafe { CloseHandle(handle) };

        if ok == 0 {
            return None;
        }
        Some(filetime_to_u64(kernel).saturating_add(filetime_to_u64(user)))
    }

    /// Percentual de CPU por PID desde a coleta anterior. PIDs sem amostra
    /// prévia ficam ausentes do mapa (indisponíveis até a próxima janela).
    pub fn cpu_percent_by_pid(pids: &[u32]) -> HashMap<u32, f64> {
        let now = Instant::now();
        let logical_processors = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1) as f64;

        let mut current = HashMap::with_capacity(pids.len());
        for &pid in pids {
            if let Some(cpu_100ns) = read_cpu_100ns(pid) {
                current.insert(pid, CpuSample { cpu_100ns, at: now });
            }
        }

        let mut usage = HashMap::new();
        if let Ok(mut guard) = CPU_SAMPLES.lock() {
            if let Some(previous) = guard.as_ref() {
                for (&pid, sample) in &current {
                    let Some(old) = previous.get(&pid) else {
                        continue;
                    };
                    let elapsed = now.duration_since(old.at).as_secs_f64();
                    if elapsed <= 0.0 {
                        continue;
                    }
                    let busy_seconds =
                        sample.cpu_100ns.saturating_sub(old.cpu_100ns) as f64 / 10_000_000.0;
                    let percent = busy_seconds / (elapsed * logical_processors) * 100.0;
                    usage.insert(pid, round_usage(percent.clamp(0.0, 100.0)));
                }
            }
            // Substituir o mapa poda PIDs encerrados e mantém a janela de delta.
            *guard = Some(current);
        }
        usage
    }

    /// Utilização agregada de GPU por PID. `None` quando o driver não expõe os
    /// contadores de engine gráfico; mapa vazio significa medição válida sem
    /// nenhuma engine ativa.
    pub fn gpu_percent_by_pid() -> Option<HashMap<u32, f64>> {
        use windows::core::{w, PCWSTR};
        use windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCloseQuery, PdhOpenQueryW, PDH_HCOUNTER, PDH_HQUERY,
        };

        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }

            let mut counter = PDH_HCOUNTER::default();
            let added = PdhAddEnglishCounterW(
                query,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut counter,
            );
            if added != 0 {
                PdhCloseQuery(query);
                return None;
            }

            let usage = collect_gpu_usage(query, counter);
            PdhCloseQuery(query);
            usage
        }
    }

    unsafe fn collect_gpu_usage(
        query: windows::Win32::System::Performance::PDH_HQUERY,
        counter: windows::Win32::System::Performance::PDH_HCOUNTER,
    ) -> Option<HashMap<u32, f64>> {
        use windows::Win32::System::Performance::{
            PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_CSTATUS_NEW_DATA,
            PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
        };

        if PdhCollectQueryData(query) != 0 {
            return None;
        }
        // A primeira coleta após registrar o contador apenas materializa as
        // instâncias; só a segunda torna os valores formatáveis.
        if PdhCollectQueryData(query) != 0 {
            return None;
        }

        let mut size = 0u32;
        let mut count = 0u32;
        let status =
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        if status != PDH_MORE_DATA && status != 0 {
            return None;
        }
        if count == 0 || size == 0 {
            return None;
        }

        // Buffer alinhado a u64 para os itens; o PDH escreve em bytes.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let status = PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut size,
            &mut count,
            Some(buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
        );
        if status != 0 {
            return None;
        }

        let items = std::slice::from_raw_parts(
            buffer.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
            count as usize,
        );

        let mut usage: HashMap<u32, f64> = HashMap::new();
        let mut measured_any = false;
        for item in items {
            let c_status = item.FmtValue.CStatus;
            if c_status != PDH_CSTATUS_VALID_DATA && c_status != PDH_CSTATUS_NEW_DATA {
                continue;
            }
            let Ok(name) = item.szName.to_string() else {
                continue;
            };
            let Some(pid) = parse_gpu_instance_pid(&name) else {
                continue;
            };
            let value = item.FmtValue.Anonymous.doubleValue;
            if !value.is_finite() {
                continue;
            }
            measured_any = true;
            *usage.entry(pid).or_insert(0.0) += value.max(0.0);
        }

        if measured_any {
            Some(usage)
        } else {
            None
        }
    }
}

#[cfg(windows)]
pub use windows_impl::{cpu_percent_by_pid, gpu_percent_by_pid};

#[cfg(not(windows))]
pub fn cpu_percent_by_pid(_pids: &[u32]) -> HashMap<u32, f64> {
    HashMap::new()
}

#[cfg(not(windows))]
pub fn gpu_percent_by_pid() -> Option<HashMap<u32, f64>> {
    None
}

#[cfg(test)]
mod tests {
    use super::{parse_gpu_instance_pid, round_usage};

    #[test]
    fn parses_gpu_engine_instance_pid() {
        assert_eq!(
            parse_gpu_instance_pid("pid_4520_luid_0x00000000_0x0000A1B2_phys_0_eng_0_engtype_3D"),
            Some(4520)
        );
        assert_eq!(
            parse_gpu_instance_pid("pid_7_luid_0x0_0x0_phys_0_eng_1_engtype_Copy"),
            Some(7)
        );
    }

    #[test]
    fn rejects_gpu_instances_without_pid() {
        assert_eq!(parse_gpu_instance_pid("_total"), None);
        assert_eq!(parse_gpu_instance_pid("engtype_3D"), None);
        assert_eq!(parse_gpu_instance_pid("pid__luid"), None);
        assert_eq!(parse_gpu_instance_pid(""), None);
    }

    #[test]
    fn rounds_usage_to_one_decimal() {
        assert_eq!(round_usage(12.34), 12.3);
        assert_eq!(round_usage(0.06), 0.1);
        assert_eq!(round_usage(100.0), 100.0);
    }
}
