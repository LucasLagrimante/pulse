use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

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

#[tauri::command]
pub fn get_performance_details() -> PerformanceDetails {
    let script = r#"$c=Get-CimInstance Win32_VideoController; $c | Select-Object -First 1 Name,AdapterRAM,CurrentTemperature | ConvertTo-Json -Compress"#;
    let result = Command::new("powershell.exe")
        .creation_flags(CREATE_NO_WINDOW)
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
    let vram_bytes = value
        .get("AdapterRAM")
        .and_then(|v| v.as_u64())
        .map(|v| Capability {
            value: Some(v),
            unavailable_reason: None,
        })
        .unwrap_or_else(|| Capability {
            value: None,
            unavailable_reason: Some("VRAM não exposta pelo driver".into()),
        });
    let temperature_celsius = Capability {
        value: None,
        unavailable_reason: Some("Temperatura não exposta pelo driver WMI".into()),
    };
    PerformanceDetails {
        gpu,
        vram_bytes,
        temperature_celsius,
    }
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
