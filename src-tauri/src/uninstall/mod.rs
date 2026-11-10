//! Desinstalador de programas com rastreamento de resíduos.
//!
//! Fluxo em quatro etapas, cada uma com um comando próprio:
//!
//! 1. `list_installed_apps` — inventário vindo do registro;
//! 2. `start_uninstall_session` + `create_restore_point` — restauração
//!    opcional e o **snapshot** do que existe agora;
//! 3. `run_app_uninstaller` — desinstalador padrão do fabricante, aguardado de
//!    verdade (a chave de desinstalação precisa sumir);
//! 4. `cleanup_uninstall_residue` — remoção apenas do que o usuário marcou
//!    **e** do que ainda bate com o snapshot.
//!
//! Princípios que sustentam o módulo:
//!
//! - **Nada é apagado por semelhança.** Só entra na lista o que tem nome exato
//!   (ver `residue`).
//! - **Nada é apagado sem prova.** Cada remoção revalida o snapshot e os
//!   guardrails de sistema antes de tocar o disco.
//! - **Nada é inventado.** Sem elevação, sem ponto de restauração, sem sensor:
//!   a tela diz o motivo em vez de fingir sucesso (`AGENTS.md`).
//! - **Whitelist de itens.** O front-end envia ids que o backend ofereceu; ids
//!   fora da sessão são recusados.

pub mod footprint;
pub mod installed;
pub mod platform;
pub mod registry;
pub mod residue;
pub mod session;

pub use installed::{list_installed_apps, InstalledApp, InstalledAppsSnapshot};
pub use platform::UninstallCapabilities;
pub use session::{
    cancel_uninstall_session, cleanup_uninstall_residue, run_app_uninstaller, start_uninstall_session,
    CleanupReport, UninstallSession, UninstallerOutcome,
};

/// Usado pelo modal para desabilitar o checkbox de restauração com o motivo na
/// tela, em vez de deixar o clique falhar depois.
#[tauri::command]
pub async fn get_uninstall_capabilities() -> UninstallCapabilities {
    platform::capabilities()
}

/// Cria o ponto de restauração isoladamente. O botão "seguir" da etapa 1 usa
/// este comando antes de chamar o desinstalador.
#[tauri::command]
pub async fn create_system_restore_point(description: String) -> platform::RestorePointOutcome {
    let text = description;
    tauri::async_runtime::spawn_blocking(move || platform::create_restore_point(&text))
        .await
        .unwrap_or(platform::RestorePointOutcome {
            created: false,
            verified: false,
            sequence_number: 0,
            description: String::new(),
            unavailable_reason: Some(
                "A criação do ponto de restauração foi interrompida.".to_string(),
            ),
        })
}