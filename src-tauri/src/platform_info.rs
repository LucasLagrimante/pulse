//! Identificação da plataforma em que o Pulse está rodando.
//!
//! O frontend consulta isto uma única vez para trocar textos e esconder ações
//! que só existem no Windows. Tudo aqui é leitura barata (variáveis de ambiente,
//! `/etc/os-release`, `sw_vers`); nenhuma coleta é repetida.

use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    /// `"windows"`, `"linux"` ou `"macos"` (outros SOs devolvem `std::env::consts::OS`).
    pub os: String,
    /// Nome amigável do sistema, ex.: `"Linux Mint 22.3"`, `"macOS 14.5"`, `"Windows"`.
    pub os_name: String,
    /// Versão do sistema quando detectável.
    pub version: Option<String>,
    /// Ambiente de desktop (`XDG_CURRENT_DESKTOP`), só no Linux.
    pub desktop: Option<String>,
    /// Tipo de sessão gráfica (`x11`/`wayland`), só no Linux.
    pub session_type: Option<String>,
    /// Formato de instalação do Pulse quando detectável (`appimage`, `deb`, `rpm`, `nsis`, `app`).
    pub package_format: Option<String>,
    /// Arquitetura do binário (`x86_64`, `aarch64`).
    pub arch: String,
}

/// Lê um campo `CHAVE=valor` de um texto no formato de `/etc/os-release`.
fn os_release_field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.trim().split_once('=')?;
        if k != key {
            return None;
        }
        let value = v.trim().trim_matches('"').trim_matches('\'').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Monta nome e versão a partir do conteúdo de `/etc/os-release`.
fn parse_os_release(text: &str) -> (Option<String>, Option<String>) {
    let version = os_release_field(text, "VERSION_ID");
    let name = os_release_field(text, "NAME").map(|name| match &version {
        Some(v) if !name.contains(v.as_str()) => format!("{name} {v}"),
        _ => name,
    });
    let name = name.or_else(|| os_release_field(text, "PRETTY_NAME"));
    (name, version)
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Normaliza `XDG_SESSION_TYPE`; valores desconhecidos (`tty`, `unspecified`) viram `None`.
fn normalize_session_type(raw: Option<String>) -> Option<String> {
    let value = raw?.to_ascii_lowercase();
    matches!(value.as_str(), "x11" | "wayland").then_some(value)
}

/// Deduz o formato de instalação no Linux pelo ambiente e pelo caminho do executável.
fn linux_package_format(
    appimage_env: Option<&str>,
    exe: Option<&std::path::Path>,
) -> Option<String> {
    if appimage_env.is_some_and(|v| !v.is_empty()) {
        return Some("appimage".into());
    }
    let exe = exe?;
    if !exe.starts_with("/usr") {
        return None;
    }
    // Instalado em /usr: pacote do sistema. Distingue deb/rpm pelo gerenciador presente.
    if std::path::Path::new("/var/lib/dpkg/status").exists() {
        Some("deb".into())
    } else if std::path::Path::new("/var/lib/rpm").exists() {
        Some("rpm".into())
    } else {
        None
    }
}

fn detect() -> PlatformInfo {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let exe = std::env::current_exe().ok();
    match os.as_str() {
        "linux" => {
            let text = std::fs::read_to_string("/etc/os-release")
                .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
                .unwrap_or_default();
            let (name, version) = parse_os_release(&text);
            PlatformInfo {
                os_name: name.unwrap_or_else(|| "Linux".into()),
                version,
                desktop: non_empty_env("XDG_CURRENT_DESKTOP"),
                session_type: normalize_session_type(non_empty_env("XDG_SESSION_TYPE")),
                package_format: linux_package_format(
                    non_empty_env("APPIMAGE").as_deref(),
                    exe.as_deref(),
                ),
                os,
                arch,
            }
        }
        "macos" => {
            // TODO(macos): validar em hardware.
            let version = Command::new("sw_vers")
                .arg("-productVersion")
                .env("LC_ALL", "C")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|v| !v.is_empty());
            let package_format = exe
                .as_deref()
                .and_then(|p| p.to_str())
                .filter(|p| p.contains(".app/Contents/MacOS/"))
                .map(|_| "app".to_string());
            PlatformInfo {
                os_name: match &version {
                    Some(v) => format!("macOS {v}"),
                    None => "macOS".into(),
                },
                version,
                desktop: None,
                session_type: None,
                package_format,
                os,
                arch,
            }
        }
        "windows" => PlatformInfo {
            os_name: "Windows".into(),
            version: None,
            desktop: None,
            session_type: None,
            package_format: Some("nsis".into()),
            os,
            arch,
        },
        _ => PlatformInfo {
            os_name: os.clone(),
            version: None,
            desktop: None,
            session_type: None,
            package_format: None,
            os,
            arch,
        },
    }
}

#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    detect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINT: &str = r#"NAME="Linux Mint"
VERSION="22.3 (Zena)"
ID=linuxmint
ID_LIKE="ubuntu debian"
PRETTY_NAME="Linux Mint 22.3"
VERSION_ID="22.3"
"#;

    #[test]
    fn os_release_monta_nome_com_versao() {
        let (name, version) = parse_os_release(MINT);
        assert_eq!(name.as_deref(), Some("Linux Mint 22.3"));
        assert_eq!(version.as_deref(), Some("22.3"));
    }

    #[test]
    fn os_release_sem_versao_usa_nome_puro() {
        let (name, version) = parse_os_release("NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\n");
        assert_eq!(name.as_deref(), Some("Arch Linux"));
        assert_eq!(version, None);
    }

    #[test]
    fn os_release_vazio_nao_inventa_nome() {
        assert_eq!(parse_os_release(""), (None, None));
    }

    #[test]
    fn session_type_aceita_so_x11_e_wayland() {
        assert_eq!(
            normalize_session_type(Some("X11".into())).as_deref(),
            Some("x11")
        );
        assert_eq!(
            normalize_session_type(Some("wayland".into())).as_deref(),
            Some("wayland")
        );
        assert_eq!(normalize_session_type(Some("tty".into())), None);
        assert_eq!(normalize_session_type(None), None);
    }

    #[test]
    fn appimage_tem_prioridade() {
        let exe = std::path::Path::new("/tmp/.mount_Pulse/usr/bin/pulse-windows");
        assert_eq!(
            linux_package_format(Some("/home/u/Pulse.AppImage"), Some(exe)).as_deref(),
            Some("appimage")
        );
        assert_eq!(
            linux_package_format(
                None,
                Some(std::path::Path::new("/home/u/dev/target/debug/pulse"))
            ),
            None
        );
    }

    #[test]
    fn detect_devolve_os_coerente() {
        let info = detect();
        assert_eq!(info.os, std::env::consts::OS);
        assert!(!info.os_name.is_empty());
        assert!(!info.arch.is_empty());
        #[cfg(target_os = "linux")]
        assert!(info.desktop.is_none() || !info.desktop.as_deref().unwrap_or_default().is_empty());
    }
}
