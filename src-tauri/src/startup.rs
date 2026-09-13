use serde::Serialize;
use std::env;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

// Impede que janelas de console apareçam ao executar subprocessos no Windows
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const VALUE_NAME: &str = "Pulse";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupRegistration {
    pub enabled: bool,
    pub command: Option<String>,
}

fn command_value(executable: &str) -> String {
    format!("\"{executable}\" --minimized-to-tray")
}

fn startup_approved_hex(enabled: bool) -> String {
    if enabled {
        return "020000000000000000000000".to_owned();
    }

    // StartupApproved armazena o FILETIME em little-endian depois do estado.
    // Mantemos o valor Run intacto para que o Windows e o usuário possam
    // reativá-lo sem reconstruir a linha de comando.
    let filetime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| {
            elapsed
                .as_secs()
                .saturating_add(11_644_473_600)
                .saturating_mul(10_000_000)
        })
        .unwrap_or(0);
    let mut bytes = vec![0x03, 0x00, 0x00, 0x00];
    bytes.extend_from_slice(&filetime.to_le_bytes());
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn is_startup_approved_enabled() -> bool {
    let output = Command::new("reg.exe")
        .args(["query", APPROVED_KEY, "/v", VALUE_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(result) = output else { return true };
    if !result.status.success() {
        return true;
    }
    let text = String::from_utf8_lossy(&result.stdout);
    text.split_whitespace()
        .find(|token| {
            token.len() >= 2
                && token.as_bytes()[0].is_ascii_hexdigit()
                && token.as_bytes()[1].is_ascii_hexdigit()
        })
        .and_then(|hex| u8::from_str_radix(&hex[..2], 16).ok())
        .map(|state| state & 1 == 0)
        .unwrap_or(true)
}

fn write_startup_approved(enabled: bool) -> Result<(), String> {
    let hex = startup_approved_hex(enabled);
    let result = Command::new("reg.exe")
        .args([
            "add",
            APPROVED_KEY,
            "/v",
            VALUE_NAME,
            "/t",
            "REG_BINARY",
            "/d",
            &hex,
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("Não foi possível atualizar StartupApproved: {error}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "O Windows recusou a atualização de StartupApproved: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

pub fn should_start_minimized(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--minimized-to-tray")
}

#[tauri::command]
pub fn get_startup_registration() -> StartupRegistration {
    let output = Command::new("reg.exe")
        .args(["query", RUN_KEY, "/v", VALUE_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(result) if result.status.success() => StartupRegistration {
            enabled: is_startup_approved_enabled(),
            command: Some(String::from_utf8_lossy(&result.stdout).trim().to_owned()),
        },
        _ => StartupRegistration {
            enabled: false,
            command: None,
        },
    }
}

#[tauri::command]
pub fn set_startup_registration(enabled: bool) -> Result<StartupRegistration, String> {
    if enabled {
        let executable = env::current_exe().map_err(|error| {
            format!("Não foi possível localizar o executável do Pulse: {error}")
        })?;
        let executable = executable.to_string_lossy().into_owned();
        let result = Command::new("reg.exe")
            .args([
                "add",
                RUN_KEY,
                "/v",
                VALUE_NAME,
                "/t",
                "REG_SZ",
                "/d",
                &command_value(&executable),
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("Não foi possível registrar a inicialização: {error}"))?;

        if !result.status.success() {
            return Err(format!(
                "O Windows recusou o registro de inicialização: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        write_startup_approved(true)?;
    } else {
        write_startup_approved(false)?;
    }

    Ok(get_startup_registration())
}

#[cfg(test)]
mod tests {
    use super::{command_value, should_start_minimized, startup_approved_hex};

    #[test]
    fn startup_command_quotes_executables_with_spaces() {
        assert_eq!(
            command_value(r"C:\Program Files\Pulse\pulse-windows.exe"),
            r#""C:\Program Files\Pulse\pulse-windows.exe" --minimized-to-tray"#
        );
    }

    #[test]
    fn detects_the_start_minimized_argument() {
        assert!(should_start_minimized(&[
            "pulse.exe".into(),
            "--minimized-to-tray".into()
        ]));
        assert!(!should_start_minimized(&["pulse.exe".into()]));
    }

    #[test]
    fn disabled_startup_approval_keeps_a_filetime() {
        let value = startup_approved_hex(false);
        assert!(value.starts_with("03000000"));
        assert_ne!(&value[8..], "0000000000000000");
    }
}
