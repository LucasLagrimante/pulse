//! Elevação do processo e ponto de restauração por plataforma.
//!
//! - Windows: `platform_windows.rs` (token efetivo + `srclient.dll`).
//! - Linux/macOS: elevação é `euid == 0`. Não existe um "ponto de restauração"
//!   padrão do sistema (Timeshift/Snapper no Linux e Time Machine no macOS são
//!   opcionais e têm semântica própria), então a opção é declarada
//!   indisponível com o motivo, nunca simulada.

#[cfg(windows)]
#[path = "platform_windows.rs"]
mod imp;

#[cfg(not(windows))]
mod imp {
    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UninstallCapabilities {
        pub elevated: bool,
        pub restore_point_supported: bool,
        pub restore_point_unavailable_reason: Option<String>,
        pub verification_available: bool,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RestorePointOutcome {
        pub created: bool,
        pub verified: bool,
        pub sequence_number: i64,
        pub description: String,
        pub unavailable_reason: Option<String>,
    }

    #[cfg(target_os = "macos")]
    const RESTORE_UNAVAILABLE: &str =
        "O macOS não tem ponto de restauração por aplicativo; use um backup do Time Machine antes de desinstalar.";
    #[cfg(not(target_os = "macos"))]
    const RESTORE_UNAVAILABLE: &str =
        "O Linux não tem ponto de restauração padrão do sistema; se usar Timeshift ou Snapper, crie o snapshot por eles antes de desinstalar.";

    /// `true` só quando o processo roda como root. O Pulse não deve rodar
    /// assim no Linux/macOS: ações privilegiadas pedem senha pontualmente.
    pub fn is_elevated() -> bool {
        // SAFETY: `geteuid` não recebe argumentos e não falha.
        unsafe { libc::geteuid() == 0 }
    }

    pub fn create_restore_point(description: &str) -> RestorePointOutcome {
        RestorePointOutcome {
            created: false,
            verified: false,
            sequence_number: 0,
            description: description.trim().chars().take(200).collect(),
            unavailable_reason: Some(RESTORE_UNAVAILABLE.to_string()),
        }
    }

    pub fn capabilities() -> UninstallCapabilities {
        UninstallCapabilities {
            elevated: is_elevated(),
            restore_point_supported: false,
            restore_point_unavailable_reason: Some(RESTORE_UNAVAILABLE.to_string()),
            verification_available: false,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn restore_point_fora_do_windows_sempre_explica_o_motivo() {
            let outcome = create_restore_point("teste");
            assert!(!outcome.created);
            assert!(outcome.unavailable_reason.is_some());
            assert!(capabilities().restore_point_unavailable_reason.is_some());
        }
    }
}

pub use imp::*;
