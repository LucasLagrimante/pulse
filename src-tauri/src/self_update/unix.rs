//! Atualização do Pulse no Linux e no macOS.
//!
//! - Linux AppImage (`APPIMAGE` definido e pasta gravável): baixa
//!   `Pulse_<versão>_amd64.AppImage` da GitHub Release oficial, valida o cabeçalho
//!   ELF e o `SHA-256` (quando publicado no manifesto), substitui o arquivo
//!   atomicamente via `rename` no mesmo diretório e relança o novo AppImage.
//! - Linux `.deb` / outros empacotamentos (ou AppImage em pasta somente leitura):
//!   apenas informa a nova versão com o link da GitHub Release.
//! - macOS: apenas informa a nova versão com o link da GitHub Release.
//!   // TODO(macos): validar em hardware.

#[cfg(target_os = "linux")]
use super::{current_version, now_ms, save_config, sha256_hex, UPDATED_RELAUNCH_ARG};
#[cfg(any(target_os = "linux", test))]
use super::{parse_semver, validate_sha256_hex, ALLOWED_URL_PREFIXES};
use super::{update_and_emit, ReleaseManifest, SelfUpdateConfig, SelfUpdateStatus};
#[cfg(target_os = "linux")]
use reqwest::blocking::Client;
use std::time::Instant;
#[cfg(target_os = "linux")]
use std::{
    env,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
#[cfg(any(target_os = "linux", test))]
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use tauri::AppHandle;
#[cfg(target_os = "linux")]
use tauri::Manager;

const RELEASE_TAG_BASE_URL: &str = "https://github.com/LucasLagrimante/pulse/releases/tag/v";
#[cfg(any(target_os = "linux", test))]
const GITHUB_RELEASE_DOWNLOAD_BASE: &str =
    "https://github.com/LucasLagrimante/pulse/releases/download";

#[cfg(target_os = "linux")]
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
#[cfg(any(target_os = "linux", test))]
const MIN_APPIMAGE_BYTES: usize = 512 * 1024;
#[cfg(any(target_os = "linux", test))]
const MAX_APPIMAGE_BYTES: usize = 250 * 1024 * 1024;

/// URL oficial da página da release para atualização manual.
pub(super) fn release_page_url(version: &str) -> String {
    let clean = version.trim().trim_start_matches(['v', 'V']);
    format!("{RELEASE_TAG_BASE_URL}{clean}")
}

/// Tamanho formatado do artefato correspondente à plataforma atual.
/// Nunca reaproveita o tamanho do instalador `.exe` do Windows.
pub(super) fn release_size_formatted(manifest: &ReleaseManifest) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let linux = manifest.linux.as_ref()?;
        if detect_appimage_path().is_some() {
            linux
                .appimage
                .as_ref()
                .and_then(|asset| asset.size_formatted.clone())
        } else {
            linux
                .deb
                .as_ref()
                .and_then(|asset| asset.size_formatted.clone())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = manifest;
        None
    }
}

/// Escolhe a URL e o SHA-256 opcional do AppImage a partir do manifesto.
///
/// Se `manifest.linux.appimage` estiver presente, valida a URL contra a lista
/// oficial e o SHA-256 (se publicado). Caso o manifesto só contenha a seção
/// Windows, deriva a URL canônica da GitHub Release (`Pulse_<ver>_amd64.AppImage`).
#[cfg(any(target_os = "linux", test))]
pub(super) fn select_and_validate_appimage_asset(
    manifest: &ReleaseManifest,
    remote_version: &str,
) -> Result<(String, Option<String>), String> {
    if let Some(asset) = manifest.linux.as_ref().and_then(|l| l.appimage.as_ref()) {
        let candidate = asset
            .asset_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| asset.url.as_deref().filter(|s| !s.trim().is_empty()));
        if let Some(url) = candidate.map(str::trim) {
            if !ALLOWED_URL_PREFIXES
                .iter()
                .any(|prefix| url.starts_with(prefix))
            {
                return Err(
                    "A URL do AppImage no manifesto não pertence ao repositório ou domínio oficial do Pulse."
                        .to_owned(),
                );
            }
            if !url.ends_with(".AppImage")
                || url.contains('"')
                || url.contains('\'')
                || url.chars().any(char::is_whitespace)
            {
                return Err("A URL do AppImage no manifesto tem formato inválido.".to_owned());
            }
            let sha = if asset.sha256.trim().is_empty() {
                None
            } else {
                Some(validate_sha256_hex(&asset.sha256)?)
            };
            return Ok((url.to_owned(), sha));
        }
        // Manifesto trouxe só o sha256 sem URL explícita: deriva a URL oficial.
        let sha = if asset.sha256.trim().is_empty() {
            None
        } else {
            Some(validate_sha256_hex(&asset.sha256)?)
        };
        let url = canonical_appimage_url(remote_version)?;
        return Ok((url, sha));
    }

    Ok((canonical_appimage_url(remote_version)?, None))
}

#[cfg(any(target_os = "linux", test))]
fn canonical_appimage_url(version: &str) -> Result<String, String> {
    let (major, minor, patch) = parse_semver(version).ok_or_else(|| {
        format!("Versão de release inválida para download do AppImage: {version}")
    })?;
    let ver = format!("{major}.{minor}.{patch}");
    Ok(format!(
        "{GITHUB_RELEASE_DOWNLOAD_BASE}/v{ver}/Pulse_{ver}_amd64.AppImage"
    ))
}

/// Detecta execução via AppImage (`$APPIMAGE` apontando para um arquivo real).
#[cfg(target_os = "linux")]
fn detect_appimage_path() -> Option<PathBuf> {
    let raw = env::var_os("APPIMAGE").filter(|v| !v.is_empty())?;
    let path = PathBuf::from(raw);
    path.is_file().then_some(path)
}

/// Verifica se conseguimos criar arquivo temporário ao lado do AppImage para
/// garantir `rename(2)` atômico sem cruzar sistemas de arquivos.
#[cfg(target_os = "linux")]
fn is_appimage_parent_writable(target: &Path) -> bool {
    let Some(parent) = target.parent() else {
        return false;
    };
    let probe = parent.join(format!(".pulse-update-probe-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn validate_appimage_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_APPIMAGE_BYTES {
        return Err("O AppImage baixado excede o tamanho máximo permitido (250 MB).".to_owned());
    }
    if bytes.len() < MIN_APPIMAGE_BYTES {
        return Err("O AppImage baixado está incompleto ou corrompido.".to_owned());
    }
    // Todo AppImage é um executável ELF (assinatura 0x7F 'E' 'L' 'F').
    if bytes.get(0..4) != Some(b"\x7fELF") {
        return Err("O arquivo baixado não é um AppImage ELF válido do Linux.".to_owned());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn download_appimage_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(format!("Pulse/{}", current_version()))
        .build()
        .map_err(|error| format!("Não foi possível iniciar o download da atualização: {error}"))?;

    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("Falha ao baixar o AppImage do Pulse: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Falha ao baixar o AppImage (status HTTP {})",
            response.status().as_u16()
        ));
    }

    let mut bytes = Vec::new();
    let mut limited = response.take((MAX_APPIMAGE_BYTES as u64) + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Erro durante a leitura do AppImage: {error}"))?;

    validate_appimage_bytes(&bytes)?;
    Ok(bytes)
}

/// Substitui o arquivo do AppImage atomicamente: grava um arquivo temporário no
/// mesmo diretório, aplica bit de execução e troca via `rename(2)`.
#[cfg(any(target_os = "linux", test))]
pub(super) fn replace_appimage_atomically(
    target: &Path,
    version: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "Não foi possível determinar a pasta do AppImage atual.".to_owned())?;
    let sanitized_version: String = version
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '-')
        .collect();
    let temp_path = parent.join(format!(
        ".Pulse_{sanitized_version}.{}.AppImage.part",
        std::process::id()
    ));

    let mode = fs::metadata(target)
        .map(|meta| meta.permissions().mode() | 0o700)
        .unwrap_or(0o755);

    if let Err(error) = fs::write(&temp_path, bytes) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("Não foi possível gravar o novo AppImage: {error}"));
    }
    if let Err(error) = fs::set_permissions(&temp_path, fs::Permissions::from_mode(mode)) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Não foi possível marcar o novo AppImage como executável: {error}"
        ));
    }
    if let Err(error) = fs::rename(&temp_path, target) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Não foi possível substituir o AppImage atomicamente: {error}"
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn spawn_relaunch_and_exit(app: &AppHandle, appimage_path: &Path) -> Result<(), String> {
    let main_visible = app
        .get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    let mut cmd = Command::new(appimage_path);
    // Limpa variáveis internas do runtime do AppImage antigo para o novo
    // montar seu próprio squashfs sem herdar o ponto de montagem prestes a sair.
    for var in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
        cmd.env_remove(var);
    }
    cmd.arg(UPDATED_RELAUNCH_ARG);
    if !main_visible {
        cmd.arg("--minimized-to-tray");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Não foi possível reiniciar o novo AppImage: {error}"))?;

    std::thread::sleep(Duration::from_millis(150));
    app.exit(0);
    Ok(())
}

fn manual_update_message(remote_version: &str, appimage_readonly: bool) -> String {
    let url = release_page_url(remote_version);
    if cfg!(target_os = "macos") {
        // TODO(macos): validar em hardware.
        format!("Nova versão {remote_version} disponível para macOS. Baixe manualmente em: {url}")
    } else if appimage_readonly {
        format!(
            "Nova versão {remote_version} disponível. O AppImage atual está em uma pasta sem permissão de escrita; baixe em: {url}"
        )
    } else {
        format!(
            "Nova versão {remote_version} disponível. Para pacotes .deb/sistema, baixe a atualização em: {url}"
        )
    }
}

fn notify_manual_update(
    app: &AppHandle,
    config: SelfUpdateConfig,
    curr: String,
    remote_version: String,
    size_formatted: Option<String>,
    started: Instant,
    appimage_readonly: bool,
) -> SelfUpdateStatus {
    let message = manual_update_message(&remote_version, appimage_readonly);
    crate::debug::record_message(
        "self_update_check",
        started.elapsed().as_millis(),
        "ok",
        &message,
    );
    update_and_emit(
        Some(app),
        SelfUpdateStatus {
            enabled: config.enabled,
            current_version: curr,
            phase: "available".to_owned(),
            available_version: Some(remote_version),
            release_size_formatted: size_formatted,
            last_checked_at_unix_ms: config.last_checked_at_unix_ms,
            last_updated_to_version: config.last_updated_to_version,
            last_updated_from_version: config.last_updated_from_version,
            last_updated_at_unix_ms: config.last_updated_at_unix_ms,
            message: Some(message),
            error: None,
        },
    )
}

#[cfg(target_os = "linux")]
pub(super) fn install_or_notify(
    app: &AppHandle,
    manifest: &ReleaseManifest,
    mut config: SelfUpdateConfig,
    curr: String,
    remote_version: String,
    size_formatted: Option<String>,
    started: Instant,
) -> Result<SelfUpdateStatus, String> {
    let Some(appimage_path) = detect_appimage_path() else {
        return Ok(notify_manual_update(
            app,
            config,
            curr,
            remote_version,
            size_formatted,
            started,
            false,
        ));
    };

    if !is_appimage_parent_writable(&appimage_path) {
        return Ok(notify_manual_update(
            app,
            config,
            curr,
            remote_version,
            size_formatted,
            started,
            true,
        ));
    }

    let (download_url, expected_sha256) =
        select_and_validate_appimage_asset(manifest, &remote_version)?;

    update_and_emit(
        Some(app),
        SelfUpdateStatus {
            enabled: config.enabled,
            current_version: curr.clone(),
            phase: "downloading".to_owned(),
            available_version: Some(remote_version.clone()),
            release_size_formatted: size_formatted.clone(),
            last_checked_at_unix_ms: config.last_checked_at_unix_ms,
            last_updated_to_version: config.last_updated_to_version.clone(),
            last_updated_from_version: config.last_updated_from_version.clone(),
            last_updated_at_unix_ms: config.last_updated_at_unix_ms,
            message: Some(format!("Baixando Pulse {remote_version} (AppImage)…")),
            error: None,
        },
    );

    let bytes = match download_appimage_bytes(&download_url) {
        Ok(bytes) => bytes,
        Err(error) => {
            crate::debug::record(
                "self_update_download",
                started.elapsed().as_millis(),
                Err(&error),
            );
            config.last_error = Some(error.clone());
            let _ = save_config(&config);
            update_and_emit(
                Some(app),
                SelfUpdateStatus {
                    enabled: config.enabled,
                    current_version: curr,
                    phase: "error".to_owned(),
                    available_version: Some(remote_version),
                    release_size_formatted: size_formatted,
                    last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                    last_updated_to_version: config.last_updated_to_version,
                    last_updated_from_version: config.last_updated_from_version,
                    last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                    message: None,
                    error: Some(error.clone()),
                },
            );
            return Err(error);
        }
    };

    let verification_note = if let Some(expected) = expected_sha256 {
        update_and_emit(
            Some(app),
            SelfUpdateStatus {
                enabled: config.enabled,
                current_version: curr.clone(),
                phase: "verifying".to_owned(),
                available_version: Some(remote_version.clone()),
                release_size_formatted: size_formatted.clone(),
                last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                last_updated_to_version: config.last_updated_to_version.clone(),
                last_updated_from_version: config.last_updated_from_version.clone(),
                last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                message: Some("Verificando integridade SHA-256 do AppImage…".to_owned()),
                error: None,
            },
        );
        let actual = sha256_hex(&bytes);
        if actual != expected {
            let error = "O SHA-256 do AppImage baixado não confere com o manifesto oficial; atualização abortada.".to_owned();
            crate::debug::record(
                "self_update_verify",
                started.elapsed().as_millis(),
                Err(&error),
            );
            config.last_error = Some(error.clone());
            let _ = save_config(&config);
            update_and_emit(
                Some(app),
                SelfUpdateStatus {
                    enabled: config.enabled,
                    current_version: curr,
                    phase: "error".to_owned(),
                    available_version: Some(remote_version),
                    release_size_formatted: size_formatted,
                    last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                    last_updated_to_version: config.last_updated_to_version,
                    last_updated_from_version: config.last_updated_from_version,
                    last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                    message: None,
                    error: Some(error.clone()),
                },
            );
            return Err(error);
        }
        format!("SHA-256 validado ({actual})")
    } else {
        "cabeçalho ELF validado (manifesto sem SHA-256 para AppImage)".to_owned()
    };

    if let Err(error) = replace_appimage_atomically(&appimage_path, &remote_version, &bytes) {
        crate::debug::record(
            "self_update_install",
            started.elapsed().as_millis(),
            Err(&error),
        );
        config.last_error = Some(error.clone());
        let _ = save_config(&config);
        update_and_emit(
            Some(app),
            SelfUpdateStatus {
                enabled: config.enabled,
                current_version: curr,
                phase: "error".to_owned(),
                available_version: Some(remote_version),
                release_size_formatted: size_formatted,
                last_checked_at_unix_ms: config.last_checked_at_unix_ms,
                last_updated_to_version: config.last_updated_to_version,
                last_updated_from_version: config.last_updated_from_version,
                last_updated_at_unix_ms: config.last_updated_at_unix_ms,
                message: None,
                error: Some(error.clone()),
            },
        );
        return Err(error);
    }

    config.last_attempted_version = Some(remote_version.clone());
    config.last_attempt_at_unix_ms = Some(now_ms());
    config.last_updated_from_version = Some(curr.clone());
    config.last_error = None;
    let _ = save_config(&config);

    let installing_status = update_and_emit(
        Some(app),
        SelfUpdateStatus {
            enabled: config.enabled,
            current_version: curr,
            phase: "installing".to_owned(),
            available_version: Some(remote_version.clone()),
            release_size_formatted: size_formatted,
            last_checked_at_unix_ms: config.last_checked_at_unix_ms,
            last_updated_to_version: config.last_updated_to_version,
            last_updated_from_version: config.last_updated_from_version,
            last_updated_at_unix_ms: config.last_updated_at_unix_ms,
            message: Some(format!(
                "AppImage substituído para Pulse {remote_version}; reiniciando…"
            )),
            error: None,
        },
    );

    crate::debug::record_message(
        "self_update_install",
        started.elapsed().as_millis(),
        "ok",
        &format!("{verification_note}; AppImage substituído atomicamente para v{remote_version}"),
    );

    spawn_relaunch_and_exit(app, &appimage_path)?;
    Ok(installing_status)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn install_or_notify(
    app: &AppHandle,
    _manifest: &ReleaseManifest,
    config: SelfUpdateConfig,
    curr: String,
    remote_version: String,
    size_formatted: Option<String>,
    started: Instant,
) -> Result<SelfUpdateStatus, String> {
    Ok(notify_manual_update(
        app,
        config,
        curr,
        remote_version,
        size_formatted,
        started,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::self_update::{LinuxReleaseAssets, ReleaseInstaller};

    fn base_manifest(linux: Option<LinuxReleaseAssets>) -> ReleaseManifest {
        ReleaseManifest {
            version: "1.6.0".into(),
            installer: ReleaseInstaller {
                filename: Some("Pulse_1.6.0_x64-setup.exe".into()),
                url: None,
                asset_url: Some(
                    "https://github.com/LucasLagrimante/pulse/releases/download/v1.6.0/Pulse_1.6.0_x64-setup.exe"
                        .into(),
                ),
                size_bytes: Some(4_000_000),
                size_formatted: Some("3,8 MB".into()),
                sha256: "a".repeat(64),
            },
            updated_at: None,
            linux,
        }
    }

    #[test]
    fn derives_canonical_appimage_url_when_manifest_has_no_linux_section() {
        let manifest = base_manifest(None);
        let (url, sha) = select_and_validate_appimage_asset(&manifest, "v1.6.0").unwrap();
        assert_eq!(
            url,
            "https://github.com/LucasLagrimante/pulse/releases/download/v1.6.0/Pulse_1.6.0_amd64.AppImage"
        );
        assert!(sha.is_none());
    }

    #[test]
    fn validates_appimage_url_and_optional_sha_from_manifest() {
        let sha_hex = "420d69613d72db6e6059d226aa297d4af9fa29e410b357b5f98e51796f2f72bf";
        let manifest = base_manifest(Some(LinuxReleaseAssets {
            appimage: Some(ReleaseInstaller {
                filename: Some("Pulse_1.6.0_amd64.AppImage".into()),
                url: None,
                asset_url: Some(
                    "https://github.com/LucasLagrimante/pulse/releases/download/v1.6.0/Pulse_1.6.0_amd64.AppImage"
                        .into(),
                ),
                size_bytes: Some(15_000_000),
                size_formatted: Some("14,3 MB".into()),
                sha256: sha_hex.to_uppercase(),
            }),
            deb: None,
        }));
        let (url, sha) = select_and_validate_appimage_asset(&manifest, "1.6.0").unwrap();
        assert!(url.ends_with("Pulse_1.6.0_amd64.AppImage"));
        assert_eq!(sha.as_deref(), Some(sha_hex));

        // Domínio não oficial é rejeitado.
        let untrusted = base_manifest(Some(LinuxReleaseAssets {
            appimage: Some(ReleaseInstaller {
                filename: None,
                url: Some("https://evil.example/Pulse_1.6.0_amd64.AppImage".into()),
                asset_url: None,
                size_bytes: None,
                size_formatted: None,
                sha256: String::new(),
            }),
            deb: None,
        }));
        assert!(select_and_validate_appimage_asset(&untrusted, "1.6.0").is_err());

        // Extensão diferente de .AppImage é rejeitada.
        let wrong_ext = base_manifest(Some(LinuxReleaseAssets {
            appimage: Some(ReleaseInstaller {
                filename: None,
                url: None,
                asset_url: Some(
                    "https://github.com/LucasLagrimante/pulse/releases/download/v1.6.0/Pulse.sh"
                        .into(),
                ),
                size_bytes: None,
                size_formatted: None,
                sha256: String::new(),
            }),
            deb: None,
        }));
        assert!(select_and_validate_appimage_asset(&wrong_ext, "1.6.0").is_err());
    }

    #[test]
    fn validates_elf_header_and_size_bounds() {
        assert!(validate_appimage_bytes(b"\x7fELFcurto").is_err());
        let mut fake_elf = vec![0u8; MIN_APPIMAGE_BYTES];
        fake_elf[..4].copy_from_slice(b"\x7fELF");
        assert!(validate_appimage_bytes(&fake_elf).is_ok());

        let mut not_elf = vec![0u8; MIN_APPIMAGE_BYTES];
        not_elf[..2].copy_from_slice(b"MZ");
        assert!(validate_appimage_bytes(&not_elf).is_err());
    }

    #[test]
    fn replaces_appimage_atomically_and_preserves_executable_bit() {
        let dir = env::temp_dir().join(format!("pulse-appimage-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("Pulse.AppImage");
        fs::write(&target, b"versao-antiga").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

        let payload = b"\x7fELF-versao-nova";
        replace_appimage_atomically(&target, "1.6.0", payload).unwrap();

        assert_eq!(fs::read(&target).unwrap(), payload);
        let mode = fs::metadata(&target).unwrap().permissions().mode();
        assert_ne!(
            mode & 0o100,
            0,
            "o novo AppImage precisa continuar executável"
        );
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "nenhum .part deve sobrar após o rename"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manual_update_message_includes_release_link() {
        let msg = manual_update_message("1.6.0", false);
        assert!(msg.contains("1.6.0"));
        assert!(msg.contains("https://github.com/LucasLagrimante/pulse/releases/tag/v1.6.0"));
        let ro_msg = manual_update_message("1.6.0", true);
        assert!(ro_msg.contains("sem permissão de escrita"));
    }
}
