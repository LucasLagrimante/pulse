//! Atualizações pendentes no Linux: Flatpak (usuário e sistema), apt e Snap.
//!
//! ## Listagem
//!
//! - Flatpak: `flatpak remote-ls --updates --user|--system` (consulta os
//!   remotos, sem root) cruzado com `flatpak list` para a versão instalada.
//! - apt: `apt list --upgradable` com `LC_ALL=C`. **Não** roda `apt update`
//!   (exige root e muda o sistema): a lista reflete o último `apt update`, e a
//!   idade desse cache vai no rótulo da origem para o usuário saber o quão
//!   fresca ela é.
//! - Snap: `snap refresh --list` (quando instalado) cruzado com `snap list`
//!   para a versão instalada.
//!
//! ## Atualização
//!
//! - Flatpak: `flatpak update -y --noninteractive --user|--system <ref>`. O
//!   Flatpak pede autorização ao polkit sozinho quando a instalação é do
//!   sistema; a repetição "como administrador" usa `pkexec`.
//! - apt: `pkexec env DEBIAN_FRONTEND=noninteractive apt-get install
//!   --only-upgrade -y <pacote>` com `--force-confold` (mantém arquivos de
//!   configuração editados pelo usuário em vez de travar num prompt).
//! - Snap: `pkexec snap refresh <pacote>`.
//! - "Atualizar tudo" agrupa: **uma** senha para todos os pacotes apt, uma
//!   para todos os snaps e uma chamada por instalação Flatpak, em vez de um
//!   pedido de senha por pacote.
//!
//! ## Identificadores
//!
//! `flatpak:user:<ref>`, `flatpak:system:<ref>`, `apt:<pacote>` e
//! `snap:<pacote>`. Cada parte é validada por caractere antes de virar
//! argumento de processo.

use super::{
    emit_progress, join_readers, spawn_output_reader, truncate_chars, unavailable,
    wait_for_upgrade, BatchFailure, BatchUpgradeReport, PendingUpdate, UpdatesSnapshot,
    UpgradeFailure, ALL_PACKAGES_ID, UPGRADE_OUTPUT_LIMIT,
};
use crate::services::command;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tauri::AppHandle;

const LIST_TIMEOUT: Duration = Duration::from_secs(90);

/// Instalação Flatpak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installation {
    User,
    System,
}

impl Installation {
    fn flag(self) -> &'static str {
        match self {
            Installation::User => "--user",
            Installation::System => "--system",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Installation::User => "user",
            Installation::System => "system",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Installation::User => "Flatpak (usuário)",
            Installation::System => "Flatpak (sistema)",
        }
    }
}

/// Pacote alvo de uma atualização, já validado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Flatpak(Installation, String),
    Apt(String),
    Snap(String),
}

/// Ref do Flatpak: `app/org.x.Y/x86_64/stable` ou `org.x.Y//stable`.
fn valid_flatpak_ref(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// Nome de pacote Debian (com qualificador de arquitetura opcional).
fn valid_apt_package(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && text.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ':')
        })
}

/// Nome de pacote Snap (minúsculas, dígitos e hífen; não começa nem termina por `-`).
fn valid_snap_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 40
        && !text.starts_with('-')
        && !text.ends_with('-')
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn parse_target(id: &str) -> Result<Target, String> {
    let invalid = || "Identificador de pacote inválido".to_string();
    if let Some(rest) = id.strip_prefix("flatpak:") {
        let (installation, reference) = rest.split_once(':').ok_or_else(invalid)?;
        let installation = match installation {
            "user" => Installation::User,
            "system" => Installation::System,
            _ => return Err(invalid()),
        };
        if !valid_flatpak_ref(reference) {
            return Err(invalid());
        }
        return Ok(Target::Flatpak(installation, reference.to_string()));
    }
    if let Some(package) = id.strip_prefix("apt:") {
        if !valid_apt_package(package) {
            return Err(invalid());
        }
        return Ok(Target::Apt(package.to_string()));
    }
    if let Some(name) = id.strip_prefix("snap:") {
        if !valid_snap_name(name) {
            return Err(invalid());
        }
        return Ok(Target::Snap(name.to_string()));
    }
    Err(invalid())
}

/* ------------------------------------------------------------------ */
/* Parsers                                                             */
/* ------------------------------------------------------------------ */

/// `flatpak remote-ls --updates --columns=application,name,version,ref` (TSV).
pub fn parse_flatpak_updates(
    output: &str,
    installation: Installation,
    installed_versions: &HashMap<String, String>,
) -> Vec<PendingUpdate> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
            if fields.len() < 4 {
                return None;
            }
            let (application, name, version, reference) =
                (fields[0], fields[1], fields[2], fields[3]);
            if application.is_empty() || !valid_flatpak_ref(reference) {
                return None;
            }
            let branch = reference.rsplit('/').next().unwrap_or("");
            let current = installed_versions
                .get(reference)
                .cloned()
                .filter(|v| !v.is_empty())
                // Runtimes de driver (GL.nvidia) não têm versão: o branch é o
                // que identifica o que está instalado.
                .unwrap_or_else(|| format!("branch {branch}"));
            let available = if version.is_empty() {
                format!("branch {branch} (nova compilação)")
            } else {
                version.to_string()
            };
            Some(PendingUpdate {
                id: format!("flatpak:{}:{reference}", installation.key()),
                name: if name.is_empty() {
                    application.to_string()
                } else {
                    name.to_string()
                },
                current_version: current,
                available_version: available,
                source: installation.label().to_string(),
                icon: None,
            })
        })
        .collect()
}

/// `flatpak list --columns=ref,version` → ref → versão.
pub fn parse_flatpak_installed(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let reference = fields.next()?.trim();
            let version = fields.next().unwrap_or("").trim();
            (!reference.is_empty()).then(|| (reference.to_string(), version.to_string()))
        })
        .collect()
}

/// `apt list --upgradable`: `nome/suites versão arch [upgradable from: antiga]`.
///
/// Em sistemas multiarch (`amd64` + `i386`), o mesmo nome pode aparecer duas
/// vezes com arquiteturas diferentes; nesse caso o ID e o nome recebem o
/// qualificador de arquitetura (`apt:libgbm1:i386`) para não colidirem.
pub fn parse_apt_upgradable(output: &str, source: &str) -> Vec<PendingUpdate> {
    let rows: Vec<(&str, &str, &str, &str)> = output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (package, rest) = line.split_once('/')?;
            if !valid_apt_package(package) {
                return None;
            }
            let mut tokens = rest.split_whitespace();
            let _suites = tokens.next()?;
            let available = tokens.next()?;
            let arch = tokens.next().unwrap_or("");
            let current = line
                .split_once("[upgradable from: ")
                .and_then(|(_, tail)| tail.strip_suffix(']'))
                .unwrap_or("")
                .trim();
            if current.is_empty() {
                return None;
            }
            Some((package, arch, current, available))
        })
        .collect();

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (package, _, _, _) in &rows {
        *counts.entry(package).or_insert(0) += 1;
    }

    rows.into_iter()
        .map(|(package, arch, current, available)| {
            let needs_arch = counts.get(package).copied().unwrap_or(0) > 1
                && !package.contains(':')
                && !arch.is_empty()
                && arch != "all"
                && valid_apt_package(&format!("{package}:{arch}"));
            let (id, name) = if needs_arch {
                (
                    format!("apt:{package}:{arch}"),
                    format!("{package} ({arch})"),
                )
            } else {
                (format!("apt:{package}"), package.to_string())
            };
            PendingUpdate {
                id,
                name,
                current_version: current.to_string(),
                available_version: available.to_string(),
                source: source.to_string(),
                icon: None,
            }
        })
        .collect()
}

/// `snap list`: cabeçalho `Name  Version  Rev  Tracking  Publisher  Notes` → nome → versão.
pub fn parse_snap_installed(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let version = fields.next()?;
            if name == "Name" || !valid_snap_name(name) {
                return None;
            }
            Some((name.to_string(), version.to_string()))
        })
        .collect()
}

/// `snap refresh --list`: `Name  Version  Rev  Size  Publisher  Notes`.
pub fn parse_snap_updates(
    output: &str,
    installed_versions: &HashMap<String, String>,
) -> Vec<PendingUpdate> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let available = fields.next()?;
            let _rev = fields.next()?;
            if name == "Name" || !valid_snap_name(name) {
                return None;
            }
            let current = installed_versions
                .get(name)
                .cloned()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Indisponível".to_string());
            Some(PendingUpdate {
                id: format!("snap:{name}"),
                name: name.to_string(),
                current_version: current,
                available_version: available.to_string(),
                source: "Snap".to_string(),
                icon: None,
            })
        })
        .collect()
}

/// Idade legível do cache do apt.
pub fn describe_age(age: Duration) -> String {
    let hours = age.as_secs() / 3600;
    match hours {
        0 => "atualizada há menos de 1 hora".to_string(),
        1 => "atualizada há 1 hora".to_string(),
        2..=47 => format!("atualizada há {hours} horas"),
        _ => format!("atualizada há {} dias", hours / 24),
    }
}

/// Rótulo da origem apt com a idade da lista de pacotes. A data de modificação
/// do diretório `/var/lib/apt/lists` muda a cada `apt update` (os arquivos
/// são renomeados para dentro dele); a dos arquivos não serve porque o apt
/// copia o `Last-Modified` do servidor.
fn apt_source_label() -> String {
    std::fs::metadata("/var/lib/apt/lists")
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|age| format!("apt (lista de pacotes {})", describe_age(age)))
        .unwrap_or_else(|| "apt (idade da lista de pacotes indisponível)".to_string())
}

/* ------------------------------------------------------------------ */
/* Listagem                                                            */
/* ------------------------------------------------------------------ */

fn flatpak_updates(installation: Installation) -> Result<Vec<PendingUpdate>, String> {
    let mut installed = command::c_locale_command("flatpak");
    installed.args(["list", installation.flag(), "--columns=ref,version"]);
    let versions = command::run(installed, LIST_TIMEOUT)
        .ok()
        .filter(|output| output.success())
        .map(|output| parse_flatpak_installed(&output.stdout))
        .unwrap_or_default();
    if versions.is_empty() {
        // Nada instalado nesta instalação: nada a atualizar (e sem rede à toa).
        return Ok(Vec::new());
    }
    let mut remote = command::c_locale_command("flatpak");
    remote.args([
        "remote-ls",
        "--updates",
        installation.flag(),
        "--columns=application,name,version,ref",
    ]);
    let output = command::run(remote, LIST_TIMEOUT)?;
    if !output.success() {
        return Err(format!(
            "{}: {}",
            installation.label(),
            output
                .first_error_line()
                .unwrap_or_else(|| format!("código {:?}", output.code))
        ));
    }
    Ok(parse_flatpak_updates(
        &output.stdout,
        installation,
        &versions,
    ))
}

fn apt_updates() -> Result<Vec<PendingUpdate>, String> {
    let mut list = command::c_locale_command("apt");
    list.args(["list", "--upgradable"]);
    let output = command::run(list, LIST_TIMEOUT)?;
    if !output.success() {
        return Err(format!(
            "apt: {}",
            output
                .first_error_line()
                .unwrap_or_else(|| format!("código {:?}", output.code))
        ));
    }
    Ok(parse_apt_upgradable(&output.stdout, &apt_source_label()))
}

fn snap_updates() -> Result<Vec<PendingUpdate>, String> {
    let mut refresh = command::c_locale_command("snap");
    refresh.args(["refresh", "--list"]);
    let output = command::run(refresh, LIST_TIMEOUT)?;
    if !output.success() {
        return Err(format!(
            "Snap: {}",
            output
                .first_error_line()
                .unwrap_or_else(|| format!("código {:?}", output.code))
        ));
    }
    if output.stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut installed_cmd = command::c_locale_command("snap");
    installed_cmd.arg("list");
    let installed = command::run(installed_cmd, LIST_TIMEOUT)
        .ok()
        .filter(|out| out.success())
        .map(|out| parse_snap_installed(&out.stdout))
        .unwrap_or_default();
    Ok(parse_snap_updates(&output.stdout, &installed))
}

pub fn collect() -> UpdatesSnapshot {
    let started = std::time::Instant::now();
    let has_flatpak = command::program_exists("flatpak");
    let has_apt = command::program_exists("apt") && command::program_exists("apt-get");
    let has_snap = command::program_exists("snap");
    if !has_flatpak && !has_apt && !has_snap {
        return unavailable(
            "Nenhum gerenciador de pacotes suportado (apt, Flatpak ou Snap) foi encontrado neste sistema",
        );
    }

    let mut updates = Vec::new();
    let mut failures = Vec::new();
    let mut sources_ok = 0;
    if has_flatpak {
        for installation in [Installation::User, Installation::System] {
            match flatpak_updates(installation) {
                Ok(found) => {
                    sources_ok += 1;
                    updates.extend(found);
                }
                Err(error) => failures.push(error),
            }
        }
    }
    if has_apt {
        match apt_updates() {
            Ok(found) => {
                sources_ok += 1;
                updates.extend(found);
            }
            Err(error) => failures.push(error),
        }
    }
    if has_snap {
        match snap_updates() {
            Ok(found) => {
                sources_ok += 1;
                updates.extend(found);
            }
            Err(error) => failures.push(error),
        }
    }

    if !failures.is_empty() {
        crate::debug::record_failure(
            "updates_list_linux",
            started.elapsed().as_millis(),
            crate::debug::Failure::new("Parte das fontes de atualização falhou")
                .stage("list")
                .detail(failures.join(" | ")),
        );
    }
    if sources_ok == 0 {
        return unavailable(format!(
            "Não foi possível consultar as atualizações: {}",
            failures.join("; ")
        ));
    }

    let total = updates.len();
    UpdatesSnapshot {
        updates,
        total_count: total,
        unavailable_reason: None,
        collected_at_unix_ms: super::now_unix_ms(),
    }
}

/* ------------------------------------------------------------------ */
/* Atualização                                                         */
/* ------------------------------------------------------------------ */

fn pkexec() -> Result<String, UpgradeFailure> {
    let path = command::resolve_program("pkexec", &["/usr/bin/pkexec", "/bin/pkexec"]);
    if std::path::Path::new(&path).is_file() || command::program_exists("pkexec") {
        Ok(path)
    } else {
        Err(UpgradeFailure::plain(
            "Atualizar pacotes do sistema exige o pkexec (polkit), que não foi encontrado",
        ))
    }
}

fn apt_command(packages: &[String]) -> Result<std::process::Command, UpgradeFailure> {
    let mut cmd = command::c_locale_command(&pkexec()?);
    cmd.args([
        "/usr/bin/env",
        "DEBIAN_FRONTEND=noninteractive",
        "LC_ALL=C",
        "apt-get",
        "install",
        "--only-upgrade",
        "-y",
        "-o",
        "Dpkg::Options::=--force-confdef",
        "-o",
        "Dpkg::Options::=--force-confold",
        "--",
    ])
    .args(packages);
    Ok(cmd)
}

fn flatpak_command(
    installation: Installation,
    refs: &[String],
    elevated: bool,
) -> Result<std::process::Command, UpgradeFailure> {
    let mut cmd = if elevated && installation == Installation::System {
        let mut cmd = command::c_locale_command(&pkexec()?);
        cmd.arg(command::resolve_program(
            "flatpak",
            &["/usr/bin/flatpak", "/bin/flatpak"],
        ));
        cmd
    } else {
        command::c_locale_command("flatpak")
    };
    cmd.args([
        "update",
        "-y",
        "--noninteractive",
        installation.flag(),
        "--",
    ])
    .args(refs);
    Ok(cmd)
}

fn snap_command(names: &[String]) -> Result<std::process::Command, UpgradeFailure> {
    let mut cmd = command::c_locale_command(&pkexec()?);
    cmd.arg(command::resolve_program(
        "snap",
        &["/usr/bin/snap", "/snap/bin/snap"],
    ))
    .arg("refresh")
    .args(names);
    Ok(cmd)
}

/// Traduz a falha do pkexec/apt/flatpak.
pub fn describe_failure(code: Option<i32>, output: &str, used_pkexec: bool) -> UpgradeFailure {
    let detail = useful_detail(output);
    let lower = output.to_lowercase();
    let (message, needs_admin) = if used_pkexec && command::describe_pkexec_exit(code).is_some() {
        (
            command::describe_pkexec_exit(code)
                .unwrap_or_default()
                .to_string(),
            false,
        )
    } else if lower.contains("could not get lock")
        || lower.contains("unable to acquire the dpkg frontend lock")
    {
        (
            "Outra instalação está em andamento (o apt está bloqueado); tente mais tarde"
                .to_string(),
            false,
        )
    } else if lower.contains("unable to locate package") {
        (
            "O apt não encontrou o pacote; a lista de pacotes pode estar desatualizada".to_string(),
            false,
        )
    } else if lower.contains("no space left") {
        ("Sem espaço livre em disco para instalar".to_string(), false)
    } else if lower.contains("not allowed")
        || lower.contains("not authorized")
        || lower.contains("permission denied")
    {
        (
            "A atualização exige autorização de administrador".to_string(),
            true,
        )
    } else if lower.contains("dpkg was interrupted") {
        ("Uma instalação anterior foi interrompida; rode `sudo dpkg --configure -a` num terminal".to_string(), false)
    } else {
        match code {
            Some(code) => (format!("A atualização falhou com o código {code}"), false),
            None => (
                "O processo de atualização foi encerrado sem código de saída".to_string(),
                false,
            ),
        }
    };
    UpgradeFailure {
        message,
        code: code.map(|c| c.to_string()),
        needs_admin,
        detail,
    }
}

/// Últimas 3 linhas que explicam algo (sem progresso de download).
fn useful_detail(output: &str) -> Option<String> {
    let useful: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("Get:") && !line.starts_with("Hit:"))
        .filter(|line| !line.ends_with('%'))
        .collect();
    if useful.is_empty() {
        return None;
    }
    let start = useful.len().saturating_sub(3);
    Some(useful[start..].join("\n"))
}

/// Roda o comando emitindo cada linha como progresso de `progress_id`.
fn run_streaming(
    mut cmd: std::process::Command,
    app: &AppHandle,
    progress_id: &str,
    started_line: &str,
    used_pkexec: bool,
) -> Result<String, UpgradeFailure> {
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            UpgradeFailure::plain(format!("Não foi possível iniciar a atualização: {error}"))
        })?;
    emit_progress(app, progress_id, "started", started_line);
    let mut readers = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        readers.push(spawn_output_reader(
            pipe,
            app.clone(),
            progress_id.to_string(),
        ));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(spawn_output_reader(
            pipe,
            app.clone(),
            progress_id.to_string(),
        ));
    }
    let status = wait_for_upgrade(&mut child, app, progress_id)?;
    let output = join_readers(readers);
    if !status.success() {
        let failure = describe_failure(status.code(), &output, used_pkexec);
        emit_progress(app, progress_id, "failed", &failure.message);
        return Err(failure);
    }
    Ok(truncate_chars(&output, UPGRADE_OUTPUT_LIMIT))
}

pub fn upgrade(id: &str, app: &AppHandle, elevated: bool) -> Result<String, UpgradeFailure> {
    let target = parse_target(id).map_err(UpgradeFailure::plain)?;
    let result = match &target {
        Target::Apt(package) => run_streaming(
            apt_command(std::slice::from_ref(package))?,
            app,
            id,
            "Aguardando a senha de administrador para o apt…",
            true,
        ),
        Target::Flatpak(installation, reference) => {
            let use_pkexec = elevated && *installation == Installation::System;
            run_streaming(
                flatpak_command(*installation, std::slice::from_ref(reference), elevated)?,
                app,
                id,
                if use_pkexec {
                    "Aguardando a senha de administrador para o Flatpak…"
                } else {
                    "Atualização iniciada pelo Flatpak"
                },
                use_pkexec,
            )
        }
        Target::Snap(name) => run_streaming(
            snap_command(std::slice::from_ref(name))?,
            app,
            id,
            "Aguardando a senha de administrador para o Snap…",
            true,
        ),
    };
    if result.is_ok() {
        emit_progress(app, id, "finished", "Concluído");
    }
    result
}

/// "Atualizar tudo": apt numa só chamada (uma senha), Snap numa só chamada e
/// Flatpak por instalação.
pub fn upgrade_all(
    updates: &[PendingUpdate],
    app: &AppHandle,
) -> Result<BatchUpgradeReport, UpgradeFailure> {
    let total = updates.len();
    emit_progress(
        app,
        ALL_PACKAGES_ID,
        "started",
        &format!("{total} pacotes na fila de atualização"),
    );

    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    let mut groups: Vec<(&str, Vec<&PendingUpdate>, Vec<String>)> = Vec::new();

    let mut apt: Vec<&PendingUpdate> = Vec::new();
    let mut flatpak_user: Vec<&PendingUpdate> = Vec::new();
    let mut flatpak_system: Vec<&PendingUpdate> = Vec::new();
    let mut snap: Vec<&PendingUpdate> = Vec::new();
    for update in updates {
        match parse_target(&update.id) {
            Ok(Target::Apt(_)) => apt.push(update),
            Ok(Target::Flatpak(Installation::User, _)) => flatpak_user.push(update),
            Ok(Target::Flatpak(Installation::System, _)) => flatpak_system.push(update),
            Ok(Target::Snap(_)) => snap.push(update),
            Err(message) => failed.push(BatchFailure {
                id: update.id.clone(),
                name: update.name.clone(),
                failure: UpgradeFailure::plain(message),
            }),
        }
    }
    for (label, members) in [
        ("Flatpak (usuário)", flatpak_user),
        ("Flatpak (sistema)", flatpak_system),
        ("Snap", snap),
        ("apt", apt),
    ] {
        if members.is_empty() {
            continue;
        }
        let args: Vec<String> = members
            .iter()
            .filter_map(|update| match parse_target(&update.id) {
                Ok(Target::Apt(package)) => Some(package),
                Ok(Target::Flatpak(_, reference)) => Some(reference),
                Ok(Target::Snap(name)) => Some(name),
                Err(_) => None,
            })
            .collect();
        groups.push((label, members, args));
    }

    for (label, members, args) in groups {
        emit_progress(
            app,
            ALL_PACKAGES_ID,
            "log",
            &format!("Atualizando {} pacote(s) via {label}…", members.len()),
        );
        let outcome = match label {
            "apt" => apt_command(&args).and_then(|cmd| {
                run_streaming(
                    cmd,
                    app,
                    ALL_PACKAGES_ID,
                    "Aguardando a senha de administrador para o apt…",
                    true,
                )
            }),
            "Snap" => snap_command(&args).and_then(|cmd| {
                run_streaming(
                    cmd,
                    app,
                    ALL_PACKAGES_ID,
                    "Aguardando a senha de administrador para o Snap…",
                    true,
                )
            }),
            "Flatpak (usuário)" => {
                flatpak_command(Installation::User, &args, false).and_then(|cmd| {
                    run_streaming(
                        cmd,
                        app,
                        ALL_PACKAGES_ID,
                        "Atualizando Flatpak do usuário",
                        false,
                    )
                })
            }
            _ => flatpak_command(Installation::System, &args, false).and_then(|cmd| {
                run_streaming(
                    cmd,
                    app,
                    ALL_PACKAGES_ID,
                    "Atualizando Flatpak do sistema",
                    false,
                )
            }),
        };
        match outcome {
            Ok(_) => succeeded.extend(members.iter().map(|update| update.id.clone())),
            Err(failure) => failed.extend(members.iter().map(|update| BatchFailure {
                id: update.id.clone(),
                name: update.name.clone(),
                failure: failure.clone(),
            })),
        }
    }

    emit_progress(
        app,
        ALL_PACKAGES_ID,
        "finished",
        &format!("{} de {total} pacotes atualizados", succeeded.len()),
    );
    Ok(BatchUpgradeReport {
        total,
        succeeded,
        failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apt_upgradable() {
        let text = "Listing...\n\
beekeeper-studio/unknown 6.3.3 amd64 [upgradable from: 6.3.2]\n\
gir1.2-gstreamer-1.0/noble-updates,noble-security 1.24.2-1ubuntu0.2 amd64 [upgradable from: 1.24.2-1ubuntu0.1]\n\
libc6:i386/noble-updates 2.39-0ubuntu8.6 i386 [upgradable from: 2.39-0ubuntu8.5]\n\
libgbm1/noble-updates 25.2.8-0ubuntu0.24.04.5 amd64 [upgradable from: 25.2.8-0ubuntu0.24.04.4]\n\
libgbm1/noble-updates 25.2.8-0ubuntu0.24.04.5 i386 [upgradable from: 25.2.8-0ubuntu0.24.04.4]\n\
WARNING: apt does not have a stable CLI interface.\n";
        let updates = parse_apt_upgradable(text, "apt");
        assert_eq!(updates.len(), 5);
        assert_eq!(updates[0].id, "apt:beekeeper-studio");
        assert_eq!(updates[0].current_version, "6.3.2");
        assert_eq!(updates[0].available_version, "6.3.3");
        assert_eq!(updates[1].available_version, "1.24.2-1ubuntu0.2");
        assert_eq!(updates[2].id, "apt:libc6:i386");
        // Pacotes multiarch recebem qualificador de arquitetura para evitar IDs duplicados.
        assert_eq!(updates[3].id, "apt:libgbm1:amd64");
        assert_eq!(updates[3].name, "libgbm1 (amd64)");
        assert_eq!(updates[4].id, "apt:libgbm1:i386");
        assert_eq!(updates[4].name, "libgbm1 (i386)");
    }

    #[test]
    fn parses_snap_updates_with_installed_versions() {
        let installed = parse_snap_installed(
            "Name       Version    Rev    Tracking       Publisher   Notes\n\
firefox    133.0-1    5361   latest/stable  mozilla✓    -\n\
code       1.95.2     174    latest/stable  vscode✓     classic\n",
        );
        let refresh = "Name      Version    Rev   Size   Publisher   Notes\n\
firefox   133.0.3-1  5437  284MB  mozilla✓    -\n\
htop      3.3.0      4412  3MB    maxiberta   -\n";
        let updates = parse_snap_updates(refresh, &installed);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].id, "snap:firefox");
        assert_eq!(updates[0].current_version, "133.0-1");
        assert_eq!(updates[0].available_version, "133.0.3-1");
        assert_eq!(updates[0].source, "Snap");
        assert_eq!(updates[1].id, "snap:htop");
        assert_eq!(updates[1].current_version, "Indisponível");
    }

    #[test]
    fn parses_flatpak_updates_with_installed_versions() {
        let installed = parse_flatpak_installed(
            "app/com.rtosta.zapzap/x86_64/stable\t7.5.0\nruntime/org.freedesktop.Platform.GL.nvidia-580-178-04/x86_64/1.4\t\n",
        );
        let text = "com.rtosta.zapzap\tZapZap\t7.5.1\tapp/com.rtosta.zapzap/x86_64/stable\n\
org.freedesktop.Platform.GL.nvidia-580-178-04\tnvidia-580-178-04\t\truntime/org.freedesktop.Platform.GL.nvidia-580-178-04/x86_64/1.4\n";
        let updates = parse_flatpak_updates(text, Installation::System, &installed);
        assert_eq!(updates.len(), 2);
        assert_eq!(
            updates[0].id,
            "flatpak:system:app/com.rtosta.zapzap/x86_64/stable"
        );
        assert_eq!(updates[0].current_version, "7.5.0");
        assert_eq!(updates[0].available_version, "7.5.1");
        assert_eq!(updates[0].source, "Flatpak (sistema)");
        assert_eq!(updates[1].current_version, "branch 1.4");
        assert!(updates[1].available_version.contains("nova compilação"));
    }

    #[test]
    fn validates_targets() {
        assert_eq!(
            parse_target("apt:libc6:i386").unwrap(),
            Target::Apt("libc6:i386".into())
        );
        assert_eq!(
            parse_target("flatpak:user:app/org.x.Y/x86_64/stable").unwrap(),
            Target::Flatpak(Installation::User, "app/org.x.Y/x86_64/stable".into())
        );
        assert_eq!(
            parse_target("snap:firefox-esr").unwrap(),
            Target::Snap("firefox-esr".into())
        );
        assert!(parse_target("apt:-oAPT::x=1").is_err());
        assert!(parse_target("apt:foo bar").is_err());
        assert!(parse_target("apt:Foo;rm").is_err());
        assert!(parse_target("flatpak:root:app/x").is_err());
        assert!(parse_target("flatpak:user:--help").is_err());
        assert!(parse_target("snap:--classic").is_err());
        assert!(parse_target("snap:Foo").is_err());
        assert!(parse_target("Microsoft.Edge").is_err());
    }

    #[test]
    fn translates_failures() {
        let cancelled = describe_failure(Some(126), "", true);
        assert!(cancelled.message.contains("cancelada"));
        let lock = describe_failure(
            Some(100),
            "E: Could not get lock /var/lib/dpkg/lock-frontend. It is held by process 1234",
            true,
        );
        assert!(lock.message.contains("bloqueado"));
        assert_eq!(lock.code.as_deref(), Some("100"));
        let denied = describe_failure(Some(1), "error: Not allowed to update", false);
        assert!(denied.needs_admin);
        let generic = describe_failure(Some(3), "linha 1\nlinha 2", false);
        assert!(generic.message.contains('3'));
        assert_eq!(generic.detail.as_deref(), Some("linha 1\nlinha 2"));
    }

    #[test]
    fn describes_cache_age() {
        assert_eq!(
            describe_age(Duration::from_secs(60)),
            "atualizada há menos de 1 hora"
        );
        assert_eq!(
            describe_age(Duration::from_secs(5 * 3600)),
            "atualizada há 5 horas"
        );
        assert_eq!(
            describe_age(Duration::from_secs(72 * 3600)),
            "atualizada há 3 dias"
        );
    }

    /// Coleta real nesta máquina (rede para o Flatpak): forma e sanidade.
    #[test]
    fn collects_real_updates() {
        let snapshot = collect();
        eprintln!(
            "atualizações: {} motivo={:?}",
            snapshot.total_count, snapshot.unavailable_reason
        );
        for update in snapshot.updates.iter().take(8) {
            eprintln!(
                "{} | {} | {} -> {} | {}",
                update.id,
                update.name,
                update.current_version,
                update.available_version,
                update.source
            );
        }
        assert_eq!(snapshot.total_count, snapshot.updates.len());
        let mut seen_ids = std::collections::HashSet::new();
        for update in &snapshot.updates {
            assert!(
                parse_target(&update.id).is_ok(),
                "id inválido: {}",
                update.id
            );
            assert!(
                seen_ids.insert(update.id.clone()),
                "id duplicado: {}",
                update.id
            );
            assert!(!update.current_version.is_empty());
            assert!(!update.available_version.is_empty());
        }
    }
}
