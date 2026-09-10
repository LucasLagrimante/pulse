use serde::Serialize;
use std::env;
use std::process::Command;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Pulse";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupRegistration {
    pub enabled: bool,
    pub command: Option<String>,
}

fn command_value(executable: &str) -> String {
    format!("\"{executable}\"")
}

#[tauri::command]
pub fn get_startup_registration() -> StartupRegistration {
    let output = Command::new("reg.exe")
        .args(["query", RUN_KEY, "/v", VALUE_NAME])
        .output();

    match output {
        Ok(result) if result.status.success() => StartupRegistration {
            enabled: true,
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
            .output()
            .map_err(|error| format!("Não foi possível registrar a inicialização: {error}"))?;

        if !result.status.success() {
            return Err(format!(
                "O Windows recusou o registro de inicialização: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
    } else {
        let result = Command::new("reg.exe")
            .args(["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
            .output()
            .map_err(|error| format!("Não foi possível remover a inicialização: {error}"))?;

        if !result.status.success()
            && String::from_utf8_lossy(&result.stderr).contains("Unable to find")
        {
            return Ok(StartupRegistration {
                enabled: false,
                command: None,
            });
        }
        if !result.status.success() {
            return Err(format!(
                "O Windows recusou a remoção da inicialização: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
    }

    Ok(get_startup_registration())
}

#[cfg(test)]
mod tests {
    use super::command_value;

    #[test]
    fn startup_command_quotes_executables_with_spaces() {
        assert_eq!(
            command_value(r"C:\Program Files\Pulse\pulse-windows.exe"),
            r#""C:\Program Files\Pulse\pulse-windows.exe""#
        );
    }
}
