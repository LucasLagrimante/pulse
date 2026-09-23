use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Capability<T> {
    pub value: Option<T>,
    pub unavailable_reason: Option<String>,
}

/// Um sensor térmico individual com rótulo honesto sobre a origem.
/// A CPU só aparece aqui quando algum provedor a identifica (ex.: sensor
/// "CPU Package" do LibreHardwareMonitor); zonas ACPI genéricas mantêm o
/// nome da zona para não fingir uma medição que não existe.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThermalSensor {
    pub id: String,
    pub label: String,
    pub celsius: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceDetails {
    pub gpu: Capability<String>,
    pub vram_bytes: Capability<u64>,
    pub temperature_celsius: Capability<f64>,
    pub thermal_sensors: Vec<ThermalSensor>,
}

pub fn get_performance_details_sync() -> PerformanceDetails {
    // Um único PowerShell por coleta: o cold start do `powershell.exe`
    // (centenas de ms) domina o custo, então nome da GPU e todas as fontes
    // térmicas de firmware vêm no mesmo snapshot. Antes eram duas chamadas
    // por coleta — metade do custo some aqui.
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$gpus = @(Get-CimInstance Win32_VideoController | Where-Object { $_.Name -notlike '*Basic Display*' -and $_.Name -notlike '*RDP*' })
if ($gpus.Count -eq 0) { $gpus = @(Get-CimInstance Win32_VideoController) }
$best = $gpus | Sort-Object { [int64]$_.AdapterRAM } -Descending | Select-Object -First 1 | Select-Object Name
$zones = @(Get-CimInstance MSAcpi_ThermalZoneTemperature -Namespace root/wmi | Select-Object InstanceName, CurrentTemperature, Active)
$lhm = @(Get-CimInstance -Namespace root/LibreHardwareMonitor -ClassName Sensor | Where-Object { $_.SensorType -eq 'Temperature' } | Select-Object Name, Identifier, Value)
$ohm = @(Get-CimInstance -Namespace root/OpenHardwareMonitor -ClassName Sensor | Where-Object { $_.SensorType -eq 'Temperature' } | Select-Object Name, Identifier, Value)
$probes = @(Get-CimInstance Win32_TemperatureProbe | Select-Object Name, CurrentReading, Status)
@{ gpu = $best; zones = @($zones); lhm = @($lhm); ohm = @($ohm); probes = @($probes) } | ConvertTo-Json -Compress -Depth 4
"#;
    let snapshot = run_wmi_snapshot(script);
    let gpu = match &snapshot {
        Ok(value) => parse_gpu_name(value),
        Err(reason) => Capability {
            value: None,
            unavailable_reason: Some((*reason).to_owned()),
        },
    };
    // Win32_VideoController.AdapterRAM é um campo WMI legado de 32 bits e pode
    // truncar GPUs modernas. Sem um provedor DXGI dedicado, é mais honesto não
    // exibir uma capacidade incorreta.
    let vram_bytes = Capability {
        value: None,
        unavailable_reason: Some(
            "VRAM requer leitura DXGI; o valor WMI pode ser impreciso neste hardware".into(),
        ),
    };
    let mut thermal_sensors = read_nvidia_gpu_sensors();
    if let Ok(value) = &snapshot {
        thermal_sensors.extend(parse_snapshot_sensors(value));
    }
    thermal_sensors.truncate(16);
    let temperature_celsius = hottest_sensor(&thermal_sensors);

    PerformanceDetails {
        gpu,
        vram_bytes,
        temperature_celsius,
        thermal_sensors,
    }
}

/// Executa o snapshot WMI num único `powershell.exe`. Namespaces ausentes
/// (LHM/OHM só existem com o app em execução) viram listas vazias sem erro;
/// `Err` só quando o próprio PowerShell falha.
fn run_wmi_snapshot(script: &str) -> Result<serde_json::Value, &'static str> {
    let output = crate::process::hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|_| "PowerShell não está disponível")?;
    if !output.status.success() {
        return Err("O driver de vídeo não expôs telemetria via WMI");
    }
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| "A resposta do driver não é texto válido")?;
    serde_json::from_str(text.trim()).map_err(|_| "O driver não retornou dados de vídeo")
}

/// Extrai o nome da GPU do snapshot (`gpu` é `null` sem placa válida).
/// Função pura para permitir testes unitários fora do Windows.
fn parse_gpu_name(value: &serde_json::Value) -> Capability<String> {
    value
        .get("gpu")
        .and_then(|gpu| gpu.get("Name"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .map(|v| Capability {
            value: Some(v),
            unavailable_reason: None,
        })
        .unwrap_or_else(|| Capability {
            value: None,
            unavailable_reason: Some("Nome da GPU não exposto pelo driver".into()),
        })
}

/// Maior temperatura entre os sensores, para compatibilidade com a UI que
/// exibe um valor único.
fn hottest_sensor(sensors: &[ThermalSensor]) -> Capability<f64> {
    sensors
        .iter()
        .map(|sensor| sensor.celsius)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|celsius| Capability {
            value: Some(celsius),
            unavailable_reason: None,
        })
        .unwrap_or_else(|| Capability {
            value: None,
            unavailable_reason: Some(
                "Nenhum sensor térmico respondeu (nvidia-smi ausente, sem zonas ACPI ativas e LibreHardwareMonitor não detectado)".into(),
            ),
        })
}

/// Valida e arredonda (1 casa) uma temperatura em °C.
/// Função pura para permitir testes unitários fora do Windows.
fn valid_celsius(value: f64) -> Option<f64> {
    if value.is_finite() && (0.0..=150.0).contains(&value) {
        Some((value * 10.0).round() / 10.0)
    } else {
        None
    }
}

/// Lê um sensor por GPU via `nvidia-smi`, ignorando linhas como "N/A" ou
/// "[Not Supported]".
fn read_nvidia_gpu_sensors() -> Vec<ThermalSensor> {
    let output = crate::process::hidden_command("nvidia-smi")
        .args([
            "--query-gpu=index,name,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success());
    let Some(output) = output else {
        return Vec::new();
    };
    let text = std::str::from_utf8(&output.stdout).unwrap_or("");
    parse_nvidia_smi_sensors(text)
}

/// Extrai `(índice, nome, °C)` de cada linha CSV do `nvidia-smi`
/// (`0, NVIDIA GeForce RTX 4070 SUPER, 67`).
/// Função pura para permitir testes unitários fora do Windows.
fn parse_nvidia_smi_sensors(text: &str) -> Vec<ThermalSensor> {
    let mut sensors = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() < 2 {
            continue;
        }
        let index = fields[0]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>();
        let temperature = fields[fields.len() - 1]
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect::<String>()
            .parse::<f64>()
            .ok()
            .and_then(valid_celsius);
        let Some(celsius) = temperature else {
            continue;
        };
        let name = fields[1..fields.len() - 1].join(",").trim().to_owned();
        if index.is_empty() && name.is_empty() {
            continue;
        }
        let label = if name.is_empty() {
            format!("GPU {index}")
        } else {
            name
        };
        sensors.push(ThermalSensor {
            id: format!("gpu-{index}"),
            label,
            celsius,
            source: "nvidia-smi".into(),
        });
    }
    sensors
}

/// Converte o snapshot `{ gpu, zones, lhm, ohm, probes }` em sensores.
/// `CurrentTemperature` das zonas e `CurrentReading` das sondas vêm em
/// décimos de Kelvin (°C = valor / 10 − 273.15); `Value` do LHM/OHM já vem
/// em °C. Zonas com `Active: false` são ignoradas; sem `Active`, ativa.
/// Função pura para permitir testes unitários fora do Windows.
fn parse_snapshot_sensors(value: &serde_json::Value) -> Vec<ThermalSensor> {
    let mut sensors = Vec::new();
    let items = |key: &str| -> Vec<&serde_json::Value> {
        match value.get(key) {
            Some(serde_json::Value::Array(items)) => items.iter().collect(),
            Some(single @ serde_json::Value::Object(_)) => vec![single],
            _ => Vec::new(),
        }
    };
    // LibreHardwareMonitor e OpenHardwareMonitor (legado) expõem o mesmo
    // esquema `Sensor`; ambos só existem com o app correspondente em execução.
    for (key, source) in [("lhm", "LibreHardwareMonitor"), ("ohm", "OpenHardwareMonitor")] {
        let prefix = if key == "ohm" { "ohm" } else { "lhm" };
        for item in items(key) {
            let name = item.get("Name").and_then(|v| v.as_str()).unwrap_or("");
            let identifier = item.get("Identifier").and_then(|v| v.as_str()).unwrap_or("");
            let celsius = item.get("Value").and_then(|v| v.as_f64()).and_then(valid_celsius);
            if let Some(celsius) = celsius.filter(|_| !name.is_empty()) {
                sensors.push(ThermalSensor {
                    id: format!("{prefix}-{}", sanitize_id(identifier)),
                    label: name.to_owned(),
                    celsius,
                    source: source.into(),
                });
            }
        }
    }
    for item in items("zones") {
        if item.get("Active").and_then(|v| v.as_bool()) == Some(false) {
            continue;
        }
        let Some(celsius) =
            raw_decikelvin(item, "CurrentTemperature").and_then(decikelvin_to_celsius)
        else {
            continue;
        };
        let raw_name = item
            .get("InstanceName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .rsplit('\\')
            .next()
            .unwrap_or("");
        // Remove só o sufixo de instância (`TZ00_0` → `TZ00`).
        let zone = match raw_name.rsplit_once('_') {
            Some((base, suffix))
                if !base.is_empty() && !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) =>
            {
                base
            }
            _ => raw_name,
        };
        let zone = if zone.is_empty() { "desconhecida" } else { zone };
        sensors.push(ThermalSensor {
            id: format!("acpi-{}", sanitize_id(zone)),
            label: format!("Zona térmica {zone}"),
            celsius,
            source: "ACPI".into(),
        });
    }
    // Win32_TemperatureProbe quase nunca é implementada pelos fabricantes,
    // mas quando existe (sonda exposta pela placa-mãe) a leitura sai de graça
    // no mesmo snapshot — custo marginal zero.
    for item in items("probes") {
        if let Some(status) = item.get("Status").and_then(|v| v.as_str()) {
            if status.to_lowercase().contains("error") {
                continue;
            }
        }
        let Some(celsius) =
            raw_decikelvin(item, "CurrentReading").and_then(decikelvin_to_celsius)
        else {
            continue;
        };
        let name = item
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let label = if name.is_empty() {
            "Sonda térmica".to_owned()
        } else {
            name.to_owned()
        };
        sensors.push(ThermalSensor {
            id: format!("probe-{}", sanitize_id(&label)),
            label,
            celsius,
            source: "WMI".into(),
        });
    }
    sensors
}

/// Lê um inteiro de décimos de Kelvin tolerando número ou string numérica.
fn raw_decikelvin(item: &serde_json::Value, key: &str) -> Option<u64> {
    item.get(key)
        .and_then(|v| v.as_u64())
        .or_else(|| {
            item
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|v| v as u64)
        })
}

/// Décimos de Kelvin para °C, só na faixa plausível 0–150 °C
/// (bruto 2732–4232). Função pura para testes fora do Windows.
fn decikelvin_to_celsius(raw: u64) -> Option<f64> {
    if !(2732..=4232).contains(&raw) {
        return None;
    }
    valid_celsius((raw as f64 / 10.0) - 273.15)
}

fn sanitize_id(raw: &str) -> String {
    let cleaned: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let collapsed = cleaned.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
    if collapsed.is_empty() {
        "sensor".into()
    } else {
        collapsed
    }
}

#[tauri::command]
pub async fn get_performance_details(app: tauri::AppHandle) -> PerformanceDetails {
    let started = Instant::now();
    let details = tauri::async_runtime::spawn_blocking(get_performance_details_sync)
        .await
        .unwrap_or_else(|_| unavailable("Falha na execução em segundo plano"));
    crate::debug::record("performance_details", started.elapsed().as_millis(), Ok(()));
    // Tooltip do tray de carona na leitura que a UI já pediu: zero
    // subprocesso extra, só reaproveita os sensores recém-coletados.
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(crate::tray::tray_tooltip_for_sensors(
            &details.thermal_sensors,
        )));
    }
    details
}

fn unavailable(reason: &str) -> PerformanceDetails {
    PerformanceDetails {
        gpu: Capability {
            value: None,
            unavailable_reason: Some(reason.to_owned()),
        },
        vram_bytes: Capability {
            value: None,
            unavailable_reason: Some(reason.to_owned()),
        },
        temperature_celsius: Capability {
            value: None,
            unavailable_reason: Some(reason.to_owned()),
        },
        thermal_sensors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decikelvin_to_celsius, hottest_sensor, parse_gpu_name, parse_nvidia_smi_sensors,
        parse_snapshot_sensors, raw_decikelvin, sanitize_id, valid_celsius, Capability,
        ThermalSensor,
    };

    #[test]
    fn unavailable_capability_has_reason() {
        let c: Capability<u64> = Capability {
            value: None,
            unavailable_reason: Some("driver absent".into()),
        };
        assert!(c.value.is_none());
        assert_eq!(c.unavailable_reason.as_deref(), Some("driver absent"));
    }

    #[test]
    fn parses_named_gpu_sensors() {
        let sensors = parse_nvidia_smi_sensors("0, NVIDIA GeForce RTX 4070 SUPER, 67\n");
        assert_eq!(
            sensors,
            vec![ThermalSensor {
                id: "gpu-0".into(),
                label: "NVIDIA GeForce RTX 4070 SUPER".into(),
                celsius: 67.0,
                source: "nvidia-smi".into(),
            }]
        );
    }

    #[test]
    fn ignores_unsupported_gpu_lines() {
        let sensors = parse_nvidia_smi_sensors("0, NVIDIA GeForce RTX 4070 SUPER, [Not Supported]\n1, Other GPU, N/A\n");
        assert!(sensors.is_empty());
        assert!(parse_nvidia_smi_sensors("").is_empty());
    }

    #[test]
    fn rejects_out_of_range_gpu_temperatures() {
        assert!(parse_nvidia_smi_sensors("0, GPU, -5\n").is_empty());
        assert!(parse_nvidia_smi_sensors("0, GPU, 300\n").is_empty());
    }

    #[test]
    fn parses_lhm_cpu_package_sensor() {
        let value = serde_json::json!({
            "zones": [],
            "lhm": [{ "Name": "CPU Package", "Identifier": "/intelcpu/0/temperature/0", "Value": 54.25 }],
        });
        assert_eq!(
            parse_snapshot_sensors(&value),
            vec![ThermalSensor {
                id: "lhm-intelcpu-0-temperature-0".into(),
                label: "CPU Package".into(),
                celsius: 54.3,
                source: "LibreHardwareMonitor".into(),
            }]
        );
    }

    #[test]
    fn parses_legacy_openhardwaremonitor_sensors() {
        let value = serde_json::json!({
            "zones": [],
            "lhm": [],
            "ohm": [{ "Name": "CPU Core #1", "Identifier": "/intelcpu/0/temperature/1", "Value": 48.0 }],
        });
        assert_eq!(
            parse_snapshot_sensors(&value),
            vec![ThermalSensor {
                id: "ohm-intelcpu-0-temperature-1".into(),
                label: "CPU Core #1".into(),
                celsius: 48.0,
                source: "OpenHardwareMonitor".into(),
            }]
        );
    }

    #[test]
    fn parses_named_thermal_zones() {
        // 3000 décimos-K = 300 K = 26.9 °C (arredondado a 1 casa).
        let value = serde_json::json!({
            "zones": [{ "InstanceName": "ACPI\\ThermalZone\\TZ00_0", "CurrentTemperature": 3000, "Active": true }],
            "lhm": [],
        });
        assert_eq!(
            parse_snapshot_sensors(&value),
            vec![ThermalSensor {
                id: "acpi-tz00".into(),
                label: "Zona térmica TZ00".into(),
                celsius: 26.9,
                source: "ACPI".into(),
            }]
        );
    }

    #[test]
    fn ignores_inactive_and_implausible_zones() {
        let value = serde_json::json!({
            "zones": [
                { "InstanceName": "ACPI\\ThermalZone\\TZ00_0", "CurrentTemperature": 3000, "Active": false },
                { "InstanceName": "ACPI\\ThermalZone\\TZ01_0", "CurrentTemperature": 100, "Active": true },
            ],
            "lhm": [],
        });
        assert!(parse_snapshot_sensors(&value).is_empty());
        let empty = serde_json::json!({ "zones": [], "lhm": [] });
        assert!(parse_snapshot_sensors(&empty).is_empty());
    }

    #[test]
    fn parses_gpu_name_from_snapshot() {
        let value = serde_json::json!({ "gpu": { "Name": "NVIDIA GeForce RTX 4070 SUPER" } });
        let gpu = parse_gpu_name(&value);
        assert_eq!(gpu.value.as_deref(), Some("NVIDIA GeForce RTX 4070 SUPER"));
        assert!(gpu.unavailable_reason.is_none());
        let missing = serde_json::json!({ "gpu": null });
        let gpu = parse_gpu_name(&missing);
        assert!(gpu.value.is_none());
        assert!(gpu.unavailable_reason.is_some());
        let empty = serde_json::json!({ "gpu": { "Name": "" } });
        assert!(parse_gpu_name(&empty).value.is_none());
    }

    #[test]
    fn parses_temperature_probes() {
        // 3182 décimos-K = 318.2 K = 45.1 °C (45.05 arredonda para 45.1).
        let value = serde_json::json!({
            "zones": [],
            "lhm": [],
            "ohm": [],
            "probes": [{ "Name": "CPU Temp", "CurrentReading": 3182, "Status": "OK" }],
        });
        assert_eq!(
            parse_snapshot_sensors(&value),
            vec![ThermalSensor {
                id: "probe-cpu-temp".into(),
                label: "CPU Temp".into(),
                celsius: 45.1,
                source: "WMI".into(),
            }]
        );
    }

    #[test]
    fn ignores_broken_probes() {
        let value = serde_json::json!({
            "probes": [
                { "Name": "Bad", "CurrentReading": 3182, "Status": "Error" },
                { "Name": "Cold", "CurrentReading": 100 },
                { "Name": "", "CurrentReading": 3182 },
            ],
        });
        let sensors = parse_snapshot_sensors(&value);
        // Com erro ou fora de faixa, some; sem nome vira "Sonda térmica".
        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].label, "Sonda térmica");
        assert_eq!(sensors[0].celsius, 45.1);
    }

    #[test]
    fn converts_decikelvin_with_plausible_range() {
        assert_eq!(decikelvin_to_celsius(3000), Some(26.9));
        assert_eq!(decikelvin_to_celsius(100), None);
        assert_eq!(decikelvin_to_celsius(5000), None);
        let item = serde_json::json!({ "CurrentReading": 3182 });
        assert_eq!(raw_decikelvin(&item, "CurrentReading"), Some(3182));
        assert_eq!(raw_decikelvin(&item, "Missing"), None);
    }

    #[test]
    fn hottest_sensor_reports_maximum() {
        let sensors = vec![
            ThermalSensor { id: "a".into(), label: "A".into(), celsius: 41.0, source: "ACPI".into() },
            ThermalSensor { id: "b".into(), label: "B".into(), celsius: 67.0, source: "nvidia-smi".into() },
        ];
        let hottest = hottest_sensor(&sensors);
        assert_eq!(hottest.value, Some(67.0));
        assert!(hottest.unavailable_reason.is_none());
        let empty = hottest_sensor(&[]);
        assert!(empty.value.is_none());
        assert!(empty.unavailable_reason.is_some());
    }

    #[test]
    fn sanitizes_sensor_ids() {
        assert_eq!(sanitize_id("/intelcpu/0/temperature/0"), "intelcpu-0-temperature-0");
        assert_eq!(sanitize_id(""), "sensor");
    }

    #[test]
    fn rejects_invalid_celsius() {
        assert_eq!(valid_celsius(67.25), Some(67.3));
        assert_eq!(valid_celsius(-1.0), None);
        assert_eq!(valid_celsius(200.0), None);
        assert_eq!(valid_celsius(f64::NAN), None);
    }
}
