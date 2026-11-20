//! Remoção de programas fora do Windows, sempre pelo dono da instalação:
//!
//! - deb: `pkexec apt-get remove -y <pacote>` — **nunca** `purge` nem
//!   `autoremove`. Antes, `apt-get -s remove` (simulação, sem root): se o apt
//!   levaria qualquer outro pacote junto, a remoção é recusada com a lista;
//! - Flatpak: `flatpak uninstall -y --system|--user <ref>` (a instalação de
//!   sistema pede autorização pelo próprio polkit do Flatpak). Os dados em
//!   `~/.var/app` ficam para a etapa de resíduos (sem `--delete-data`);
//! - Snap: `pkexec snap remove <nome>` (sem `--purge`: o snapd guarda o
//!   snapshot automático dos dados);
//! - AppImage: o arquivo e o `.desktop`/ícone de integração que apontam
//!   exatamente para ele vão para a **Lixeira** (reversível);
//! - macOS: o `.app` vai para a Lixeira pelo Finder (`osascript`).
//!
//! Tudo é montado a partir do item do inventário (nunca de texto do IPC), com
//! nome/ref/caminho validados e argumentos fixos — nenhum shell no meio.
//! "Terminou" só é afirmado quando o item **sai do inventário**.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::installed::{self, InstalledApp};
use super::installed_unix::{
    find_in_path, parse_id, run_command, CommandOutput, SOURCE_APPIMAGE, SOURCE_DEB,
    SOURCE_FLATPAK, SOURCE_MAC_APP, SOURCE_SNAP,
};
use super::session::{REGISTRY_SETTLE_TIMEOUT, UNINSTALLER_TIMEOUT};

const SIMULATION_TIMEOUT: Duration = Duration::from_secs(90);
const PRESENCE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default)]
pub struct UnixRunReport {
    pub started: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub entry_removed: bool,
    pub detection_note: Option<String>,
    pub messages: Vec<String>,
}

struct Executed {
    exit_code: Option<i32>,
    timed_out: bool,
    messages: Vec<String>,
}

fn record_refusal(app: &InstalledApp, stage: &str, message: &str, detail: &str) {
    crate::debug::record_failure(
        "uninstall.run",
        0,
        crate::debug::Failure::new(message)
            .stage(stage)
            .detail(detail)
            .context(app.id.clone()),
    );
}

pub fn run(app: &InstalledApp) -> UnixRunReport {
    if let Some(reason) = &app.protected_reason {
        return not_started(format!("O Pulse não desinstala este item: {reason}."));
    }
    let Some(id) = parse_id(&app.id) else {
        return not_started("Identificador de programa inválido.".to_string());
    };
    let executed = match id.source.as_str() {
        SOURCE_DEB => run_deb(&id.key),
        SOURCE_FLATPAK => run_flatpak(&id.scope, &id.key),
        SOURCE_SNAP => run_snap(&id.key),
        SOURCE_APPIMAGE => run_appimage(&id.key),
        SOURCE_MAC_APP => run_mac_app(&id.key),
        other => Err(format!("Origem de instalação desconhecida ({other}).")),
    };
    let executed = match executed {
        Ok(executed) => executed,
        Err(note) => {
            record_refusal(app, "prepare", "Desinstalação não iniciada", &note);
            return not_started(note);
        }
    };
    if executed.exit_code != Some(0) {
        record_refusal(
            app,
            "command",
            "O comando de remoção não terminou com sucesso",
            &executed.messages.join(" | "),
        );
    }
    let entry_removed =
        !executed.timed_out && wait_until_gone(&app.id, executed.exit_code == Some(0));
    let detection_note = if entry_removed || executed.timed_out {
        None
    } else {
        Some(
            "O comando terminou, mas o programa continua no inventário. A limpeza de resíduos não foi recomendada.".to_string(),
        )
    };
    UnixRunReport {
        started: true,
        exit_code: executed.exit_code,
        timed_out: executed.timed_out,
        entry_removed,
        detection_note,
        messages: executed.messages,
    }
}

fn not_started(note: String) -> UnixRunReport {
    UnixRunReport {
        detection_note: Some(note),
        ..UnixRunReport::default()
    }
}

/// Prova de conclusão: o item sumiu na checagem barata **e** num inventário
/// novo. Com código de saída diferente de zero, uma checagem só.
fn wait_until_gone(app_id: &str, succeeded: bool) -> bool {
    let deadline = Instant::now()
        + if succeeded {
            REGISTRY_SETTLE_TIMEOUT.min(Duration::from_secs(20))
        } else {
            Duration::ZERO
        };
    loop {
        if !is_present(app_id) {
            installed::invalidate_cache();
            return !installed::collect_cached()
                .apps
                .iter()
                .any(|app| app.id == app_id);
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
}

/// Checagem barata de presença, sem reenumerar todas as origens. Na dúvida
/// (ferramenta não executou) responde "presente": não dá para provar remoção.
pub fn is_present(app_id: &str) -> bool {
    let Some(id) = parse_id(app_id) else {
        return false;
    };
    match id.source.as_str() {
        SOURCE_DEB => {
            let Some(dpkg_query) = find_in_path("dpkg-query") else {
                return true;
            };
            match run_command(
                &dpkg_query.to_string_lossy(),
                &["-W", "-f", "${db:Status-Abbrev}", &id.key],
                PRESENCE_TIMEOUT,
                true,
            ) {
                Ok(output) if output.success() => {
                    matches!(output.stdout.chars().nth(1), Some(state) if state != 'n' && state != 'c')
                }
                // Código 1 = pacote desconhecido para o dpkg.
                Ok(output) if output.status == Some(1) => false,
                _ => true,
            }
        }
        SOURCE_FLATPAK => {
            let (Some(flatpak), Some(flag)) =
                (find_in_path("flatpak"), flatpak_scope_flag(&id.scope))
            else {
                return true;
            };
            match run_command(
                &flatpak.to_string_lossy(),
                &["info", &flag, &id.key],
                PRESENCE_TIMEOUT,
                true,
            ) {
                Ok(output) => output.success() || output.timed_out,
                Err(_) => true,
            }
        }
        SOURCE_SNAP => {
            let Some(snap) = find_in_path("snap") else {
                return true;
            };
            match run_command(
                &snap.to_string_lossy(),
                &["list", &id.key],
                PRESENCE_TIMEOUT,
                true,
            ) {
                Ok(output) => output.success() || output.timed_out,
                Err(_) => true,
            }
        }
        SOURCE_APPIMAGE | SOURCE_MAC_APP => std::fs::symlink_metadata(&id.key).is_ok(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Validação de entrada (a chave vem do inventário, mas é revalidada aqui)
// ---------------------------------------------------------------------------

/// Nome de pacote Debian (+ `:arch` opcional): minúsculas, dígitos, `+-.`.
pub fn is_valid_deb_package(name: &str) -> bool {
    let (package, arch) = match name.split_once(':') {
        Some((package, arch)) => (package, Some(arch)),
        None => (name, None),
    };
    let package_ok = package.len() >= 2
        && package.len() <= 128
        && package
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && package
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'));
    let arch_ok = arch.is_none_or(|arch| {
        !arch.is_empty()
            && arch
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    });
    package_ok && arch_ok
}

/// `app/<id>/<arch>/<branch>` com caracteres de id/branch do Flatpak.
pub fn is_valid_flatpak_ref(reference: &str) -> bool {
    let parts: Vec<&str> = reference.split('/').collect();
    parts.len() == 4
        && parts[0] == "app"
        && parts[1..].iter().all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

pub fn flatpak_scope_flag(scope: &str) -> Option<String> {
    match scope {
        "system" => Some("--system".to_string()),
        "user" => Some("--user".to_string()),
        other
            if !other.is_empty()
                && other
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) =>
        {
            Some(format!("--installation={other}"))
        }
        _ => None,
    }
}

pub fn is_valid_snap_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Pacotes listados sob `The following packages have unmet dependencies:`.
/// Retorna `(nome_do_pacote, linha_completa)` preservando a ordem da saída.
pub fn parse_apt_unmet_dependencies(text: &str) -> Vec<(String, String)> {
    let mut in_section = false;
    let mut entries = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if !in_section {
            if trimmed == "The following packages have unmet dependencies:" {
                in_section = true;
            }
            continue;
        }
        if trimmed.is_empty()
            || (!raw.starts_with(' ') && !raw.starts_with('\t'))
            || trimmed.starts_with("E:")
        {
            break;
        }
        if let Some((pkg, _)) = trimmed.split_once(':') {
            let name = pkg.trim();
            if !name.is_empty() {
                entries.push((name.to_string(), trimmed.to_string()));
            }
        }
    }
    entries
}

/// Traduz uma recusa do `apt-get -s remove` combinando `stdout` e `stderr`:
/// o `apt-get` escreve os pacotes quebrados em `stdout` e só o resumo `E:` em
/// `stderr`, então olhar apenas `stderr` esconde qual pacote causou o bloqueio.
pub fn describe_apt_simulation_failure(package: &str, output: &CommandOutput) -> String {
    if output.timed_out {
        return "A simulação do apt ultrapassou o tempo limite; tente novamente.".to_string();
    }
    let combined_lower = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    if combined_lower.contains("dpkg was interrupted")
        || combined_lower.contains("dpkg --configure -a")
    {
        return "Uma operação anterior do dpkg foi interrompida. Execute `sudo dpkg --configure -a` no terminal para concluir a configuração pendente e tente novamente.".to_string();
    }

    let mut unmet = parse_apt_unmet_dependencies(&output.stdout);
    if unmet.is_empty() {
        unmet = parse_apt_unmet_dependencies(&output.stderr);
    }
    if !unmet.is_empty() || combined_lower.contains("unmet dependencies") {
        let target = bare(package);
        let mut names: Vec<&str> = Vec::new();
        for (name, _) in &unmet {
            if !names.contains(&name.as_str()) {
                names.push(name.as_str());
            }
        }
        let details: Vec<&str> = unmet
            .iter()
            .map(|(_, line)| line.as_str())
            .take(3)
            .collect();
        let more_details = if unmet.len() > details.len() {
            "…"
        } else {
            ""
        };

        if !names.is_empty() && !names.iter().any(|name| bare(name) == target) {
            let shown_names: Vec<&str> = names.iter().copied().take(4).collect();
            let more_names = if names.len() > shown_names.len() {
                "…"
            } else {
                ""
            };
            let list = format!("{}{more_names}", shown_names.join(", "));
            return format!(
                "O apt bloqueou a remoção porque há dependência quebrada em outro pacote do sistema ({list}): {}{more_details}. Corrija com `sudo apt --fix-broken install` no terminal (ou remova {list} antes) e tente novamente.",
                details.join("; "),
            );
        }
        if !details.is_empty() {
            return format!(
                "O apt recusou a remoção por dependências pendentes no sistema: {}{more_details}. Execute `sudo apt --fix-broken install` no terminal e tente novamente.",
                details.join("; "),
            );
        }
        return "O apt recusou a remoção por dependências quebradas no sistema. Execute `sudo apt --fix-broken install` no terminal e tente novamente.".to_string();
    }

    let tail = output.tail(3).join(" ");
    if tail.is_empty() {
        "O apt recusou a remoção na simulação sem informar detalhes.".to_string()
    } else {
        format!("O apt recusou a remoção na simulação: {tail}")
    }
}

/// Pacotes que o `apt-get -s remove` removeria (`Remv nome [versão]`).
pub fn parse_apt_simulation(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("Remv "))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn bare(name: &str) -> &str {
    name.split(':').next().unwrap_or(name)
}

/// Textos de saída para a UI. `126`/`127` são os códigos do pkexec para
/// autorização cancelada/negada.
pub fn describe_exit(tool: &str, output: &CommandOutput, via_pkexec: bool) -> Vec<String> {
    let mut messages = match output.status {
        Some(0) => vec!["A remoção terminou com sucesso".to_string()],
        Some(126) if via_pkexec => {
            vec!["A autorização de administrador foi cancelada; nada foi removido".to_string()]
        }
        Some(127) if via_pkexec => vec![
            "A autorização de administrador falhou (senha incorreta ou polkit indisponível)".to_string(),
        ],
        Some(code) => vec![format!("{tool} terminou com código {code}")],
        None if output.timed_out => vec![
            format!("{tool} continua em execução e o Pulse parou de aguardá-lo. Confira se a remoção terminou."),
        ],
        None => vec![format!("{tool} foi encerrado antes de terminar")],
    };
    if output.status != Some(0) && !output.timed_out {
        messages.extend(output.tail(3));
    }
    messages
}

fn pkexec() -> Result<String, String> {
    find_in_path("pkexec")
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| {
            "O pkexec (polkit) não está instalado; sem ele o Pulse não tem como pedir a senha de administrador.".to_string()
        })
}

fn execute(tool: &str, program: &str, args: &[&str], via_pkexec: bool) -> Result<Executed, String> {
    let output = run_command(program, args, UNINSTALLER_TIMEOUT, false)
        .map_err(|error| format!("Não foi possível executar {tool}: {error}"))?;
    Ok(Executed {
        exit_code: output.status,
        timed_out: output.timed_out,
        messages: describe_exit(tool, &output, via_pkexec),
    })
}

// ---------------------------------------------------------------------------
// Origens
// ---------------------------------------------------------------------------

fn run_deb(package: &str) -> Result<Executed, String> {
    if !is_valid_deb_package(package) {
        return Err("Nome de pacote inválido.".to_string());
    }
    let apt_get = find_in_path("apt-get")
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "apt-get não foi encontrado neste sistema.".to_string())?;
    let pkexec = pkexec()?;

    // Simulação sem root: mostra exatamente o que o apt faria.
    let simulation = run_command(
        &apt_get,
        &["-s", "remove", package],
        SIMULATION_TIMEOUT,
        true,
    )
    .map_err(|error| format!("A simulação do apt não pôde ser executada: {error}"))?;
    if !simulation.success() {
        return Err(describe_apt_simulation_failure(package, &simulation));
    }
    let removed = parse_apt_simulation(&simulation.stdout);
    if !removed.iter().any(|name| bare(name) == bare(package)) {
        return Err(format!("O apt informa que {package} não está instalado."));
    }
    let extras: Vec<&str> = removed
        .iter()
        .map(String::as_str)
        .filter(|name| bare(name) != bare(package))
        .collect();
    if !extras.is_empty() {
        let shown: Vec<&str> = extras.iter().copied().take(12).collect();
        return Err(format!(
            "Remover {package} também removeria {} outro(s) pacote(s): {}{}. O Pulse remove só o programa escolhido; use o gerenciador de pacotes se quiser remover o conjunto.",
            extras.len(),
            shown.join(", "),
            if extras.len() > shown.len() { "…" } else { "" }
        ));
    }

    let env = find_in_path("env")
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "/usr/bin/env".to_string());
    // `DEBIAN_FRONTEND=noninteractive`: sem terminal, uma pergunta do debconf
    // penduraria o processo esperando resposta que nunca chega.
    execute(
        "apt-get",
        &pkexec,
        &[
            &env,
            "DEBIAN_FRONTEND=noninteractive",
            &apt_get,
            "remove",
            "-y",
            package,
        ],
        true,
    )
}

fn run_flatpak(scope: &str, reference: &str) -> Result<Executed, String> {
    if !is_valid_flatpak_ref(reference) {
        return Err("Referência Flatpak inválida.".to_string());
    }
    let flag =
        flatpak_scope_flag(scope).ok_or_else(|| "Instalação Flatpak inválida.".to_string())?;
    let flatpak = find_in_path("flatpak")
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "flatpak não foi encontrado neste sistema.".to_string())?;
    execute(
        "flatpak",
        &flatpak,
        &["uninstall", "-y", "--noninteractive", &flag, reference],
        false,
    )
}

fn run_snap(name: &str) -> Result<Executed, String> {
    if !is_valid_snap_name(name) {
        return Err("Nome de snap inválido.".to_string());
    }
    let snap = find_in_path("snap")
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "snap não foi encontrado neste sistema.".to_string())?;
    let pkexec = pkexec()?;
    execute("snap", &pkexec, &[&snap, "remove", name], true)
}

#[cfg(target_os = "linux")]
fn run_appimage(raw_path: &str) -> Result<Executed, String> {
    use super::packages_linux::{
        appimage_dirs, appimage_integration_files, has_appimage_signature,
    };

    let home = dirs::home_dir().ok_or_else(|| "Pasta pessoal não encontrada.".to_string())?;
    let path = PathBuf::from(raw_path);
    validate_appimage_path(&path, &appimage_dirs(&home))?;
    if !has_appimage_signature(&path) {
        return Err(
            "O arquivo não tem mais a assinatura de AppImage; nada foi removido.".to_string(),
        );
    }
    let trash_root = user_trash_root(&home);
    let integrations = appimage_integration_files(&home, &path);
    move_to_trash_in(&path, &trash_root)?;

    let mut messages = vec!["O AppImage foi movido para a Lixeira".to_string()];
    let icons_root = home.join(".local/share/icons");
    for desktop in integrations {
        match move_to_trash_in(&desktop.path, &trash_root) {
            Ok(_) => messages.push(format!(
                "Atalho de integração removido: {}",
                desktop.path.display()
            )),
            Err(error) => messages.push(format!(
                "Atalho {} mantido: {error}",
                desktop.path.display()
            )),
        }
        // Ícone só sai quando é o gerado pela integração (`appimagekit_*`)
        // dentro de `~/.local/share/icons`: ícone com nome comum pode ser
        // compartilhado com outro programa.
        let Some(icon) = desktop.icon.as_deref().filter(|icon| icon.starts_with('/')) else {
            continue;
        };
        let icon_path = PathBuf::from(icon);
        let generated = icon_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("appimagekit_"));
        let is_file = std::fs::symlink_metadata(&icon_path).is_ok_and(|meta| meta.is_file());
        if generated
            && is_file
            && icon_path.starts_with(&icons_root)
            && move_to_trash_in(&icon_path, &trash_root).is_ok()
        {
            messages.push(format!(
                "Ícone de integração removido: {}",
                icon_path.display()
            ));
        }
    }
    Ok(Executed {
        exit_code: Some(0),
        timed_out: false,
        messages,
    })
}

#[cfg(not(target_os = "linux"))]
fn run_appimage(_raw_path: &str) -> Result<Executed, String> {
    Err("AppImage só existe no Linux.".to_string())
}

/// O AppImage precisa ser arquivo comum (não link) **diretamente** dentro
/// de uma das pastas varridas pelo inventário.
pub fn validate_appimage_path(path: &Path, allowed_dirs: &[PathBuf]) -> Result<(), String> {
    let text = path.to_string_lossy();
    if !path.is_absolute() || text.split('/').any(|part| part == ".." || part == ".") {
        return Err("Caminho de AppImage inválido.".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Caminho de AppImage inválido.".to_string())?;
    if !allowed_dirs.iter().any(|dir| dir == parent) {
        return Err("O AppImage não está em uma pasta analisada pelo Pulse.".to_string());
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "O AppImage não existe mais.".to_string())?;
    if !metadata.file_type().is_file() {
        return Err("O caminho não é mais um arquivo comum (virou link ou pasta).".to_string());
    }
    Ok(())
}

/// Lixeira da pasta pessoal (freedesktop): `$XDG_DATA_HOME/Trash`.
#[cfg(target_os = "linux")]
fn user_trash_root(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("Trash")
}

/// `Path=` do `.trashinfo`: URL-encoding de tudo fora do conjunto seguro.
pub fn trash_info_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut encoded = String::new();
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Move para a Lixeira seguindo a spec freedesktop: grava o `.trashinfo`
/// com `create_new` (reserva o nome sem corrida) e só então `rename`. Sem
/// cópia entre discos: `rename` que falha devolve erro e nada é apagado.
pub fn move_to_trash_in(path: &Path, trash_root: &Path) -> Result<PathBuf, String> {
    use std::io::Write;
    let files = trash_root.join("files");
    let info = trash_root.join("info");
    std::fs::create_dir_all(&files)
        .and_then(|_| std::fs::create_dir_all(&info))
        .map_err(|error| format!("A Lixeira não pôde ser preparada ({error})"))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| "Caminho sem nome de arquivo".to_string())?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        return Err("Caminho relativo não vai para a Lixeira".to_string());
    };
    for attempt in 0..1000u32 {
        let candidate = if attempt == 0 {
            file_name.clone()
        } else {
            format!("{file_name}.{attempt}")
        };
        let info_path = info.join(format!("{candidate}.trashinfo"));
        let target = files.join(&candidate);
        if std::fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        let mut handle = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&info_path)
        {
            Ok(handle) => handle,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("A Lixeira recusou o registro ({error})")),
        };
        let content = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            trash_info_path(&absolute),
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S")
        );
        if let Err(error) = handle.write_all(content.as_bytes()) {
            let _ = std::fs::remove_file(&info_path);
            return Err(format!("A Lixeira recusou o registro ({error})"));
        }
        drop(handle);
        return match std::fs::rename(&absolute, &target) {
            Ok(()) => Ok(target),
            Err(error) => {
                let _ = std::fs::remove_file(&info_path);
                if error.raw_os_error() == Some(libc::EXDEV) {
                    Err("O arquivo está em outro disco e a Lixeira da pasta pessoal não o recebe; remova-o pelo gerenciador de arquivos.".to_string())
                } else {
                    Err(format!("Não foi possível mover para a Lixeira ({error})"))
                }
            }
        };
    }
    Err("A Lixeira já tem itens demais com este nome".to_string())
}

/// `.app` direto em `/Applications` ou `~/Applications` (ou uma subpasta
/// abaixo delas), pasta real e não link.
pub fn validate_mac_app_path(path: &Path, roots: &[PathBuf]) -> Result<(), String> {
    let text = path.to_string_lossy();
    if !path.is_absolute()
        || text.split('/').any(|part| part == ".." || part == ".")
        || path.extension().is_none_or(|ext| ext != "app")
    {
        return Err("Caminho de aplicativo inválido.".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Caminho de aplicativo inválido.".to_string())?;
    let inside = roots
        .iter()
        .any(|root| parent == root || parent.parent() == Some(root.as_path()));
    if !inside {
        return Err("O aplicativo não está em /Applications nem em ~/Applications.".to_string());
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "O aplicativo não existe mais.".to_string())?;
    if !metadata.file_type().is_dir() {
        return Err("O caminho não é mais um pacote .app (virou link ou arquivo).".to_string());
    }
    Ok(())
}

/// TODO(macos): validar em hardware. O caminho vai como argumento do
/// AppleScript (`item 1 of argv`), nunca interpolado no texto do script.
fn run_mac_app(raw_path: &str) -> Result<Executed, String> {
    let path = PathBuf::from(raw_path);
    let roots: Vec<PathBuf> = super::apps_macos::app_roots(dirs::home_dir().as_deref())
        .into_iter()
        .map(|(root, _)| root)
        .collect();
    validate_mac_app_path(&path, &roots)?;
    if !Path::new("/usr/bin/osascript").exists() {
        return Err(
            "osascript não está disponível; mova o aplicativo para o Lixo pelo Finder.".to_string(),
        );
    }
    execute(
        "Finder",
        "/usr/bin/osascript",
        &[
            "-e",
            "on run argv",
            "-e",
            "set alvo to POSIX file (item 1 of argv)",
            "-e",
            "tell application \"Finder\" to delete alvo",
            "-e",
            "end run",
            raw_path,
        ],
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomes_de_pacote_deb() {
        assert!(is_valid_deb_package("firefox"));
        assert!(is_valid_deb_package("libreoffice-writer"));
        assert!(is_valid_deb_package("g++"));
        assert!(is_valid_deb_package("libc6:amd64"));
        assert!(!is_valid_deb_package("-y"));
        assert!(!is_valid_deb_package("Firefox"));
        assert!(!is_valid_deb_package("foo bar"));
        assert!(!is_valid_deb_package("foo;rm"));
        assert!(!is_valid_deb_package("x"));
        assert!(!is_valid_deb_package("foo:"));
    }

    #[test]
    fn referencias_flatpak_e_escopo() {
        assert!(is_valid_flatpak_ref("app/com.rtosta.zapzap/x86_64/stable"));
        assert!(!is_valid_flatpak_ref("runtime/org.kde.Platform/x86_64/6.7"));
        assert!(!is_valid_flatpak_ref("app/--system/x86_64/stable"));
        assert!(!is_valid_flatpak_ref("app/com.x/x86_64"));
        assert!(!is_valid_flatpak_ref("app/com x/x86_64/stable"));
        assert_eq!(flatpak_scope_flag("system").as_deref(), Some("--system"));
        assert_eq!(flatpak_scope_flag("user").as_deref(), Some("--user"));
        assert_eq!(
            flatpak_scope_flag("extra_1").as_deref(),
            Some("--installation=extra_1")
        );
        assert_eq!(flatpak_scope_flag("a b"), None);
        assert_eq!(flatpak_scope_flag(""), None);
    }

    #[test]
    fn nomes_de_snap() {
        assert!(is_valid_snap_name("firefox"));
        assert!(is_valid_snap_name("core22"));
        assert!(!is_valid_snap_name("--purge"));
        assert!(!is_valid_snap_name("Firefox"));
        assert!(!is_valid_snap_name(""));
    }

    #[test]
    fn simulacao_do_apt_lista_o_que_sairia() {
        let simulation = "NOTE: This is only a simulation!\n\
Reading package lists...\n\
The following packages will be REMOVED:\n  dolphin kde-baseapps\n\
Remv kde-baseapps [4:23.08]\n\
Remv dolphin [4:23.08.5-0ubuntu1]\n";
        assert_eq!(
            parse_apt_simulation(simulation),
            vec!["kde-baseapps".to_string(), "dolphin".to_string()]
        );
        assert!(parse_apt_simulation("0 upgraded, 0 newly installed, 0 to remove").is_empty());
    }

    #[test]
    fn falha_de_simulacao_do_apt_identifica_outro_pacote_quebrado() {
        let output = CommandOutput {
            status: Some(100),
            stdout: concat!(
                "Reading package lists...\n",
                "You might want to run 'apt --fix-broken install' to correct these.\n",
                "The following packages have unmet dependencies:\n",
                " handy : Depends: libopenblas0 but it is not going to be installed\n",
            )
            .to_string(),
            stderr:
                "E: Unmet dependencies. Try 'apt --fix-broken install' with no packages (or specify a solution).\n"
                    .to_string(),
            timed_out: false,
        };
        let message = describe_apt_simulation_failure("chatgpt", &output);
        assert!(message.contains("outro pacote do sistema (handy)"));
        assert!(message.contains("libopenblas0"));
        assert!(message.contains("sudo apt --fix-broken install"));

        let same_pkg = describe_apt_simulation_failure("handy", &output);
        assert!(!same_pkg.contains("outro pacote"));
        assert!(same_pkg.contains("libopenblas0"));

        let interrupted = CommandOutput {
            status: Some(100),
            stderr:
                "E: dpkg was interrupted, you must manually run 'sudo dpkg --configure -a' to correct the problem.\n"
                    .to_string(),
            ..CommandOutput::default()
        };
        assert!(describe_apt_simulation_failure("chatgpt", &interrupted)
            .contains("sudo dpkg --configure -a"));

        let timed_out = CommandOutput {
            timed_out: true,
            ..CommandOutput::default()
        };
        assert!(describe_apt_simulation_failure("chatgpt", &timed_out).contains("tempo limite"));
    }

    #[test]
    fn saida_do_pkexec_vira_mensagem_clara() {
        let cancelled = CommandOutput {
            status: Some(126),
            ..CommandOutput::default()
        };
        assert!(describe_exit("apt-get", &cancelled, true)[0].contains("cancelada"));
        let failed = CommandOutput {
            status: Some(100),
            stderr: "E: Could not get lock /var/lib/dpkg/lock-frontend\n".to_string(),
            ..CommandOutput::default()
        };
        let messages = describe_exit("apt-get", &failed, true);
        assert!(messages[0].contains("100"));
        assert!(messages.iter().any(|line| line.contains("lock")));
        let ok = CommandOutput {
            status: Some(0),
            ..CommandOutput::default()
        };
        assert_eq!(describe_exit("flatpak", &ok, false).len(), 1);
    }

    #[test]
    fn caminho_de_appimage_precisa_estar_na_pasta_varrida() {
        let base = std::env::temp_dir().join(format!("pulse-appimage-{}", std::process::id()));
        let apps = base.join("Applications");
        std::fs::create_dir_all(apps.join("sub")).unwrap();
        std::fs::write(apps.join("X.AppImage"), b"x").unwrap();
        std::fs::write(apps.join("sub/Y.AppImage"), b"y").unwrap();
        std::os::unix::fs::symlink(apps.join("X.AppImage"), apps.join("L.AppImage")).unwrap();
        let allowed = vec![apps.clone()];

        let ok = validate_appimage_path(&apps.join("X.AppImage"), &allowed);
        let nested = validate_appimage_path(&apps.join("sub/Y.AppImage"), &allowed);
        let link = validate_appimage_path(&apps.join("L.AppImage"), &allowed);
        let traversal = validate_appimage_path(&apps.join("sub/../X.AppImage"), &allowed);
        let missing = validate_appimage_path(&apps.join("Z.AppImage"), &allowed);
        std::fs::remove_dir_all(&base).ok();

        assert!(ok.is_ok());
        assert!(nested.is_err());
        assert!(link.is_err());
        assert!(traversal.is_err());
        assert!(missing.is_err());
    }

    #[test]
    fn caminho_de_app_do_macos_validado_sem_tocar_o_finder() {
        let base = std::env::temp_dir().join(format!("pulse-macapp-{}", std::process::id()));
        let apps = base.join("Applications");
        std::fs::create_dir_all(apps.join("Foto.app/Contents")).unwrap();
        std::fs::create_dir_all(apps.join("Fabricante/Editor.app")).unwrap();
        std::fs::create_dir_all(base.join("Outro/Fora.app")).unwrap();
        let roots = vec![apps.clone()];

        let ok = validate_mac_app_path(&apps.join("Foto.app"), &roots);
        let vendor = validate_mac_app_path(&apps.join("Fabricante/Editor.app"), &roots);
        let outside = validate_mac_app_path(&base.join("Outro/Fora.app"), &roots);
        let not_app = validate_mac_app_path(&apps.join("Fabricante"), &roots);
        std::fs::remove_dir_all(&base).ok();

        assert!(ok.is_ok());
        assert!(vendor.is_ok());
        assert!(outside.is_err());
        assert!(not_app.is_err());
    }

    #[test]
    fn lixeira_freedesktop_registra_origem_e_nao_sobrescreve() {
        let base = std::env::temp_dir().join(format!("pulse-trash-{}", std::process::id()));
        let trash = base.join("Trash");
        std::fs::create_dir_all(&base).unwrap();
        let first = base.join("Meu App.AppImage");
        std::fs::write(&first, b"1").unwrap();
        let moved = move_to_trash_in(&first, &trash).expect("mesmo disco");
        std::fs::write(&first, b"2").unwrap();
        let moved_again = move_to_trash_in(&first, &trash).expect("nome repetido ganha sufixo");
        let info = std::fs::read_to_string(trash.join("info/Meu App.AppImage.trashinfo")).unwrap();
        let first_exists = first.exists();
        let contents = (
            std::fs::read(&moved).unwrap(),
            std::fs::read(&moved_again).unwrap(),
        );
        std::fs::remove_dir_all(&base).ok();

        assert!(!first_exists);
        assert_ne!(moved, moved_again);
        assert_eq!(contents, (b"1".to_vec(), b"2".to_vec()));
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains("Meu%20App.AppImage"));
        assert!(info.contains("DeletionDate="));
    }

    #[test]
    fn id_desconhecido_nao_esta_presente() {
        assert!(!is_present("lixo"));
        assert!(!is_present(
            "appimage|user|/caminho/que/nao/existe.AppImage"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn presenca_real_de_pacote_deb_instalado_e_inexistente() {
        if find_in_path("dpkg-query").is_none() {
            return;
        }
        // `dpkg` é Essential em todo sistema baseado em Debian.
        assert!(is_present("deb|system|dpkg"));
        assert!(!is_present("deb|system|pacote-que-nao-existe-pulse"));
    }
}
