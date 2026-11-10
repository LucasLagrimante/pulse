//! Nível de privilégio do processo do Pulse e reinício como administrador.
//!
//! A elevação não muda durante a vida do processo, então o resultado é lido uma
//! vez. A lista de limitações descreve só o que foi verificado no código: cada
//! item aponta uma funcionalidade que o Windows recusa sem token elevado.

use serde::Serialize;
use std::sync::OnceLock;

/// Argumento que o processo elevado recebe para esperar a instância anterior
/// liberar o mutex de instância única, em vez de só focá-la e sair.
pub const ELEVATED_RELAUNCH_ARG: &str = "--elevated-relaunch";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrivilegeLimitation {
    pub area: &'static str,
    pub feature: &'static str,
    pub detail: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivilegeStatus {
    /// `true` só com token efetivo elevado; "não sei" vira `false`.
    pub elevated: bool,
    /// O que fica limitado no modo atual (vazio quando elevado).
    pub limitations: Vec<PrivilegeLimitation>,
}

/// Funcionalidades que dependem de administrador, verificadas no código.
fn admin_only_features() -> Vec<PrivilegeLimitation> {
    vec![
        PrivilegeLimitation {
            area: "Limpeza",
            feature: "Temporários do Windows, Otimização de Entrega e instaladores NVIDIA",
            detail: "Pastas do sistema (C:\\Windows, ProgramData) só são listadas e apagadas por administrador; sem isso a análise fica parcial e os itens são pulados.",
        },
        PrivilegeLimitation {
            area: "Limpeza",
            feature: "Minidumps do Windows",
            detail: "C:\\Windows\\Minidump exige administrador; os despejos de falha do usuário continuam funcionando.",
        },
        PrivilegeLimitation {
            area: "Desinstalar",
            feature: "Ponto de restauração antes de desinstalar",
            detail: "O Windows só grava pontos de restauração com privilégio administrativo.",
        },
        PrivilegeLimitation {
            area: "Desinstalar",
            feature: "Resíduos em HKLM",
            detail: "Chaves de registro de máquina não podem ser removidas sem administrador; o Pulse as lista e explica a recusa.",
        },
        PrivilegeLimitation {
            area: "Serviços",
            feature: "Iniciar e parar serviços do sistema",
            detail: "Serviços do Windows recusam o controle sem administrador; serviços por usuário continuam funcionando.",
        },
    ]
}

fn build_status(elevated: bool) -> PrivilegeStatus {
    PrivilegeStatus {
        elevated,
        limitations: if elevated {
            Vec::new()
        } else {
            admin_only_features()
        },
    }
}

pub fn is_elevated_cached() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(crate::uninstall::platform::is_elevated)
}

#[tauri::command]
pub fn get_privilege_status() -> PrivilegeStatus {
    build_status(is_elevated_cached())
}

/// Abre o UAC para iniciar uma cópia elevada do Pulse e encerra esta instância
/// assim que o Windows aceitar. Se o usuário cancelar o UAC, nada muda.
#[tauri::command]
pub async fn relaunch_as_admin(app: tauri::AppHandle) -> Result<(), String> {
    if is_elevated_cached() {
        return Err("O Pulse já está em modo administrador.".into());
    }
    // ShellExecuteExW fica bloqueado enquanto o UAC está aberto.
    tauri::async_runtime::spawn_blocking(launch_elevated)
        .await
        .map_err(|error| format!("Falha interna ao pedir elevação: {error}"))??;
    crate::debug::record("privileges_relaunch", 0, Ok(()));
    app.exit(0);
    Ok(())
}

fn launch_elevated() -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = std::env::current_exe()
        .map_err(|error| format!("Não foi possível localizar o executável do Pulse: {error}"))?;
    let wide = |text: &std::ffi::OsStr| -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    };
    let verb = wide(std::ffi::OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    // Argumento fixo: nada vindo do IPC entra na linha de comando.
    let params = wide(std::ffi::OsStr::new(ELEVATED_RELAUNCH_ARG));
    let dir = exe.parent().map(|parent| wide(parent.as_os_str()));

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        lpDirectory: dir
            .as_ref()
            .map_or_else(PCWSTR::null, |dir| PCWSTR(dir.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: todos os ponteiros apontam para buffers UTF-16 terminados em zero
    // que vivem até o fim desta função; `cbSize` segue o contrato da API.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|error| {
        if error.code() == ERROR_CANCELLED.to_hresult() {
            "A permissão de administrador (UAC) foi cancelada. O Pulse continua no modo padrão."
                .to_string()
        } else {
            format!("Não foi possível reiniciar como administrador: {error}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::build_status;

    #[test]
    fn elevated_has_no_limitations_and_standard_lists_each_area() {
        assert!(build_status(true).limitations.is_empty());
        let standard = build_status(false);
        assert!(!standard.elevated);
        for area in ["Limpeza", "Desinstalar", "Serviços"] {
            assert!(
                standard.limitations.iter().any(|item| item.area == area),
                "{area} deveria aparecer"
            );
        }
    }
}
