use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
$processes = @(Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 3 @{N='name';E={$_.ProcessName}}, @{N='id';E={$_.Id}}, @{N='memoryMb';E={[math]::Round($_.WorkingSet64 / 1MB, 1)}})
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
        .map(|duration| duration.as_millis() as u64)
        .map_err(|error| format!("Relógio do sistema inválido: {error}"))
}

fn parse_overview(payload: &str, sampled_at_unix_ms: u64) -> Result<SystemOverview, String> {
    let raw: RawSystemOverview = serde_json::from_str(payload)
        .map_err(|error| format!("Não foi possível interpretar o resumo do Windows: {error}"))?;

    Ok(SystemOverview {
        cpu_percent: raw.cpu_percent,
        memory_used_gb: raw.memory_used_gb,
        memory_total_gb: raw.memory_total_gb,
        disk_used_gb: raw.disk_used_gb,
        disk_total_gb: raw.disk_total_gb,
        startup_item_count: raw.startup_item_count,
        top_processes: raw.top_processes,
        sampled_at_unix_ms,
        source: "Windows CIM + Process API".to_owned(),
    })
}

#[tauri::command]
pub fn get_system_overview() -> Result<SystemOverview, String> {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", OVERVIEW_SCRIPT])
        .output()
        .map_err(|error| format!("Não foi possível iniciar a coleta do Windows: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "A coleta do Windows falhou: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let payload = String::from_utf8(output.stdout)
        .map_err(|error| format!("A coleta retornou texto inválido: {error}"))?;
    parse_overview(payload.trim(), now_unix_ms()?)
}

#[cfg(test)]
mod tests {
    use super::parse_overview;

    #[test]
    fn parsed_overview_preserves_live_windows_metrics() {
        let payload = r#"{"cpuPercent":13.2,"memoryUsedGb":10.5,"memoryTotalGb":31.9,"diskUsedGb":120.1,"diskTotalGb":475.8,"startupItemCount":7,"topProcesses":[{"name":"Code","id":42,"memoryMb":512.4}]}"#;

        let overview = parse_overview(payload, 123).expect("valid Windows payload");

        assert_eq!(overview.cpu_percent, 13.2);
        assert_eq!(overview.startup_item_count, 7);
        assert_eq!(overview.top_processes[0].name, "Code");
        assert_eq!(overview.source, "Windows CIM + Process API");
    }
}
