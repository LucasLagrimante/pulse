use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TopProcess {
    pub name: String,
    pub id: u32,
    pub memory_mb: f64,
    #[serde(default)]
    pub process_ids: Vec<u32>,
    #[serde(default = "default_process_count")]
    pub process_count: usize,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
}

fn default_process_count() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemOverview {
    pub cpu_percent: Option<f64>,
    pub memory_used_gb: Option<f64>,
    pub memory_total_gb: Option<f64>,
    pub disk_used_gb: Option<f64>,
    pub disk_total_gb: Option<f64>,
    pub startup_item_count: u32,
    pub top_processes: Vec<TopProcess>,
    pub sampled_at_unix_ms: u64,
    pub source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSystemOverview {
    cpu_percent: Option<f64>,
    memory_used_gb: Option<f64>,
    memory_total_gb: Option<f64>,
    disk_used_gb: Option<f64>,
    disk_total_gb: Option<f64>,
    startup_item_count: u32,
    top_processes: Vec<TopProcess>,
}

const OVERVIEW_SCRIPT: &str = r#"
$all = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.WorkingSet64 -ge 0 })
$processById = @{}
$all | ForEach-Object { $processById[[int]$_.Id] = $_ }
$parentById = @{}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | ForEach-Object {
  $parentById[[int]$_.ProcessId] = [int]$_.ParentProcessId
}

# WebView2 é um runtime compartilhado: o nome do executável identifica o
# runtime, não o aplicativo que o está usando. Suba pela cadeia de pais até
# encontrar o primeiro processo que não seja outro WebView2 e use-o como dono.
$records = @($all | ForEach-Object {
  $groupKey = $_.ProcessName.ToLowerInvariant()
  if ($groupKey -eq "msedgewebview2") {
    $ownerId = $parentById[[int]$_.Id]
    while ($ownerId -and $processById.ContainsKey($ownerId) -and
      $processById[$ownerId].ProcessName.ToLowerInvariant() -eq "msedgewebview2") {
      $ownerId = $parentById[$ownerId]
    }
    if ($ownerId -and $processById.ContainsKey($ownerId)) {
      $groupKey = $processById[$ownerId].ProcessName.ToLowerInvariant()
    }
  }
  [pscustomobject]@{
    groupKey = $groupKey
    process = $_
  }
})
$processes = @($records | Group-Object groupKey | ForEach-Object {
  $group = @($_.Group | ForEach-Object { $_.process } | Sort-Object WorkingSet64 -Descending)
  $representative = @($group | Where-Object {
    $_.ProcessName.ToLowerInvariant() -ne "msedgewebview2"
  })[0]
  if (-not $representative) { $representative = $group[0] }
  [pscustomobject]@{
    name = $representative.ProcessName
    id = $representative.Id
    processIds = @($group | ForEach-Object { $_.Id })
    processCount = $group.Count
    memoryMb = [math]::Round((($group | Measure-Object WorkingSet64 -Sum).Sum / 1MB), 1)
  }
} | Sort-Object memoryMb -Descending | Select-Object -First 25)
$cpuSamples = @(Get-CimInstance Win32_Processor -ErrorAction SilentlyContinue | ForEach-Object { $_.LoadPercentage } | Where-Object { $_ -ne $null })
$cpuAverage = if ($cpuSamples.Count -gt 0) { [math]::Round((($cpuSamples | Measure-Object -Average).Average), 1) } else { $null }
$enrichedProcesses = @($processes | ForEach-Object {
  $displayName = [string]$_.name
  $publisher = $null
  try {
    $nativeProcess = Get-Process -Id ([int]$_.id) -ErrorAction Stop
    $path = $nativeProcess.Path
    if ($path) {
      $versionInfo = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($path)
      if ($versionInfo.FileDescription) { $displayName = [string]$versionInfo.FileDescription }
      if ($versionInfo.CompanyName) { $publisher = [string]$versionInfo.CompanyName }
    }
  } catch {}
  [pscustomobject]@{
    name = $_.name
    id = $_.id
    processIds = $_.processIds
    processCount = $_.processCount
    memoryMb = $_.memoryMb
    displayName = $displayName
    publisher = $publisher
  }
})
@{
  # LoadPercentage é uma leitura real de instante; a leitura Win32 abaixo a
  # substitui por uma amostra de delta assim que ela estiver disponível.
  cpuPercent = $cpuAverage
  memoryUsedGb = $null
  memoryTotalGb = $null
  diskUsedGb = $null
  diskTotalGb = $null
  startupItemCount = 0
  topProcesses = $enrichedProcesses
} | ConvertTo-Json -Compress -Depth 3
"#;

struct OverviewCache {
    data: SystemOverview,
    last_fetched: Instant,
}

static CACHE: Mutex<Option<OverviewCache>> = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_millis(3500);

fn now_unix_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|e| format!("Relógio do sistema inválido: {e}"))
}

pub fn parse_overview(payload: &str, sampled_at_unix_ms: u64) -> Result<SystemOverview, String> {
    let raw: RawSystemOverview = serde_json::from_str(payload)
        .map_err(|e| format!("Não foi possível interpretar o resumo do Windows: {e}"))?;
    Ok(SystemOverview {
        cpu_percent: raw.cpu_percent,
        memory_used_gb: raw.memory_used_gb,
        memory_total_gb: raw.memory_total_gb,
        disk_used_gb: raw.disk_used_gb,
        disk_total_gb: raw.disk_total_gb,
        startup_item_count: raw.startup_item_count,
        top_processes: raw.top_processes.into_iter().take(25).collect(),
        sampled_at_unix_ms,
        source: "Windows CIM + Process API".to_owned(),
    })
}

#[cfg(windows)]
mod win32 {
    use std::sync::Mutex;

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FileTime {
        low_date_time: u32,
        high_date_time: u32,
    }

    extern "system" {
        fn GlobalMemoryStatusEx(stat: *mut MemoryStatusEx) -> i32;
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_number_of_bytes: *mut u64,
            total_number_of_free_bytes: *mut u64,
        ) -> i32;
        fn GetSystemTimes(
            idle_time: *mut FileTime,
            kernel_time: *mut FileTime,
            user_time: *mut FileTime,
        ) -> i32;
    }

    fn file_time_to_u64(ft: FileTime) -> u64 {
        ((ft.high_date_time as u64) << 32) | (ft.low_date_time as u64)
    }

    struct CpuTimes {
        idle: u64,
        kernel: u64,
        user: u64,
    }

    static LAST_CPU: Mutex<Option<CpuTimes>> = Mutex::new(None);

    fn read_cpu_times() -> Option<CpuTimes> {
        let mut idle = FileTime {
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

        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }

        let idle_u = file_time_to_u64(idle);
        let kernel_u = file_time_to_u64(kernel);
        let user_u = file_time_to_u64(user);

        Some(CpuTimes {
            idle: idle_u,
            kernel: kernel_u,
            user: user_u,
        })
    }

    pub fn read_cpu_percent() -> Option<f64> {
        let current = read_cpu_times()?;
        let mut guard = LAST_CPU.lock().ok()?;
        if let Some(ref prev) = *guard {
            let idle_diff = current.idle.saturating_sub(prev.idle);
            let kernel_diff = current.kernel.saturating_sub(prev.kernel);
            let user_diff = current.user.saturating_sub(prev.user);
            let total = kernel_diff + user_diff;

            *guard = Some(current);

            if total > 0 {
                let busy = total.saturating_sub(idle_diff);
                let pct = (busy as f64 / total as f64) * 100.0;
                return Some((pct * 10.0).round() / 10.0);
            }
        } else {
            *guard = Some(current);
        }
        None
    }

    pub fn read_memory_gb() -> Option<(f64, f64)> {
        let mut stat = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        if unsafe { GlobalMemoryStatusEx(&mut stat) } == 0 {
            return None;
        }
        let gb = 1024.0 * 1024.0 * 1024.0;
        let total = (stat.total_phys as f64 / gb * 10.0).round() / 10.0;
        let used =
            (stat.total_phys.saturating_sub(stat.avail_phys) as f64 / gb * 10.0).round() / 10.0;
        Some((used, total))
    }

    pub fn read_disk_c_gb() -> Option<(f64, f64)> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        let path: Vec<u16> = OsStr::new("C:\\")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_avail: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;

        if unsafe {
            GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut free_avail,
                &mut total_bytes,
                &mut total_free,
            )
        } == 0
        {
            return None;
        }
        let gb = 1024.0 * 1024.0 * 1024.0;
        let total = (total_bytes as f64 / gb * 10.0).round() / 10.0;
        let used = (total_bytes.saturating_sub(total_free) as f64 / gb * 10.0).round() / 10.0;
        Some((used, total))
    }
}

pub fn get_system_overview_sync() -> Result<SystemOverview, String> {
    if let Ok(guard) = CACHE.lock() {
        if let Some(ref cached) = *guard {
            if cached.last_fetched.elapsed() < CACHE_TTL {
                #[cfg(windows)]
                {
                    let mut fresh = cached.data.clone();
                    if let Some(cpu) = win32::read_cpu_percent() {
                        fresh.cpu_percent = Some(cpu);
                    }
                    if let Some((used, total)) = win32::read_memory_gb() {
                        fresh.memory_used_gb = Some(used);
                        fresh.memory_total_gb = Some(total);
                    }
                    if let Ok(ms) = now_unix_ms() {
                        fresh.sampled_at_unix_ms = ms;
                    }
                    return Ok(fresh);
                }
                #[cfg(not(windows))]
                return Ok(cached.data.clone());
            }
        }
    }

    let started = Instant::now();
    let result = (|| {
        #[cfg(windows)]
        {
            let output = crate::process::hidden_command("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", OVERVIEW_SCRIPT])
                .output()
                .map_err(|e| format!("Não foi possível iniciar a coleta do Windows: {e}"))?;

            if !output.status.success() {
                return Err("A coleta do Windows falhou".to_owned());
            }

            let mut overview = parse_overview(
                std::str::from_utf8(&output.stdout)
                    .map_err(|e| format!("A coleta retornou texto inválido: {e}"))?
                    .trim(),
                now_unix_ms()?,
            )?;

            // Aplica leituras Win32 de precisão instantânea para CPU, RAM e Disco se disponíveis
            if let Some(cpu) = win32::read_cpu_percent() {
                overview.cpu_percent = Some(cpu);
            }
            if let Some((used, total)) = win32::read_memory_gb() {
                overview.memory_used_gb = Some(used);
                overview.memory_total_gb = Some(total);
            }
            if let Some((used, total)) = win32::read_disk_c_gb() {
                overview.disk_used_gb = Some(used);
                overview.disk_total_gb = Some(total);
            }
            // O card e o módulo usam o mesmo inventário cacheado (TTL de 10s),
            // evitando que o resumo mostre zero enquanto a tela detalhada mostra itens.
            if let Ok(items) = crate::startup_inventory::list_startup_items() {
                overview.startup_item_count = items.len() as u32;
            }

            Ok(overview)
        }
        #[cfg(not(windows))]
        {
            Err("A coleta de telemetria é compatível somente com Windows.".to_owned())
        }
    })();

    if let Ok(ref overview) = result {
        if let Ok(mut guard) = CACHE.lock() {
            *guard = Some(OverviewCache {
                data: overview.clone(),
                last_fetched: Instant::now(),
            });
        }
    }

    crate::debug::record(
        "system_overview",
        started.elapsed().as_millis(),
        result.as_ref().map(|_| ()).map_err(|_| "Falha na coleta"),
    );

    result
}

#[tauri::command]
pub async fn get_system_overview() -> Result<SystemOverview, String> {
    tauri::async_runtime::spawn_blocking(get_system_overview_sync)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn terminate_process(id: u32) -> Result<(), String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || terminate_process_sync(id))
        .await
        .map_err(|e| e.to_string())?;
    crate::debug::record(
        "terminate_process",
        started.elapsed().as_millis(),
        result
            .as_ref()
            .map(|_| ())
            .map_err(|_| "Ação recusada ou falhou"),
    );
    result
}

fn terminate_process_sync(id: u32) -> Result<(), String> {
    if id == 0 || id == std::process::id() {
        return Err("O Pulse não pode encerrar este processo.".to_owned());
    }

    let process = get_system_overview_sync()?
        .top_processes
        .into_iter()
        .find(|process| process.id == id)
        .ok_or_else(|| "O processo não pertence mais à coleta atual.".to_owned())?;
    let process_ids = if process.process_ids.is_empty() {
        vec![process.id]
    } else {
        process.process_ids.clone()
    };

    if process_ids.iter().any(|process_id| *process_id == 0 || *process_id == std::process::id()) {
        return Err("O Pulse não pode encerrar este processo.".to_owned());
    }

    #[cfg(windows)]
    {
        let display_name = process.display_name.as_deref().unwrap_or(&process.name);

        if is_wsl_host_process(&process.name) {
            let output = crate::process::hidden_command("wsl.exe")
                .arg("--shutdown")
                .output()
                .map_err(|error| {
                    format!("Não foi possível solicitar o desligamento do WSL: {error}")
                })?;

            if !output.status.success() {
                return Err(command_failure_detail(
                    &output,
                    "O WSL não permitiu desligar a máquina virtual.",
                ));
            }
        } else {
            let mut graceful_args = vec!["/PID".to_owned()];
            for (index, process_id) in process_ids.iter().enumerate() {
                if index > 0 {
                    graceful_args.push("/PID".to_owned());
                }
                graceful_args.push(process_id.to_string());
            }
            let graceful = crate::process::hidden_command("taskkill.exe")
                .args(&graceful_args)
                .output()
                .map_err(|error| format!("Não foi possível solicitar o encerramento: {error}"))?;

            if !graceful.status.success() {
                if !requires_forced_termination(&graceful.stderr) {
                    return Err(command_failure_detail(
                        &graceful,
                        &format!("O Windows não permitiu encerrar {display_name}."),
                    ));
                }

                let mut forced_args = vec!["/F".to_owned()];
                forced_args.extend(graceful_args.iter().cloned());
                let forced = crate::process::hidden_command("taskkill.exe")
                    .args(&forced_args)
                    .output()
                    .map_err(|error| format!("Não foi possível forçar o encerramento: {error}"))?;

                if !forced.status.success() {
                    return Err(command_failure_detail(
                        &forced,
                        &format!("O Windows não permitiu encerrar {display_name}."),
                    ));
                }
            }
        }

        if let Ok(mut guard) = CACHE.lock() {
            *guard = None;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    Err(format!(
        "Encerramento de processos só está disponível no Windows (PID {}).",
        process_ids[0]
    ))
}

fn is_wsl_host_process(name: &str) -> bool {
    name.trim_end_matches(".exe")
        .eq_ignore_ascii_case("vmmemWSL")
}

fn requires_forced_termination(stderr: &[u8]) -> bool {
    let detail = String::from_utf8_lossy(stderr).to_lowercase();
    detail.contains("can only be terminated forcefully")
        || detail.contains("só pode ser encerrado à força")
        || detail.contains("só pode ser terminado à força")
        || detail.contains("somente pode ser encerrado à força")
        || detail.contains("somente pode ser terminado à força")
}

#[cfg(windows)]
fn command_failure_detail(output: &std::process::Output, fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        fallback.to_owned()
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_wsl_host_process, parse_overview, requires_forced_termination, terminate_process_sync,
        OVERVIEW_SCRIPT,
    };

    #[test]
    fn process_termination_rejects_invalid_and_own_pid() {
        assert!(terminate_process_sync(0).is_err());
        assert!(terminate_process_sync(std::process::id()).is_err());
    }

    #[test]
    fn recognizes_only_the_wsl_virtual_machine_host() {
        assert!(is_wsl_host_process("vmmemWSL"));
        assert!(is_wsl_host_process("VMMEMWSL.exe"));
        assert!(!is_wsl_host_process("vmmem"));
        assert!(!is_wsl_host_process("wsl"));
    }

    #[test]
    fn forces_only_when_windows_explicitly_requires_it() {
        assert!(requires_forced_termination(
            b"This process can only be terminated forcefully (with /F option)."
        ));
        assert!(requires_forced_termination(
            "Este processo só pode ser encerrado à força.".as_bytes()
        ));
        assert!(!requires_forced_termination(b"Access is denied."));
    }

    #[test]
    fn overview_is_capped_at_twenty_five_processes() {
        let processes = (0..26)
            .map(|i| format!(r#"{{"name":"p{i}","id":{i},"memoryMb":1.0}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let payload = format!(
            r#"{{"cpuPercent":1,"memoryUsedGb":1,"memoryTotalGb":2,"diskUsedGb":3,"diskTotalGb":4,"startupItemCount":0,"topProcesses":[{processes}]}}"#
        );
        assert_eq!(
            parse_overview(&payload, 123).unwrap().top_processes.len(),
            25
        );
    }

    #[test]
    fn overview_preserves_optional_process_identity() {
        let payload = r#"{"cpuPercent":1,"memoryUsedGb":1,"memoryTotalGb":2,"diskUsedGb":3,"diskTotalGb":4,"startupItemCount":2,"topProcesses":[{"name":"code","id":42,"memoryMb":512.0,"displayName":"Visual Studio Code","publisher":"Microsoft Corporation"}]}"#;
        let process = &parse_overview(payload, 123).unwrap().top_processes[0];
        assert_eq!(process.display_name.as_deref(), Some("Visual Studio Code"));
        assert_eq!(process.publisher.as_deref(), Some("Microsoft Corporation"));
    }

    #[test]
    fn overview_preserves_grouped_process_members() {
        let payload = r#"{"cpuPercent":1,"memoryUsedGb":1,"memoryTotalGb":2,"diskUsedGb":3,"diskTotalGb":4,"startupItemCount":0,"topProcesses":[{"name":"codex","id":42,"processIds":[42,43,44],"processCount":3,"memoryMb":1200.0}]}"#;
        let process = &parse_overview(payload, 123).unwrap().top_processes[0];
        assert_eq!(process.process_ids, vec![42, 43, 44]);
        assert_eq!(process.process_count, 3);
    }

    #[test]
    fn overview_attributes_webview2_to_its_owner_process() {
        assert!(OVERVIEW_SCRIPT.contains("$parentById"));
        assert!(OVERVIEW_SCRIPT.contains("msedgewebview2"));
        assert!(OVERVIEW_SCRIPT.contains("$groupKey = $processById[$ownerId].ProcessName"));
        assert!(OVERVIEW_SCRIPT.contains("$_.ProcessName.ToLowerInvariant() -ne \"msedgewebview2\""));
        assert!(!OVERVIEW_SCRIPT.contains("Group-Object { $_.ProcessName.ToLowerInvariant() }"));
    }
}
