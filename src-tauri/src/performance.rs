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
    // exibir uma capacidade incorreta. Temperatura também não pertence a esta
    // classe WMI; ela exige um sensor exposto pelo driver.
    let vram_bytes = Capability {
        value: None,
        unavailable_reason: Some(
            "VRAM requer leitura DXGI; o valor WMI pode ser impreciso neste hardware".into(),
        ),
    };
    let temperature_celsius = Capability {
        value: None,
        unavailable_reason: Some("O driver não expôs um sensor de temperatura compatível".into()),
    };

    PerformanceDetails {
        gpu,
        vram_bytes,
        temperature_celsius,
    }
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
    use super::Capability;
    #[test]
    fn unavailable_capability_has_reason() {
        let c: Capability<u64> = Capability {
            value: None,
            unavailable_reason: Some("driver absent".into()),
        };
        assert!(c.value.is_none());
        assert_eq!(c.unavailable_reason.as_deref(), Some("driver absent"));
    }
}
