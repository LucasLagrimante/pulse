use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Capability<T> {
    pub value: Option<T>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceDetails {
    pub gpu: Capability<String>,
    pub vram_bytes: Capability<u64>,
    pub temperature_celsius: Capability<f64>,
}

pub fn get_performance_details_sync() -> PerformanceDetails {
    let script = r#"
$gpus = @(Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike '*Basic Display*' -and $_.Name -notlike '*RDP*' })
if ($gpus.Count -eq 0) { $gpus = @(Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue) }
$best = $gpus | Sort-Object { [int64]$_.AdapterRAM } -Descending | Select-Object -First 1
if ($best) {
  @{
    Name = [string]$best.Name
  } | ConvertTo-Json -Compress
} else {
  '{}'
}
"#;
    let result = crate::process::hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output();
    let Ok(output) = result else {
        return unavailable("PowerShell não está disponível");
    };
    if !output.status.success() {
        return unavailable("O driver de vídeo não expôs telemetria via WMI");
    }
    let Ok(text) = std::str::from_utf8(&output.stdout) else {
        return unavailable("A resposta do driver não é texto válido");
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return unavailable("O driver não retornou dados de vídeo");
    };
    let gpu = value
        .get("Name")
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
        });
    // Win32_VideoController.AdapterRAM é um campo WMI legado de 32 bits e pode
    // truncar GPUs modernas. Sem um provedor DXGI dedicado, é mais honesto não
    // exibir uma capacidade incorreta.
    let vram_bytes = Capability {
        value: None,
        unavailable_reason: Some(
            "VRAM requer leitura DXGI; o valor WMI pode ser impreciso neste hardware".into(),
        ),
    };
    let temperature_celsius = read_temperature();

    PerformanceDetails {
        gpu,
        vram_bytes,
        temperature_celsius,
    }
}

/// Tenta as fontes reais de temperatura em ordem de especificidade:
/// 1. `nvidia-smi` (sensor direto da GPU NVIDIA, ex.: 67 °C na RTX 4070 SUPER);
/// 2. `MSAcpi_ThermalZoneTemperature` via WMI (zonas térmicas ACPI — CPU/placa).
/// Win32_VideoController nunca expôs temperatura, por isso era sempre
/// "indisponível" antes desta correção.
fn read_temperature() -> Capability<f64> {
    if let Some(gpu_temp) = read_gpu_temperature_nvidia_smi() {
        return Capability {
            value: Some(gpu_temp),
            unavailable_reason: None,
        };
    }
    if let Some(zone_temp) = read_thermal_zone_temperature() {
        return Capability {
            value: Some(zone_temp),
            unavailable_reason: None,
        };
    }
    Capability {
        value: None,
        unavailable_reason: Some(
            "Nenhum sensor térmico respondeu (nvidia-smi ausente e sem zonas ACPI ativas)".into(),
        ),
    }
}

/// Lê a temperatura da GPU via `nvidia-smi`. Suporta múltiplas GPUs
/// (retorna a mais quente) e ignora saídas como "N/A" ou "[Not Supported]".
fn read_gpu_temperature_nvidia_smi() -> Option<f64> {
    let output = crate::process::hidden_command("nvidia-smi")
        .args([
            "--query-gpu=temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    parse_nvidia_smi_temperature(text)
}

/// Extrai a maior temperatura válida (0–150 °C) da saída CSV do `nvidia-smi`.
/// Função pura para permitir testes unitários fora do Windows.
fn parse_nvidia_smi_temperature(text: &str) -> Option<f64> {
    let mut hottest: Option<f64> = None;
    for token in text.split([',', '\n', '\r'].as_slice()) {
        let cleaned: String = token
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        if cleaned.is_empty() {
            continue;
        }
        if let Ok(value) = cleaned.parse::<f64>() {
            if value.is_finite() && (0.0..=150.0).contains(&value) {
                let rounded = (value * 10.0).round() / 10.0;
                hottest = Some(hottest.map_or(rounded, |best: f64| best.max(rounded)));
            }
        }
    }
    hottest
}

/// Lê as zonas térmicas ACPI (`root/wmi\MSAcpi_ThermalZoneTemperature`).
/// `CurrentTemperature` vem em décimos de Kelvin: °C = valor / 10 − 273.15.
fn read_thermal_zone_temperature() -> Option<f64> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$zones = @(Get-CimInstance MSAcpi_ThermalZoneTemperature -Namespace root/wmi -ErrorAction SilentlyContinue | Select-Object CurrentTemperature, Active)
ConvertTo-Json -InputObject @($zones) -Compress
"#;
    let output = crate::process::hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    parse_thermal_zone_temperature(&value)
}

/// Converte o JSON das zonas térmicas na maior temperatura válida.
/// Zonas com `Active: false` são ignoradas; sem `Active`, assume-se ativa.
/// Função pura para permitir testes unitários fora do Windows.
fn parse_thermal_zone_temperature(value: &serde_json::Value) -> Option<f64> {
    let items: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(_) => vec![value],
        _ => Vec::new(),
    };
    let mut hottest: Option<f64> = None;
    for item in items {
        if item.get("Active").and_then(|v| v.as_bool()) == Some(false) {
            continue;
        }
        let Some(raw) = item
            .get("CurrentTemperature")
            .and_then(|v| v.as_u64())
            .or_else(|| {
                item
                    .get("CurrentTemperature")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as u64)
            })
        else {
            continue;
        };
        // Faixa plausível: 0 °C (2732 décimos-K) a 150 °C (4232 décimos-K).
        if !(2732..=4232).contains(&raw) {
            continue;
        }
        let celsius = (raw as f64 / 10.0) - 273.15;
        let rounded = (celsius * 10.0).round() / 10.0;
        if (0.0..=150.0).contains(&rounded) {
            hottest = Some(hottest.map_or(rounded, |best: f64| best.max(rounded)));
        }
    }
    hottest
}

#[tauri::command]
pub async fn get_performance_details() -> PerformanceDetails {
    let started = Instant::now();
    let details = tauri::async_runtime::spawn_blocking(get_performance_details_sync)
        .await
        .unwrap_or_else(|_| unavailable("Falha na execução em segundo plano"));
    crate::debug::record("performance_details", started.elapsed().as_millis(), Ok(()));
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
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_nvidia_smi_temperature, parse_thermal_zone_temperature, Capability,
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
    fn parses_single_gpu_temperature() {
        assert_eq!(parse_nvidia_smi_temperature("67\n"), Some(67.0));
    }

    #[test]
    fn parses_hottest_of_multiple_gpus() {
        assert_eq!(parse_nvidia_smi_temperature("54\n71\n"), Some(71.0));
        assert_eq!(parse_nvidia_smi_temperature("54, 71"), Some(71.0));
    }

    #[test]
    fn ignores_unsupported_nvidia_smi_output() {
        assert_eq!(parse_nvidia_smi_temperature("[Not Supported]\n"), None);
        assert_eq!(parse_nvidia_smi_temperature("N/A\n"), None);
        assert_eq!(parse_nvidia_smi_temperature(""), None);
    }

    #[test]
    fn rejects_out_of_range_gpu_temperatures() {
        assert_eq!(parse_nvidia_smi_temperature("-5\n"), None);
        assert_eq!(parse_nvidia_smi_temperature("300\n"), None);
    }

    #[test]
    fn parses_thermal_zone_decikelvin() {
        // 3000 décimos-K = 300 K = 26.9 °C (arredondado a 1 casa).
        let value = serde_json::json!([{ "CurrentTemperature": 3000, "Active": true }]);
        assert_eq!(parse_thermal_zone_temperature(&value), Some(26.9));
    }

    #[test]
    fn parses_thermal_zone_single_object() {
        let value = serde_json::json!({ "CurrentTemperature": 3100, "Active": true });
        // 310 K = 36.9 °C (36.85 arredonda para 36.9).
        assert_eq!(parse_thermal_zone_temperature(&value), Some(36.9));
    }

    #[test]
    fn ignores_inactive_and_implausible_zones() {
        let value = serde_json::json!([
            { "CurrentTemperature": 3000, "Active": false },
            { "CurrentTemperature": 100, "Active": true },
        ]);
        assert_eq!(parse_thermal_zone_temperature(&value), None);
        let empty = serde_json::json!([]);
        assert_eq!(parse_thermal_zone_temperature(&empty), None);
    }

    #[test]
    fn picks_hottest_active_zone() {
        let value = serde_json::json!([
            { "CurrentTemperature": 3000, "Active": true },
            { "CurrentTemperature": 3200 },
        ]);
        // 320 K = 46.9 °C (46.85 arredonda para 46.9).
        assert_eq!(parse_thermal_zone_temperature(&value), Some(46.9));
    }
}
