use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TopProcess {
    pub name: String,
    pub id: u32,
    pub memory_mb: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemOverview {
    pub cpu_percent: f64,
    pub memory_used_gb: f64,
    pub memory_total_gb: f64,
    pub disk_used_gb: f64,
    pub disk_total_gb: f64,
    pub startup_item_count: u32,
    pub top_processes: Vec<TopProcess>,
    pub sampled_at_unix_ms: u64,
    pub source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSystemOverview {
    cpu_percent: f64,
    memory_used_gb: f64,
    memory_total_gb: f64,
    disk_used_gb: f64,
    disk_total_gb: f64,
    startup_item_count: u32,
    top_processes: Vec<TopProcess>,
}

const OVERVIEW_SCRIPT: &str = r#"
$os = Get-CimInstance Win32_OperatingSystem
$cpu = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average
$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
$startup = @(Get-CimInstance Win32_StartupCommand).Count
$top = @(Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 15 @{N='name';E={$_.ProcessName}}, @{N='id';E={$_.Id}}, @{N='memoryMb';E={[math]::Round($_.WorkingSet64 / 1MB, 1)}})
$self = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {$_.ProcessName -like 'pulse*'} | Select-Object @{N='name';E={$_.ProcessName}}, @{N='id';E={$_.Id}}, @{N='memoryMb';E={[math]::Round($_.WorkingSet64 / 1MB, 1)}})
$processes = @($self + $top | Group-Object id | ForEach-Object {$_.Group[0]} | Sort-Object memoryMb -Descending | Select-Object -First 15)
@{
  cpuPercent = [math]::Round([double]$cpu, 1)
  memoryUsedGb = [math]::Round((($os.TotalVisibleMemorySize - $os.FreePhysicalMemory) / 1MB), 1)
  memoryTotalGb = [math]::Round(($os.TotalVisibleMemorySize / 1MB), 1)
  diskUsedGb = [math]::Round((($disk.Size - $disk.FreeSpace) / 1GB), 1)
  diskTotalGb = [math]::Round(($disk.Size / 1GB), 1)
  startupItemCount = [int]$startup
  topProcesses = $processes
} | ConvertTo-Json -Compress -Depth 3
"#;

fn now_unix_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|e| format!("Relógio do sistema inválido: {e}"))
}

fn parse_overview(payload: &str, sampled_at_unix_ms: u64) -> Result<SystemOverview, String> {
    let raw: RawSystemOverview = serde_json::from_str(payload)
        .map_err(|e| format!("Não foi possível interpretar o resumo do Windows: {e}"))?;
    Ok(SystemOverview {
        cpu_percent: raw.cpu_percent,
        memory_used_gb: raw.memory_used_gb,
        memory_total_gb: raw.memory_total_gb,
        disk_used_gb: raw.disk_used_gb,
        disk_total_gb: raw.disk_total_gb,
        startup_item_count: raw.startup_item_count,
        top_processes: raw.top_processes.into_iter().take(15).collect(),
        sampled_at_unix_ms,
        source: "Windows CIM + Process API".to_owned(),
    })
}

#[tauri::command]
pub fn get_system_overview() -> Result<SystemOverview, String> {
    let started = std::time::Instant::now();
    let result = (|| {
        let output = Command::new("powershell.exe")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["-NoProfile", "-NonInteractive", "-Command", OVERVIEW_SCRIPT])
            .output()
            .map_err(|e| format!("Não foi possível iniciar a coleta do Windows: {e}"))?;
        if !output.status.success() {
            return Err("A coleta do Windows falhou".to_owned());
        }
        parse_overview(
            std::str::from_utf8(&output.stdout)
                .map_err(|e| format!("A coleta retornou texto inválido: {e}"))?
                .trim(),
            now_unix_ms()?,
        )
    })();
    crate::debug::record(
        "system_overview",
        started.elapsed().as_millis(),
        result
            .as_ref()
            .map(|_| ())
            .map_err(|_| "Falha na coleta de telemetria"),
    );
    result
}

#[cfg(test)]
mod tests {
    use super::parse_overview;
    #[test]
    fn overview_is_capped_at_fifteen_processes() {
        let processes = (0..16)
            .map(|i| format!(r#"{{"name":"p{i}","id":{i},"memoryMb":1.0}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let payload = format!(
            r#"{{"cpuPercent":1,"memoryUsedGb":1,"memoryTotalGb":2,"diskUsedGb":3,"diskTotalGb":4,"startupItemCount":0,"topProcesses":[{processes}]}}"#
        );
        assert_eq!(
            parse_overview(&payload, 123).unwrap().top_processes.len(),
            15
        );
    }
}
