//! Processo da tecla Windows com privilégio de administrador.
//!
//! O Windows (UIPI) não entrega teclas de uma janela elevada em foco a um hook
//! LL de integridade média: com um VS Code "como administrador" em primeiro
//! plano, o toque na tecla Windows escapava do Pulse e abria o Iniciar.
//!
//! A solução é rodar o processo auxiliar do hook elevado, sem elevar o Pulse
//! inteiro: uma tarefa agendada (`RunLevel Highest`), registrada uma vez com um
//! aviso do UAC, executa uma **cópia protegida** do executável em
//! `%ProgramFiles%\Pulse WinKey` — nunca o executável da pasta do usuário,
//! que qualquer processo comum poderia trocar para ganhar privilégio.
//! Depois de uma atualização do Pulse a cópia fica desatualizada: o Pulse volta
//! ao processo comum e o painel oferece "Atualizar" (novo aviso do UAC).

use serde::Serialize;

/// Nome da tarefa agendada que inicia o processo da tecla elevado.
pub const TASK_NAME: &str = "PulseWinKeyHook";
/// Argumento com que a tarefa agendada inicia a cópia protegida.
pub const TASK_HOST_ARG: &str = "--win-key-hook-task";
/// Código de saída do PowerShell quando o usuário recusa o aviso do UAC.
const UAC_CANCELLED_EXIT_CODE: i32 = 1223;

/// Estado do modo administrador da tecla Windows, exibido no painel Beta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WinKeyAdminState {
    /// Só existe no Windows.
    Unsupported,
    /// Não instalado: a tecla é tratada pelo processo comum.
    Off,
    /// Instalado e igual ao executável atual do Pulse.
    Ready,
    /// Instalado, mas a cópia protegida é de outra versão do Pulse.
    Outdated,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinKeyAdminStatus {
    pub state: WinKeyAdminState,
}

/// Script (rodado elevado) que copia o executável e registra a tarefa.
pub fn install_script(source_exe: &str, user: &str) -> String {
    format!(
        "$ErrorActionPreference='Stop'\n\
         $src = {src}\n\
         $dir = Join-Path $env:ProgramFiles 'Pulse WinKey'\n\
         $dst = Join-Path $dir 'pulse-windows.exe'\n\
         {stop}\n\
         New-Item -ItemType Directory -Force -Path $dir | Out-Null\n\
         Copy-Item -LiteralPath $src -Destination $dst -Force\n\
         $a = New-ScheduledTaskAction -Execute $dst -Argument '{arg}'\n\
         $p = New-ScheduledTaskPrincipal -UserId {user} -LogonType Interactive -RunLevel Highest\n\
         $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries \
         -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances Parallel -Priority 4\n\
         Register-ScheduledTask -TaskName '{task}' -Action $a -Principal $p -Settings $s -Force | Out-Null\n",
        src = ps_quote(source_exe),
        user = ps_quote(user),
        stop = stop_running_copy(),
        arg = TASK_HOST_ARG,
        task = TASK_NAME,
    )
}

/// Script (rodado elevado) que remove a tarefa e a cópia protegida.
pub fn uninstall_script() -> String {
    format!(
        "$ErrorActionPreference='Stop'\n\
         $dir = Join-Path $env:ProgramFiles 'Pulse WinKey'\n\
         $dst = Join-Path $dir 'pulse-windows.exe'\n\
         {stop}\n\
         Unregister-ScheduledTask -TaskName '{task}' -Confirm:$false -ErrorAction SilentlyContinue\n\
         if (Test-Path -LiteralPath $dir) {{ Remove-Item -LiteralPath $dir -Recurse -Force }}\n",
        stop = stop_running_copy(),
        task = TASK_NAME,
    )
}

/// Encerra a tarefa e qualquer processo rodando da cópia protegida (o arquivo
/// fica bloqueado enquanto ela roda).
fn stop_running_copy() -> String {
    format!(
        "Get-ScheduledTask -TaskName '{TASK_NAME}' -ErrorAction SilentlyContinue | Stop-ScheduledTask -ErrorAction SilentlyContinue\n\
         Get-Process -Name 'pulse-windows' -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq $dst }} | Stop-Process -Force -ErrorAction SilentlyContinue\n\
         Start-Sleep -Milliseconds 300"
    )
}

/// Literal PowerShell entre aspas simples.
fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Script de uma linha (rodado sem elevação) que pede o UAC e roda `inner`
/// elevado. Recusa no UAC vira `1223`.
pub fn elevate_script(inner_encoded: &str) -> String {
    format!(
        "$ErrorActionPreference='Stop'; try {{ $p = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{inner_encoded}') -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode }} catch {{ $e = $_.Exception; while ($e) {{ if ($e.NativeErrorCode -eq {UAC_CANCELLED_EXIT_CODE} -or $e.HResult -eq -2147023673) {{ exit {UAC_CANCELLED_EXIT_CODE} }}; $e = $e.InnerException }}; [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}"
    )
}

/// Script PowerShell em UTF-16LE/base64 para `-EncodedCommand`.
fn encode_ps(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::path::PathBuf;

    /// Executável protegido executado pela tarefa agendada.
    pub fn protected_exe() -> Option<PathBuf> {
        let base = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles"))?;
        Some(
            PathBuf::from(base)
                .join("Pulse WinKey")
                .join("pulse-windows.exe"),
        )
    }

    pub fn status() -> WinKeyAdminState {
        let Some(protected) = protected_exe() else {
            return WinKeyAdminState::Off;
        };
        let Ok(copy) = std::fs::read(&protected) else {
            return WinKeyAdminState::Off;
        };
        let current = std::env::current_exe().and_then(std::fs::read);
        match current {
            Ok(current) if current == copy => WinKeyAdminState::Ready,
            _ => WinKeyAdminState::Outdated,
        }
    }

    /// Roda `script` elevado (um aviso do UAC) e espera terminar.
    pub fn run_elevated(script: &str) -> Result<(), String> {
        let outer = elevate_script(&encode_ps(script));
        let output = crate::process::hidden_command("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &encode_ps(&outer),
            ])
            .output()
            .map_err(|error| format!("Não foi possível iniciar o PowerShell: {error}"))?;
        match output.status.code() {
            Some(0) => Ok(()),
            Some(UAC_CANCELLED_EXIT_CODE) => {
                Err("O aviso de administrador do Windows foi recusado.".to_string())
            }
            code => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = stderr.trim();
                Err(format!(
                    "O Windows não concluiu a configuração (código {}){}",
                    code.map_or_else(|| "?".to_string(), |c| c.to_string()),
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                ))
            }
        }
    }

    pub fn install() -> Result<(), String> {
        let exe = std::env::current_exe()
            .map_err(|error| format!("Executável do Pulse não encontrado: {error}"))?;
        let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(domain), Ok(name)) if !domain.is_empty() => format!("{domain}\\{name}"),
            (_, Ok(name)) => name,
            _ => return Err("Usuário do Windows não identificado.".to_string()),
        };
        run_elevated(&install_script(&exe.to_string_lossy(), &user))
    }

    pub fn uninstall() -> Result<(), String> {
        run_elevated(&uninstall_script())
    }

    /// Inicia a tarefa agendada (sem aviso do UAC: a elevação foi concedida no registro).
    pub fn run_task() -> Result<(), String> {
        let output = crate::process::hidden_command("schtasks.exe")
            .args(["/run", "/tn", TASK_NAME])
            .output()
            .map_err(|error| format!("schtasks indisponível: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "schtasks /run falhou ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// Estado atual do modo administrador.
pub fn status() -> WinKeyAdminState {
    #[cfg(windows)]
    {
        platform::status()
    }
    #[cfg(not(windows))]
    {
        WinKeyAdminState::Unsupported
    }
}

/// Inicia o processo da tecla elevado pela tarefa agendada.
#[cfg(windows)]
pub fn run_task() -> Result<(), String> {
    platform::run_task()
}

#[tauri::command]
pub async fn win_key_admin_status() -> WinKeyAdminStatus {
    let state = tauri::async_runtime::spawn_blocking(status)
        .await
        .unwrap_or(WinKeyAdminState::Off);
    WinKeyAdminStatus { state }
}

/// Instala ou atualiza o modo administrador (um aviso do UAC) e reinicia o hook.
#[tauri::command]
pub async fn win_key_admin_enable() -> Result<WinKeyAdminStatus, String> {
    change(true).await
}

/// Remove a tarefa agendada e a cópia protegida (um aviso do UAC).
#[tauri::command]
pub async fn win_key_admin_disable() -> Result<WinKeyAdminStatus, String> {
    change(false).await
}

async fn change(enable: bool) -> Result<WinKeyAdminStatus, String> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let result = if enable {
                platform::install()
            } else {
                platform::uninstall()
            };
            if let Err(error) = &result {
                crate::debug::record_failure(
                    "launcher_win_key_admin",
                    started.elapsed().as_millis(),
                    crate::debug::Failure::new(if enable {
                        "Não foi possível ativar a tecla Windows sobre janelas de administrador"
                    } else {
                        "Não foi possível desativar a tecla Windows sobre janelas de administrador"
                    })
                    .stage(if enable { "install" } else { "uninstall" })
                    .detail(error.clone()),
                );
            }
            // Reinstala o hook para trocar de processo comum para elevado (ou vice-versa).
            crate::launcher::restart_win_key_hook();
            result.map(|()| WinKeyAdminStatus { state: status() })
        })
        .await
        .map_err(|_| "A configuração foi interrompida.".to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = enable;
        Err("Disponível apenas no Windows.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_script_quotes_paths_and_uses_protected_copy() {
        let script = install_script(r"C:\Users\dev\App's\pulse-windows.exe", r"PC\dev");
        assert!(script.contains(r"$src = 'C:\Users\dev\App''s\pulse-windows.exe'"));
        assert!(script.contains("-UserId 'PC\\dev'"));
        assert!(script.contains("Join-Path $env:ProgramFiles 'Pulse WinKey'"));
        assert!(script.contains("-Execute $dst -Argument '--win-key-hook-task'"));
        assert!(script.contains("-RunLevel Highest"));
        // A tarefa nunca aponta para o executável da pasta do usuário.
        assert!(!script.contains("-Execute $src"));
    }

    #[test]
    fn uninstall_script_removes_task_and_copy() {
        let script = uninstall_script();
        assert!(script.contains("Unregister-ScheduledTask -TaskName 'PulseWinKeyHook'"));
        assert!(script.contains("Remove-Item -LiteralPath $dir -Recurse -Force"));
    }

    #[test]
    fn elevate_script_maps_uac_refusal() {
        let script = elevate_script("QUJD");
        assert!(script.contains("-Verb RunAs"));
        assert!(script.contains("'-EncodedCommand','QUJD'"));
        assert!(script.contains("exit 1223"));
    }
}
