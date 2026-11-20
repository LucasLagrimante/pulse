//! "Iniciar com o sistema" do próprio Pulse.
//!
//! - Windows: valor `Pulse` em `HKCU\...\Run` + estado em `StartupApproved`.
//! - Linux: `~/.config/autostart/pulse.desktop` (XDG Autostart). Desativar grava
//!   `Hidden=true` e mantém o `Exec`, no mesmo espírito do `StartupApproved`.
//! - macOS: LaunchAgent em `~/Library/LaunchAgents/com.lagri.pulse.plist`
//!   (implementado, não testado em hardware).

use serde::Serialize;
use std::env;

#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const APPROVED_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
#[cfg(windows)]
const VALUE_NAME: &str = "Pulse";

/// Argumento que faz o Pulse abrir direto na bandeja.
const MINIMIZED_ARG: &str = "--minimized-to-tray";

/// Arquivo XDG Autostart do Pulse (o inventário de inicialização o reconhece
/// como "gerenciado pela Visão geral").
#[cfg(target_os = "linux")]
pub const AUTOSTART_FILE_NAME: &str = "pulse.desktop";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupRegistration {
    pub enabled: bool,
    pub command: Option<String>,
}

#[cfg(any(windows, test))]
fn command_value(executable: &str) -> String {
    format!("\"{executable}\" {MINIMIZED_ARG}")
}

#[cfg(windows)]
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

#[cfg(windows)]
fn is_startup_approved_enabled() -> bool {
    let output = crate::process::hidden_command("reg.exe")
        .args(["query", APPROVED_KEY, "/v", VALUE_NAME])
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

#[cfg(windows)]
fn write_startup_approved(enabled: bool) -> Result<(), String> {
    let hex = startup_approved_hex(enabled);
    let result = crate::process::hidden_command("reg.exe")
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
    args.iter().any(|arg| arg == MINIMIZED_ARG)
}

#[cfg(windows)]
#[tauri::command]
pub fn get_startup_registration() -> StartupRegistration {
    let output = crate::process::hidden_command("reg.exe")
        .args(["query", RUN_KEY, "/v", VALUE_NAME])
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

#[cfg(windows)]
#[tauri::command]
pub fn set_startup_registration(enabled: bool) -> Result<StartupRegistration, String> {
    if enabled {
        let executable = env::current_exe().map_err(|error| {
            format!("Não foi possível localizar o executável do Pulse: {error}")
        })?;
        let executable = executable.to_string_lossy().into_owned();
        let result = crate::process::hidden_command("reg.exe")
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
        write_startup_approved(true)?;
    } else {
        write_startup_approved(false)?;
    }

    Ok(get_startup_registration())
}

/* ------------------------------------------------------------------ */
/* Linux / macOS                                                       */
/* ------------------------------------------------------------------ */

/// Executável a registrar. Num AppImage o `current_exe` aponta para o ponto de
/// montagem temporário (`/tmp/.mount_xxx`), que some ao fechar o app; o
/// caminho estável é o do próprio arquivo, exposto em `$APPIMAGE`.
#[cfg(unix)]
fn stable_executable() -> Result<String, String> {
    if let Some(appimage) = env::var_os("APPIMAGE").filter(|value| !value.is_empty()) {
        return Ok(std::path::PathBuf::from(appimage)
            .to_string_lossy()
            .into_owned());
    }
    env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| format!("Não foi possível localizar o executável do Pulse: {error}"))
}

#[cfg(target_os = "linux")]
mod xdg {
    //! Entrada XDG Autostart do Pulse.

    use super::MINIMIZED_ARG;
    use std::path::PathBuf;

    pub const FILE_NAME: &str = super::AUTOSTART_FILE_NAME;

    pub fn autostart_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("autostart"))
    }

    /// Cita um argumento de `Exec` conforme a Desktop Entry Specification:
    /// entre aspas duplas quando há caractere reservado, com `"`, `` ` ``, `$`
    /// e `\` escapados por `\`. Como o valor do arquivo também é uma string
    /// com escapes, cada `\` resultante é dobrado.
    pub fn quote_exec_arg(arg: &str) -> String {
        let reserved = |ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '"' | '\''
                        | '\\'
                        | '>'
                        | '<'
                        | '~'
                        | '|'
                        | '&'
                        | ';'
                        | '$'
                        | '*'
                        | '?'
                        | '#'
                        | '('
                        | ')'
                        | '`'
                )
        };
        let percent_escaped = arg.replace('%', "%%");
        if !percent_escaped.chars().any(reserved) {
            return percent_escaped;
        }
        let mut quoted = String::from("\"");
        for ch in percent_escaped.chars() {
            match ch {
                '"' | '`' | '$' => {
                    quoted.push_str("\\\\");
                    quoted.push(ch);
                }
                '\\' => quoted.push_str("\\\\\\\\"),
                _ => quoted.push(ch),
            }
        }
        quoted.push('"');
        quoted
    }

    pub fn exec_line(executable: &str) -> String {
        format!("{} {MINIMIZED_ARG}", quote_exec_arg(executable))
    }

    pub fn desktop_entry(executable: &str, enabled: bool) -> String {
        let mut entry = String::from("[Desktop Entry]\n");
        entry.push_str("Type=Application\n");
        entry.push_str("Name=Pulse\n");
        entry.push_str("Comment=Monitor do sistema Pulse, iniciado na bandeja\n");
        entry.push_str(&format!("Exec={}\n", exec_line(executable)));
        entry.push_str("Icon=pulse\n");
        entry.push_str("Terminal=false\n");
        entry.push_str("X-GNOME-Autostart-enabled=true\n");
        // Desativar mantém o `Exec`: reativar não depende de reconstruir nada.
        entry.push_str(if enabled {
            "Hidden=false\n"
        } else {
            "Hidden=true\n"
        });
        entry
    }

    /// Lê `Exec`, `Hidden` e `X-GNOME-Autostart-enabled` do grupo
    /// `[Desktop Entry]`.
    pub fn parse_registration(text: &str) -> (bool, Option<String>) {
        let mut in_entry = false;
        let mut exec = None;
        let mut hidden = false;
        let mut gnome_disabled = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                match key.trim() {
                    "Exec" => exec = Some(value.trim().to_string()),
                    "Hidden" => hidden = value.trim().eq_ignore_ascii_case("true"),
                    "X-GNOME-Autostart-enabled" => {
                        gnome_disabled = value.trim().eq_ignore_ascii_case("false")
                    }
                    _ => {}
                }
            }
        }
        let enabled = exec.is_some() && !hidden && !gnome_disabled;
        (enabled, exec)
    }
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub fn get_startup_registration() -> StartupRegistration {
    let Some(path) = xdg::autostart_dir().map(|dir| dir.join(xdg::FILE_NAME)) else {
        return StartupRegistration {
            enabled: false,
            command: None,
        };
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (enabled, command) = xdg::parse_registration(&text);
            StartupRegistration { enabled, command }
        }
        Err(_) => StartupRegistration {
            enabled: false,
            command: None,
        },
    }
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub fn set_startup_registration(enabled: bool) -> Result<StartupRegistration, String> {
    let dir = xdg::autostart_dir().ok_or_else(|| {
        "Não foi possível localizar a pasta de configuração do usuário".to_string()
    })?;
    let path = dir.join(xdg::FILE_NAME);
    if !enabled && !path.exists() {
        // Nada registrado: desativar é um no-op honesto, não cria arquivo.
        return Ok(get_startup_registration());
    }
    let executable = stable_executable()?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Não foi possível criar {}: {error}", dir.display()))?;
    std::fs::write(&path, xdg::desktop_entry(&executable, enabled))
        .map_err(|error| format!("Não foi possível gravar {}: {error}", path.display()))?;
    Ok(get_startup_registration())
}

#[cfg(target_os = "macos")]
mod launch_agent {
    //! LaunchAgent do Pulse. TODO(macos): validar em hardware.

    use super::MINIMIZED_ARG;
    use std::path::PathBuf;

    pub const LABEL: &str = "com.lagri.pulse";

    pub fn plist_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| {
            home.join("Library")
                .join("LaunchAgents")
                .join(format!("{LABEL}.plist"))
        })
    }

    fn xml_escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    pub fn plist(executable: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
  <key>Label</key>\n  <string>{LABEL}</string>\n\
  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>{MINIMIZED_ARG}</string>\n  </array>\n\
  <key>RunAtLoad</key>\n  <true/>\n\
  <key>ProcessType</key>\n  <string>Interactive</string>\n\
</dict>\n</plist>\n",
            xml_escape(executable)
        )
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn get_startup_registration() -> StartupRegistration {
    match launch_agent::plist_path().and_then(|path| std::fs::read_to_string(path).ok()) {
        Some(text) => StartupRegistration {
            enabled: true,
            command: text
                .split("<string>")
                .nth(2)
                .and_then(|rest| rest.split("</string>").next())
                .map(|exe| format!("{exe} {MINIMIZED_ARG}")),
        },
        None => StartupRegistration {
            enabled: false,
            command: None,
        },
    }
}

/// No macOS desativar remove o plist do próprio Pulse: o arquivo é do app e
/// `launchd` não tem o equivalente do `Hidden=true`.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn set_startup_registration(enabled: bool) -> Result<StartupRegistration, String> {
    let path = launch_agent::plist_path()
        .ok_or_else(|| "Não foi possível localizar a pasta pessoal".to_string())?;
    if enabled {
        let executable = stable_executable()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|error| format!("Não foi possível criar {}: {error}", dir.display()))?;
        }
        std::fs::write(&path, launch_agent::plist(&executable))
            .map_err(|error| format!("Não foi possível gravar {}: {error}", path.display()))?;
    } else if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| format!("Não foi possível remover {}: {error}", path.display()))?;
    }
    Ok(get_startup_registration())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
#[tauri::command]
pub fn get_startup_registration() -> StartupRegistration {
    StartupRegistration {
        enabled: false,
        command: None,
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
#[tauri::command]
pub fn set_startup_registration(_enabled: bool) -> Result<StartupRegistration, String> {
    Err("Iniciar com o sistema não é suportado nesta plataforma".to_string())
}

#[cfg(test)]
mod tests {
    use super::{command_value, should_start_minimized};

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

    #[cfg(windows)]
    #[test]
    fn disabled_startup_approval_keeps_a_filetime() {
        let value = super::startup_approved_hex(false);
        assert!(value.starts_with("03000000"));
        assert_ne!(&value[8..], "0000000000000000");
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::super::xdg;

        #[test]
        fn exec_line_quotes_reserved_characters() {
            assert_eq!(
                xdg::exec_line("/usr/bin/pulse"),
                "/usr/bin/pulse --minimized-to-tray"
            );
            assert_eq!(
                xdg::exec_line("/opt/My Apps/pulse"),
                "\"/opt/My Apps/pulse\" --minimized-to-tray"
            );
            assert_eq!(xdg::quote_exec_arg("/a/100%"), "/a/100%%");
            assert_eq!(xdg::quote_exec_arg("/a b/$x"), "\"/a b/\\\\$x\"");
        }

        #[test]
        fn desktop_entry_round_trips_state() {
            let enabled = xdg::desktop_entry("/usr/bin/pulse", true);
            assert_eq!(
                xdg::parse_registration(&enabled),
                (true, Some("/usr/bin/pulse --minimized-to-tray".to_string()))
            );
            let disabled = xdg::desktop_entry("/usr/bin/pulse", false);
            let (state, exec) = xdg::parse_registration(&disabled);
            assert!(!state);
            assert!(exec.is_some(), "desativar mantém o Exec");
        }

        #[test]
        fn ignores_keys_outside_desktop_entry_group() {
            let text = "[Desktop Action x]\nExec=/bin/false\nHidden=true\n\n[Desktop Entry]\nExec=/bin/true\n";
            assert_eq!(
                xdg::parse_registration(text),
                (true, Some("/bin/true".to_string()))
            );
            let gnome_off = "[Desktop Entry]\nExec=/bin/true\nX-GNOME-Autostart-enabled=false\n";
            assert!(!xdg::parse_registration(gnome_off).0);
        }
    }
}
