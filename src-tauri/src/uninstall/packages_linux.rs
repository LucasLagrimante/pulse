//! Origens de programas instalados no Linux.
//!
//! - **deb**: só pacotes que instalam um `.desktop` visível em
//!   `/usr/share/applications` (`dpkg-query -S` em lote). As ~3000 bibliotecas
//!   do sistema ficam de fora — não são "programas" para o usuário.
//! - **Flatpak**: `flatpak list --app` (runtimes ficam de fora).
//! - **Snap**: `snap list`, apenas snaps com atalho em
//!   `/var/lib/snapd/desktop/applications`.
//! - **AppImage**: arquivos com assinatura AppImage em `~/Applications`,
//!   `~/.local/bin` e `~/Downloads`, com o `.desktop` de integração quando há.
//!
//! Pacote que faz parte da base do sistema (`Essential: yes`, prioridade
//! `required`/`important`) ou cuja remoção arrastaria um meta-pacote do
//! ambiente de trabalho entra na lista **protegido**, com o motivo, e nunca
//! chega ao `apt-get`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::desktop_linux::{
    exec_program, icon_roots, locale_preferences, read_desktop_dir, resolve_icon, DesktopEntry,
};
use super::installed::InstalledApp;
use super::installed_unix::{
    base_app, find_in_path, install_date_from_mtime, run_command, SOURCE_APPIMAGE, SOURCE_DEB,
    SOURCE_FLATPAK, SOURCE_SNAP,
};

pub const SYSTEM_APPLICATIONS_DIR: &str = "/usr/share/applications";
pub const SNAP_APPLICATIONS_DIR: &str = "/var/lib/snapd/desktop/applications";
pub const DPKG_INFO_DIR: &str = "/var/lib/dpkg/info";
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Entradas lidas por pasta na procura de AppImages (`~/Downloads` pode ter
/// milhares de arquivos; só os primeiros bytes de cada um são lidos).
const APPIMAGE_SCAN_MAX_ENTRIES: usize = 3000;

/// Campos do `dpkg-query -W`. Um pacote por linha, separados por tabulação.
pub const DPKG_STATUS_FORMAT: &str = "${binary:Package}\t${Package}\t${Version}\t${Installed-Size}\t${Maintainer}\t${Priority}\t${Essential}\t${db:Status-Abbrev}\t${Section}\t${Pre-Depends}\t${Depends}\n";

struct SourceResult {
    apps: Vec<InstalledApp>,
    problem: Option<String>,
    /// `true` quando a origem existe e foi lida (conta em `scanned_roots`).
    scanned: bool,
}

impl SourceResult {
    fn absent() -> Self {
        SourceResult {
            apps: Vec::new(),
            problem: None,
            scanned: false,
        }
    }

    fn failed(problem: String) -> Self {
        SourceResult {
            apps: Vec::new(),
            problem: Some(problem),
            scanned: false,
        }
    }

    fn ok(apps: Vec<InstalledApp>) -> Self {
        SourceResult {
            apps,
            problem: None,
            scanned: true,
        }
    }
}

/// As quatro origens em paralelo: `dpkg-query -S` sozinho leva ~1 s.
pub fn collect_all() -> (Vec<InstalledApp>, Vec<String>, usize) {
    let home = dirs::home_dir();
    let locales = locale_preferences();
    let roots = icon_roots(home.as_deref());
    let results: Vec<SourceResult> = std::thread::scope(|scope| {
        let handles = vec![
            (
                "Pacotes do sistema (dpkg)",
                scope.spawn(|| collect_deb(&locales, &roots)),
            ),
            (
                "Flatpak",
                scope.spawn(|| collect_flatpak(home.as_deref(), &roots)),
            ),
            ("Snap", scope.spawn(|| collect_snap(&locales, &roots))),
            (
                "AppImage",
                scope.spawn(|| collect_appimages(home.as_deref(), &locales, &roots)),
            ),
        ];
        handles
            .into_iter()
            .map(|(label, handle)| {
                handle.join().unwrap_or_else(|_| {
                    SourceResult::failed(format!("{label}: a leitura falhou inesperadamente"))
                })
            })
            .collect()
    });

    let mut apps = Vec::new();
    let mut problems = Vec::new();
    let mut scanned = 0usize;
    for result in results {
        apps.extend(result.apps);
        if let Some(problem) = result.problem {
            problems.push(problem);
        }
        if result.scanned {
            scanned += 1;
        }
    }
    (apps, problems, scanned)
}

// ---------------------------------------------------------------------------
// deb
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DebPackage {
    /// `${binary:Package}`: inclui `:arch` em pacotes multi-arch "same".
    pub binary_name: String,
    pub name: String,
    pub version: Option<String>,
    pub installed_size_kib: Option<u64>,
    pub maintainer: Option<String>,
    pub priority: Option<String>,
    pub essential: bool,
    pub installed: bool,
    pub section: Option<String>,
    /// Grupos de dependência rígida (`Pre-Depends` + `Depends`); cada grupo é a
    /// lista de alternativas (`a | b`), só com o nome do pacote.
    pub depends: Vec<Vec<String>>,
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Nome sem qualificador de arquitetura (`libc6:amd64` → `libc6`).
pub fn bare_package_name(name: &str) -> &str {
    name.split(':').next().unwrap_or(name).trim()
}

/// `a (>= 1), b | c:any, d [amd64]` → `[[a], [b, c], [d]]`.
pub fn parse_depends(field: &str) -> Vec<Vec<String>> {
    field
        .split(',')
        .filter_map(|group| {
            let alternatives: Vec<String> = group
                .split('|')
                .filter_map(|alternative| {
                    let token = alternative
                        .trim()
                        .split([' ', '(', '['])
                        .next()
                        .unwrap_or_default();
                    let name = bare_package_name(token);
                    (!name.is_empty()).then(|| name.to_string())
                })
                .collect();
            (!alternatives.is_empty()).then_some(alternatives)
        })
        .collect()
}

/// `db:Status-Abbrev` tem 3 letras: desejado, estado, erro. Estado `n`
/// (não instalado) e `c` (só configuração) não contam como instalado.
fn status_is_installed(abbrev: &str) -> bool {
    matches!(abbrev.chars().nth(1), Some(state) if state != 'n' && state != 'c')
}

pub fn parse_dpkg_status(text: &str) -> Vec<DebPackage> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 11 {
                return None;
            }
            let name = fields[1].trim();
            if name.is_empty() {
                return None;
            }
            let mut depends = parse_depends(fields[9]);
            depends.extend(parse_depends(fields[10]));
            Some(DebPackage {
                binary_name: non_empty(fields[0]).unwrap_or_else(|| name.to_string()),
                name: name.to_string(),
                version: non_empty(fields[2]),
                installed_size_kib: fields[3]
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|size| *size > 0),
                maintainer: non_empty(fields[4]),
                priority: non_empty(fields[5]).map(|value| value.to_ascii_lowercase()),
                essential: fields[6].trim().eq_ignore_ascii_case("yes"),
                installed: status_is_installed(fields[7]),
                section: non_empty(fields[8]),
                depends,
            })
        })
        .collect()
}

/// Saída de `dpkg-query -S caminho...`: `pkg1, pkg2: /caminho`. Linhas de
/// desvio (`diversion by ...`) e erros (vão para stderr) não entram.
pub fn parse_dpkg_search(text: &str) -> Vec<(Vec<String>, String)> {
    text.lines()
        .filter(|line| !line.starts_with("diversion by") && !line.starts_with("local diversion"))
        .filter_map(|line| {
            let position = line.find(": /")?;
            let packages: Vec<String> = line[..position]
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
            let path = line[position + 2..].trim().to_string();
            (!packages.is_empty()).then_some((packages, path))
        })
        .collect()
}

/// `Ubuntu Developers <x@y>` → `Ubuntu Developers`.
pub fn maintainer_display(maintainer: &str) -> Option<String> {
    let name = maintainer.split('<').next().unwrap_or_default().trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn is_metapackage(package: &DebPackage) -> bool {
    let section = package
        .section
        .as_deref()
        .map(|section| section.rsplit('/').next().unwrap_or(section))
        .unwrap_or_default();
    section == "metapackages"
        || section == "tasks"
        || package.name.starts_with("mint-meta-")
        || package.name.starts_with("task-")
}

/// Motivo de proteção por pacote (nome sem arquitetura). Três regras:
///
/// 1. `Essential: yes` — o dpkg recusa remover sem `--force`;
/// 2. prioridade `required`/`important` — base mínima do sistema;
/// 3. dependência rígida (direta ou transitiva) de um meta-pacote instalado:
///    o `apt-get remove` levaria o meta-pacote junto e, com ele, o "estado
///    instalado" do ambiente de trabalho (ex.: `dolphin` ← `kde-baseapps` ←
///    `kde-plasma-desktop`). Grupos com mais de uma alternativa instalada não
///    propagam: remover uma delas não quebra a dependência.
pub fn protected_reasons(packages: &[DebPackage]) -> HashMap<String, String> {
    let installed: HashMap<&str, &DebPackage> = packages
        .iter()
        .filter(|package| package.installed)
        .map(|package| (package.name.as_str(), package))
        .collect();
    let mut reasons: HashMap<String, String> = HashMap::new();

    for package in installed.values() {
        if package.essential {
            reasons.insert(
                package.name.clone(),
                "Pacote essencial do sistema (Essential: yes); removê-lo pode deixar o sistema sem inicializar".to_string(),
            );
        } else if let Some(priority) = package.priority.as_deref() {
            if priority == "required" || priority == "important" {
                reasons.insert(
                    package.name.clone(),
                    format!("Faz parte da base do sistema (prioridade {priority})"),
                );
            }
        }
    }

    let mut metas: Vec<&&DebPackage> = installed
        .values()
        .filter(|package| is_metapackage(package))
        .collect();
    metas.sort_by(|left, right| left.name.cmp(&right.name));
    for meta in metas {
        let mut queue: VecDeque<&str> = VecDeque::from([meta.name.as_str()]);
        let mut seen: HashSet<&str> = HashSet::from([meta.name.as_str()]);
        while let Some(current) = queue.pop_front() {
            let Some(package) = installed.get(current) else {
                continue;
            };
            for group in &package.depends {
                let present: Vec<&str> = group
                    .iter()
                    .map(String::as_str)
                    .filter(|name| installed.contains_key(name))
                    .collect();
                if present.len() != 1 {
                    continue;
                }
                let dependency = present[0];
                if seen.insert(dependency) {
                    queue.push_back(dependency);
                    reasons.entry(dependency.to_string()).or_insert_with(|| {
                        format!(
                            "Removê-lo levaria junto o meta-pacote {} do ambiente de trabalho",
                            meta.name
                        )
                    });
                }
            }
        }
    }
    reasons
}

/// Caminho do `.list` do pacote (arquivos que ele instalou). Usado também
/// pela medição de tamanho real.
pub fn dpkg_list_file(binary_name: &str) -> Option<PathBuf> {
    let bare = bare_package_name(binary_name);
    let mut candidates = vec![format!("{binary_name}.list"), format!("{bare}.list")];
    if !binary_name.contains(':') {
        for arch in ["amd64", "arm64", "i386", "armhf", "all"] {
            candidates.push(format!("{bare}:{arch}.list"));
        }
    }
    candidates
        .into_iter()
        .map(|name| Path::new(DPKG_INFO_DIR).join(name))
        .find(|path| path.is_file())
}

/// Monta os apps deb a partir dos `.desktop` visíveis e de quem é dono deles.
/// Função pura sobre dados já coletados (testável com fixtures).
pub fn build_deb_apps(
    entries: &[DesktopEntry],
    owners: &[(Vec<String>, String)],
    packages: &[DebPackage],
    icon_roots: &[PathBuf],
) -> Vec<InstalledApp> {
    let by_binary: HashMap<&str, &DebPackage> = packages
        .iter()
        .filter(|package| package.installed)
        .flat_map(|package| {
            [
                (package.binary_name.as_str(), package),
                (package.name.as_str(), package),
            ]
        })
        .collect();
    let protected = protected_reasons(packages);
    let entry_by_path: HashMap<String, &DesktopEntry> = entries
        .iter()
        .map(|entry| (entry.path.to_string_lossy().to_string(), entry))
        .collect();

    // Pacote → seus atalhos visíveis. Arquivo com dois donos (raro, divergência
    // de empacotamento) fica com o primeiro dono instalado.
    let mut grouped: BTreeMap<String, (&DebPackage, Vec<&DesktopEntry>)> = BTreeMap::new();
    for (owner_names, path) in owners {
        let Some(entry) = entry_by_path.get(path) else {
            continue;
        };
        let Some(package) = owner_names
            .iter()
            .find_map(|owner| by_binary.get(owner.as_str()).copied())
        else {
            continue;
        };
        grouped
            .entry(package.binary_name.clone())
            .or_insert_with(|| (package, Vec::new()))
            .1
            .push(entry);
    }

    grouped
        .into_values()
        .map(|(package, mut desktop_entries)| {
            desktop_entries.sort_by(|left, right| left.id.cmp(&right.id));
            // Atalho principal: o que tem o nome do pacote; senão o primeiro.
            let primary = desktop_entries
                .iter()
                .find(|entry| {
                    entry.id == package.name || entry.id.ends_with(&format!(".{}", package.name))
                })
                .or_else(|| desktop_entries.first())
                .copied();
            let display_name = primary
                .and_then(|entry| entry.name.clone())
                .unwrap_or_else(|| package.name.clone());
            let mut app = base_app(
                SOURCE_DEB,
                "system",
                &package.binary_name,
                display_name,
                format!("Pacote deb {}", package.binary_name),
            );
            app.publisher = package.maintainer.as_deref().and_then(maintainer_display);
            app.version = package.version.clone();
            app.estimated_size_kb = package.installed_size_kib;
            app.install_date = dpkg_list_file(&package.binary_name)
                .and_then(|path| install_date_from_mtime(&path));
            app.uninstall_string = Some(format!("apt-get remove {}", package.binary_name));
            app.icon = desktop_entries
                .iter()
                .filter_map(|entry| entry.icon.as_deref())
                .find_map(|icon| resolve_icon(icon, icon_roots));
            app.protected_reason = protected.get(package.name.as_str()).cloned();
            let mut names = vec![package.name.clone()];
            names.extend(desktop_entries.iter().map(|entry| entry.id.clone()));
            app.match_names = names;
            app
        })
        .collect()
}

fn collect_deb(locales: &[String], roots: &[PathBuf]) -> SourceResult {
    let Some(dpkg_query) = find_in_path("dpkg-query") else {
        return SourceResult::failed(
            "Pacotes do sistema: esta distribuição não usa dpkg; pacotes rpm/pacman ainda não são listados (Flatpak, Snap e AppImage foram lidos)".to_string(),
        );
    };
    let dpkg_query = dpkg_query.to_string_lossy().to_string();
    let entries: Vec<DesktopEntry> = read_desktop_dir(Path::new(SYSTEM_APPLICATIONS_DIR), locales)
        .into_iter()
        .filter(DesktopEntry::is_visible_app)
        .collect();
    if entries.is_empty() {
        return SourceResult::ok(Vec::new());
    }

    let paths: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.to_string_lossy().to_string())
        .collect();
    let mut search_args: Vec<&str> = vec!["-S"];
    search_args.extend(paths.iter().map(String::as_str));
    // Código 1 = algum atalho não pertence a pacote (instalado à mão): a
    // saída dos demais continua válida.
    let owners = match run_command(&dpkg_query, &search_args, QUERY_TIMEOUT, true) {
        Ok(output) if !output.timed_out && matches!(output.status, Some(0) | Some(1)) => {
            parse_dpkg_search(&output.stdout)
        }
        Ok(output) => {
            return SourceResult::failed(format!(
                "Pacotes do sistema: dpkg-query -S falhou ({})",
                output.tail(1).join(" ")
            ))
        }
        Err(error) => {
            return SourceResult::failed(format!(
                "Pacotes do sistema: dpkg-query não pôde ser executado ({error})"
            ))
        }
    };
    let packages = match run_command(
        &dpkg_query,
        &["-W", "-f", DPKG_STATUS_FORMAT],
        QUERY_TIMEOUT,
        true,
    ) {
        Ok(output) if output.success() => parse_dpkg_status(&output.stdout),
        Ok(output) => {
            return SourceResult::failed(format!(
                "Pacotes do sistema: dpkg-query -W falhou ({})",
                output.tail(1).join(" ")
            ))
        }
        Err(error) => {
            return SourceResult::failed(format!(
                "Pacotes do sistema: dpkg-query não pôde ser executado ({error})"
            ))
        }
    };
    SourceResult::ok(build_deb_apps(&entries, &owners, &packages, roots))
}

// ---------------------------------------------------------------------------
// Flatpak
// ---------------------------------------------------------------------------

/// Colunas pedidas ao `flatpak list`, na ordem do parser.
pub const FLATPAK_COLUMNS: &str = "--columns=ref,application,name,version,origin,installation,size";

#[derive(Debug, Clone, PartialEq)]
pub struct FlatpakApp {
    pub reference: String,
    pub app_id: String,
    pub name: String,
    pub version: Option<String>,
    pub origin: Option<String>,
    pub installation: String,
    pub size_bytes: Option<u64>,
}

/// Tamanho formatado pelo GLib (`g_format_size`, unidades decimais). Com
/// `LC_ALL=C` o espaço fino vira `?` (`1.4?MB`): só número e unidade importam.
pub fn parse_flatpak_size(raw: &str) -> Option<u64> {
    let text = raw.trim();
    let number: String = text
        .chars()
        .take_while(|character| {
            character.is_ascii_digit() || *character == '.' || *character == ','
        })
        .collect();
    let value: f64 = number.replace(',', ".").parse().ok()?;
    let unit: String = text[number.len()..]
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .collect();
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "bytes" | "byte" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = value * multiplier;
    (bytes.is_finite() && bytes > 0.0).then_some(bytes.round() as u64)
}

pub fn parse_flatpak_list(text: &str) -> Vec<FlatpakApp> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 7 {
                return None;
            }
            let reference = fields[0].trim();
            let app_id = fields[1].trim();
            if !reference.starts_with("app/") || app_id.is_empty() {
                return None;
            }
            Some(FlatpakApp {
                reference: reference.to_string(),
                app_id: app_id.to_string(),
                name: non_empty(fields[2]).unwrap_or_else(|| app_id.to_string()),
                version: non_empty(fields[3]),
                origin: non_empty(fields[4]),
                installation: non_empty(fields[5]).unwrap_or_else(|| "system".to_string()),
                size_bytes: parse_flatpak_size(fields[6]),
            })
        })
        .collect()
}

/// Pasta de implantação do app (contém todas as versões instaladas dele).
pub fn flatpak_app_dir(installation: &str, app_id: &str, home: Option<&Path>) -> Option<PathBuf> {
    match installation {
        "system" => Some(Path::new("/var/lib/flatpak/app").join(app_id)),
        "user" => home.map(|home| home.join(".local/share/flatpak/app").join(app_id)),
        _ => None,
    }
}

fn flatpak_exports_icons(installation: &str, home: Option<&Path>) -> Option<PathBuf> {
    match installation {
        "system" => Some(PathBuf::from("/var/lib/flatpak/exports/share/icons")),
        "user" => home.map(|home| home.join(".local/share/flatpak/exports/share/icons")),
        _ => None,
    }
}

pub fn build_flatpak_apps(
    listed: Vec<FlatpakApp>,
    home: Option<&Path>,
    roots: &[PathBuf],
) -> Vec<InstalledApp> {
    listed
        .into_iter()
        .map(|item| {
            let mut app = base_app(
                SOURCE_FLATPAK,
                &item.installation,
                &item.reference,
                item.name.clone(),
                format!("Flatpak {} ({})", item.reference, item.installation),
            );
            app.publisher = item
                .origin
                .as_ref()
                .map(|origin| format!("{origin} (Flatpak)"));
            app.version = item.version.clone();
            app.estimated_size_kb = item.size_bytes.map(|bytes| bytes.div_ceil(1024));
            let location =
                flatpak_app_dir(&item.installation, &item.app_id, home).filter(|dir| dir.is_dir());
            app.install_date = location.as_deref().and_then(install_date_from_mtime);
            app.install_location = location.map(|dir| dir.to_string_lossy().to_string());
            let scope_flag = match item.installation.as_str() {
                "system" => "--system".to_string(),
                "user" => "--user".to_string(),
                other => format!("--installation={other}"),
            };
            app.uninstall_string =
                Some(format!("flatpak uninstall {scope_flag} {}", item.reference));
            let mut search_roots: Vec<PathBuf> = flatpak_exports_icons(&item.installation, home)
                .into_iter()
                .collect();
            search_roots.extend(roots.iter().cloned());
            app.icon = resolve_icon(&item.app_id, &search_roots);
            app.match_names = vec![item.app_id.clone()];
            app
        })
        .collect()
}

fn collect_flatpak(home: Option<&Path>, roots: &[PathBuf]) -> SourceResult {
    let Some(flatpak) = find_in_path("flatpak") else {
        return SourceResult::absent();
    };
    match run_command(
        &flatpak.to_string_lossy(),
        &["list", "--app", FLATPAK_COLUMNS],
        QUERY_TIMEOUT,
        true,
    ) {
        Ok(output) if output.success() => SourceResult::ok(build_flatpak_apps(
            parse_flatpak_list(&output.stdout),
            home,
            roots,
        )),
        Ok(output) => SourceResult::failed(format!(
            "Flatpak: a lista de apps falhou ({})",
            output.tail(1).join(" ")
        )),
        Err(error) => SourceResult::failed(format!("Flatpak: não pôde ser executado ({error})")),
    }
}

// ---------------------------------------------------------------------------
// Snap
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct SnapItem {
    pub name: String,
    pub version: Option<String>,
    pub revision: String,
    pub publisher: Option<String>,
    pub notes: Vec<String>,
}

/// `snap list --unicode=never --color=never`: colunas Name, Version, Rev,
/// Tracking, Publisher, Notes separadas por espaços.
pub fn parse_snap_list(text: &str) -> Vec<SnapItem> {
    text.lines()
        .skip_while(|line| !line.starts_with("Name"))
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 6 {
                return None;
            }
            let publisher = fields[4].trim_end_matches(['*', '✓', '✪']);
            Some(SnapItem {
                name: fields[0].to_string(),
                version: non_empty(fields[1]),
                revision: fields[2].to_string(),
                publisher: (publisher != "-" && !publisher.is_empty())
                    .then(|| publisher.to_string()),
                notes: fields[5]
                    .split(',')
                    .filter(|note| *note != "-")
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}

pub fn snap_protected_reason(item: &SnapItem) -> Option<String> {
    const SYSTEM_TYPES: &[&str] = &["base", "core", "snapd", "gadget", "kernel"];
    if item
        .notes
        .iter()
        .any(|note| SYSTEM_TYPES.contains(&note.as_str()))
        || item.name == "snapd"
        || item.name.starts_with("core")
    {
        return Some(
            "Snap de base do sistema (core/snapd); outros snaps dependem dele".to_string(),
        );
    }
    None
}

fn collect_snap(locales: &[String], roots: &[PathBuf]) -> SourceResult {
    let Some(snap) = find_in_path("snap") else {
        return SourceResult::absent();
    };
    let listed = match run_command(
        &snap.to_string_lossy(),
        &["list", "--unicode=never", "--color=never"],
        QUERY_TIMEOUT,
        true,
    ) {
        Ok(output) if output.success() => parse_snap_list(&output.stdout),
        // Sem snaps instalados o `snap list` sai com erro e mensagem própria.
        Ok(output) if output.stderr.contains("No snaps are installed") => Vec::new(),
        Ok(output) => {
            return SourceResult::failed(format!(
                "Snap: a lista falhou ({})",
                output.tail(1).join(" ")
            ))
        }
        Err(error) => {
            return SourceResult::failed(format!("Snap: não pôde ser executado ({error})"))
        }
    };
    let desktop_entries = read_desktop_dir(Path::new(SNAP_APPLICATIONS_DIR), locales);
    let apps = listed
        .into_iter()
        .filter_map(|item| {
            let prefix = format!("{}_", item.name);
            let entries: Vec<&DesktopEntry> = desktop_entries
                .iter()
                .filter(|entry| entry.id.starts_with(&prefix) && entry.is_visible_app())
                .collect();
            // Só snaps com atalho no menu são "programas" para o usuário.
            let primary = entries.first()?;
            let mut app = base_app(
                SOURCE_SNAP,
                "system",
                &item.name,
                primary.name.clone().unwrap_or_else(|| item.name.clone()),
                format!("Snap {} (revisão {})", item.name, item.revision),
            );
            app.publisher = item.publisher.clone();
            app.version = item.version.clone();
            let snap_file = Path::new("/var/lib/snapd/snaps")
                .join(format!("{}_{}.snap", item.name, item.revision));
            app.estimated_size_kb = fs::metadata(&snap_file)
                .ok()
                .map(|meta| meta.len().div_ceil(1024));
            app.install_date = install_date_from_mtime(&snap_file);
            let location = Path::new("/snap").join(&item.name);
            app.install_location = location
                .is_dir()
                .then(|| location.to_string_lossy().to_string());
            app.uninstall_string = Some(format!("snap remove {}", item.name));
            app.icon = entries
                .iter()
                .filter_map(|entry| entry.icon.as_deref())
                .find_map(|icon| resolve_icon(icon, roots));
            app.protected_reason = snap_protected_reason(&item);
            app.match_names = vec![item.name.clone()];
            Some(app)
        })
        .collect();
    SourceResult::ok(apps)
}

// ---------------------------------------------------------------------------
// AppImage
// ---------------------------------------------------------------------------

/// Assinatura AppImage: ELF (`\x7fELF`) com `AI` + tipo (1 ou 2) no offset 8.
pub fn is_appimage_header(header: &[u8]) -> bool {
    header.len() >= 11
        && header[..4] == [0x7f, b'E', b'L', b'F']
        && header[8] == b'A'
        && header[9] == b'I'
        && (header[10] == 1 || header[10] == 2)
}

pub fn has_appimage_signature(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 16];
    match file.read(&mut header) {
        Ok(read) => is_appimage_header(&header[..read]),
        Err(_) => false,
    }
}

/// `Obsidian-1.5.3-x86_64.AppImage` → (`Obsidian`, `Some("1.5.3")`).
pub fn parse_appimage_file_name(file_name: &str) -> (String, Option<String>) {
    let lower = file_name.to_ascii_lowercase();
    let mut stem = if lower.ends_with(".appimage") {
        file_name[..file_name.len() - ".appimage".len()].to_string()
    } else {
        file_name.to_string()
    };
    for arch in [
        "x86_64", "amd64", "aarch64", "arm64", "i386", "i686", "armhf",
    ] {
        for separator in ['-', '_', '.'] {
            let suffix = format!("{separator}{arch}");
            if stem.to_ascii_lowercase().ends_with(&suffix) {
                stem.truncate(stem.len() - suffix.len());
            }
        }
    }
    let mut version = None;
    if let Some(position) = stem.rfind(['-', '_']) {
        let tail = stem[position + 1..].trim_start_matches(['v', 'V']);
        if tail.chars().next().is_some_and(|c| c.is_ascii_digit())
            && tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
            && tail.contains('.')
        {
            version = Some(tail.to_string());
            stem.truncate(position);
        }
    }
    let name = stem.replace('_', " ").trim().to_string();
    (
        if name.is_empty() {
            file_name.to_string()
        } else {
            name
        },
        version,
    )
}

/// Pastas onde AppImages costumam ficar. Só a pasta pessoal.
pub fn appimage_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs_found = vec![
        home.join("Applications"),
        home.join(".local/bin"),
        home.join("Downloads"),
    ];
    if let Some(download) = dirs::download_dir() {
        if !dirs_found.contains(&download) {
            dirs_found.push(download);
        }
    }
    dirs_found
}

fn collect_appimages(home: Option<&Path>, locales: &[String], roots: &[PathBuf]) -> SourceResult {
    let Some(home) = home else {
        return SourceResult::failed("AppImage: pasta pessoal não encontrada".to_string());
    };
    let integrations: Vec<DesktopEntry> =
        read_desktop_dir(&home.join(".local/share/applications"), locales);
    let mut apps = Vec::new();
    for dir in appimage_dirs(home) {
        let Ok(iter) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in iter.flatten().take(APPIMAGE_SCAN_MAX_ENTRIES) {
            let path = entry.path();
            // Link simbólico não é o AppImage: o alvo é que seria removido.
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file()
                || metadata.len() < 16
                || !has_appimage_signature(&path)
            {
                continue;
            }
            let path_text = path.to_string_lossy().to_string();
            let integration: Vec<&DesktopEntry> = integrations
                .iter()
                .filter(|desktop| {
                    desktop
                        .exec
                        .as_deref()
                        .and_then(exec_program)
                        .is_some_and(|program| program == path_text)
                        || desktop.try_exec.as_deref() == Some(path_text.as_str())
                })
                .collect();
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let (parsed_name, version) = parse_appimage_file_name(&file_name);
            let display_name = integration
                .iter()
                .find_map(|desktop| desktop.name.clone())
                .unwrap_or_else(|| parsed_name.clone());
            let mut app = base_app(
                SOURCE_APPIMAGE,
                "user",
                &path_text,
                display_name,
                format!("AppImage {path_text}"),
            );
            app.version = version;
            app.estimated_size_kb = Some(metadata.len().div_ceil(1024));
            app.install_date = install_date_from_mtime(&path);
            app.install_location = Some(path_text.clone());
            app.uninstall_string = Some(format!("Mover para a Lixeira: {path_text}"));
            app.icon = integration
                .iter()
                .filter_map(|desktop| desktop.icon.as_deref())
                .find_map(|icon| resolve_icon(icon, roots));
            let mut names = vec![parsed_name];
            names.extend(
                integration
                    .iter()
                    .filter(|desktop| !desktop.id.starts_with("appimagekit_"))
                    .map(|desktop| desktop.id.clone()),
            );
            app.match_names = names;
            apps.push(app);
        }
    }
    SourceResult::ok(apps)
}

/// `.desktop` de integração do usuário que apontam para este AppImage.
/// Usado na remoção: só o que referencia **exatamente** o arquivo sai junto.
pub fn appimage_integration_files(home: &Path, appimage: &Path) -> Vec<DesktopEntry> {
    let target = appimage.to_string_lossy().to_string();
    read_desktop_dir(&home.join(".local/share/applications"), &[])
        .into_iter()
        .filter(|desktop| {
            desktop
                .exec
                .as_deref()
                .and_then(exec_program)
                .is_some_and(|program| program == target)
                || desktop.try_exec.as_deref() == Some(target.as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DPKG_STATUS: &str = "\
dolphin\tdolphin\t4:23.08.5-0ubuntu1\t5432\tKubuntu Developers <kubuntu-devel@lists.ubuntu.com>\toptional\tno\tii \tkde\t\tkio, libc6 (>= 2.34), libkf5x | libkf5y
kde-baseapps\tkde-baseapps\t4:23.08\t20\tKubuntu Developers <x@y>\toptional\tno\tii \tmetapackages\t\tdolphin (>= 4:23), konsole | xterm
kde-plasma-desktop\tkde-plasma-desktop\t5:142\t30\tKubuntu Developers <x@y>\toptional\tno\tii \tuniverse/metapackages\t\tkde-baseapps (>= 4:23), plasma-desktop
plasma-desktop\tplasma-desktop\t4:5.27\t9000\tKubuntu Developers <x@y>\toptional\tno\tii \tkde\t\t
konsole\tkonsole\t4:23.08\t3000\tKubuntu Developers <x@y>\toptional\tno\tii \tkde\t\t
xterm\txterm\t390\t1000\tUbuntu Developers <x@y>\toptional\tno\tii \tx11\t\t
firefox\tfirefox\t1:1snap1\t200000\tUbuntu Mozilla Team <x@y>\toptional\tno\tii \tweb\t\tlibc6
bash\tbash\t5.2\t7000\tUbuntu Developers <x@y>\trequired\tyes\tii \tshells\tlibc6\t
apt\tapt\t2.7\t4000\tUbuntu Developers <x@y>\timportant\tno\tii \tadmin\t\tlibc6
libc6:amd64\tlibc6\t2.39\t13000\tUbuntu Developers <x@y>\toptional\tno\tii \tlibs\t\t
gimp\tgimp\t2.10\t20000\tDebian <x@y>\toptional\tno\trc \tgraphics\t\t
";

    #[test]
    fn status_do_dpkg_e_parseado() {
        let packages = parse_dpkg_status(DPKG_STATUS);
        assert_eq!(packages.len(), 11);
        let dolphin = &packages[0];
        assert_eq!(dolphin.name, "dolphin");
        assert_eq!(dolphin.installed_size_kib, Some(5432));
        assert_eq!(dolphin.priority.as_deref(), Some("optional"));
        assert!(dolphin.installed);
        assert_eq!(
            dolphin.depends,
            vec![
                vec!["kio".to_string()],
                vec!["libc6".to_string()],
                vec!["libkf5x".to_string(), "libkf5y".to_string()]
            ]
        );
        let libc = packages.iter().find(|p| p.name == "libc6").unwrap();
        assert_eq!(libc.binary_name, "libc6:amd64");
        let gimp = packages.iter().find(|p| p.name == "gimp").unwrap();
        assert!(!gimp.installed, "rc = só configuração, não instalado");
    }

    #[test]
    fn protecao_de_essenciais_prioridade_e_meta_pacotes() {
        let packages = parse_dpkg_status(DPKG_STATUS);
        let reasons = protected_reasons(&packages);
        assert!(reasons["bash"].contains("Essential"));
        assert!(reasons["apt"].contains("important"));
        // dolphin ← kde-baseapps ← kde-plasma-desktop.
        assert!(reasons["dolphin"].contains("meta-pacote"));
        assert!(reasons.contains_key("kde-baseapps"));
        assert!(reasons.contains_key("plasma-desktop"));
        // `konsole | xterm` com as duas instaladas: remover uma não quebra.
        assert!(!reasons.contains_key("konsole"));
        assert!(!reasons.contains_key("xterm"));
        assert!(!reasons.contains_key("firefox"));
        assert!(!reasons.contains_key("gimp"));
    }

    #[test]
    fn busca_de_dono_do_dpkg() {
        let parsed = parse_dpkg_search(
            "dolphin: /usr/share/applications/org.kde.dolphin.desktop\n\
             diversion by foo from: /usr/share/applications/x.desktop\n\
             libreoffice-common, libreoffice-writer: /usr/share/applications/libreoffice-writer.desktop\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, vec!["dolphin".to_string()]);
        assert_eq!(
            parsed[0].1,
            "/usr/share/applications/org.kde.dolphin.desktop"
        );
        assert_eq!(parsed[1].0.len(), 2);
    }

    fn entry(id: &str, name: &str) -> DesktopEntry {
        DesktopEntry {
            path: PathBuf::from(format!("/usr/share/applications/{id}.desktop")),
            id: id.to_string(),
            name: Some(name.to_string()),
            icon: None,
            exec: Some(id.to_string()),
            try_exec: None,
            entry_type: Some("Application".to_string()),
            no_display: false,
            hidden: false,
        }
    }

    #[test]
    fn apps_deb_um_por_pacote_com_protecao() {
        let packages = parse_dpkg_status(DPKG_STATUS);
        let entries = vec![
            entry("org.kde.dolphin", "Dolphin"),
            entry("firefox", "Firefox Web Browser"),
            entry("firefox-private", "Firefox Private"),
            entry("orfao", "Instalado à mão"),
        ];
        let owners = parse_dpkg_search(
            "dolphin: /usr/share/applications/org.kde.dolphin.desktop\n\
             firefox: /usr/share/applications/firefox.desktop\n\
             firefox: /usr/share/applications/firefox-private.desktop\n",
        );
        let apps = build_deb_apps(&entries, &owners, &packages, &[]);
        assert_eq!(
            apps.len(),
            2,
            "um app por pacote; atalho sem dono fica fora"
        );
        let firefox = apps
            .iter()
            .find(|app| app.id == "deb|system|firefox")
            .unwrap();
        assert_eq!(firefox.display_name, "Firefox Web Browser");
        assert_eq!(firefox.estimated_size_kb, Some(200000));
        assert_eq!(firefox.publisher.as_deref(), Some("Ubuntu Mozilla Team"));
        assert!(firefox.protected_reason.is_none());
        assert!(firefox.match_names.contains(&"firefox".to_string()));
        let dolphin = apps
            .iter()
            .find(|app| app.id == "deb|system|dolphin")
            .unwrap();
        assert_eq!(dolphin.display_name, "Dolphin");
        assert!(dolphin.protected_reason.is_some());
    }

    #[test]
    fn tamanho_do_flatpak_com_locale_c() {
        assert_eq!(parse_flatpak_size("1.4?MB"), Some(1_400_000));
        assert_eq!(parse_flatpak_size("347.3\u{a0}MB"), Some(347_300_000));
        assert_eq!(parse_flatpak_size("16.4 kB"), Some(16_400));
        assert_eq!(parse_flatpak_size("512 bytes"), Some(512));
        assert_eq!(parse_flatpak_size("1.2 GB"), Some(1_200_000_000));
        assert_eq!(parse_flatpak_size(""), None);
        assert_eq!(parse_flatpak_size("??"), None);
    }

    #[test]
    fn lista_do_flatpak_e_parseada() {
        let listed = parse_flatpak_list(
            "app/com.github.tchx84.Flatseal/x86_64/stable\tcom.github.tchx84.Flatseal\tFlatseal\t2.4.1\tflathub\tsystem\t1.4?MB\n\
             app/com.rtosta.zapzap/x86_64/stable\tcom.rtosta.zapzap\tZapZap\t\tflathub\tuser\t347.3?MB\n\
             runtime/org.kde.Platform/x86_64/6.7\torg.kde.Platform\tKDE\t\tflathub\tsystem\t1 GB\n",
        );
        assert_eq!(listed.len(), 2, "runtime não é app");
        assert_eq!(listed[0].app_id, "com.github.tchx84.Flatseal");
        assert_eq!(listed[0].size_bytes, Some(1_400_000));
        assert_eq!(listed[1].installation, "user");
        assert_eq!(listed[1].version, None);
        let apps = build_flatpak_apps(listed, Some(Path::new("/home/teste")), &[]);
        assert_eq!(
            apps[1].id,
            "flatpak|user|app/com.rtosta.zapzap/x86_64/stable"
        );
        assert_eq!(apps[1].publisher.as_deref(), Some("flathub (Flatpak)"));
        assert!(apps[1]
            .uninstall_string
            .as_deref()
            .unwrap()
            .contains("--user"));
    }

    #[test]
    fn lista_de_snaps_e_protecao_de_base() {
        let listed = parse_snap_list(
            "Name      Version   Rev    Tracking       Publisher   Notes\n\
             core22    20240111  1122   latest/stable  canonical*  base\n\
             firefox   124.0     4033   latest/stable  mozilla*    -\n\
             snapd     2.61      21184  latest/stable  canonical*  snapd\n",
        );
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[1].name, "firefox");
        assert_eq!(listed[1].publisher.as_deref(), Some("mozilla"));
        assert!(listed[1].notes.is_empty());
        assert!(snap_protected_reason(&listed[0]).is_some());
        assert!(snap_protected_reason(&listed[1]).is_none());
        assert!(snap_protected_reason(&listed[2]).is_some());
    }

    #[test]
    fn assinatura_e_nome_de_appimage() {
        let mut header = [0u8; 16];
        header[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        header[8] = b'A';
        header[9] = b'I';
        header[10] = 2;
        assert!(is_appimage_header(&header));
        header[10] = 0;
        assert!(!is_appimage_header(&header));
        assert!(!is_appimage_header(b"#!/bin/sh\n"));

        assert_eq!(
            parse_appimage_file_name("Obsidian-1.5.3-x86_64.AppImage"),
            ("Obsidian".to_string(), Some("1.5.3".to_string()))
        );
        assert_eq!(
            parse_appimage_file_name("Cura_v5.7.1.AppImage"),
            ("Cura".to_string(), Some("5.7.1".to_string()))
        );
        assert_eq!(
            parse_appimage_file_name("Krita.appimage"),
            ("Krita".to_string(), None)
        );
    }

    #[test]
    fn inventario_linux_real_traz_apps_de_usuario_e_nao_bibliotecas() {
        let (apps, problems, scanned) = collect_all();
        assert!(
            scanned >= 1 || !problems.is_empty(),
            "alguma origem precisa ter sido lida ou explicada"
        );
        assert!(apps.iter().all(|app| !app.display_name.trim().is_empty()));
        // Nenhuma biblioteca pura entra (não têm `.desktop`).
        assert!(!apps
            .iter()
            .any(|app| app.id == "deb|system|libc6" || app.id == "deb|system|libc6:amd64"));
        if Path::new("/usr/bin/dpkg-query").exists() && Path::new(SYSTEM_APPLICATIONS_DIR).is_dir()
        {
            assert!(apps
                .iter()
                .any(|app| app.source.as_deref() == Some(SOURCE_DEB)));
            assert!(
                apps.len() < 1500,
                "a lista não pode virar o inventário de bibliotecas"
            );
        }
    }
}
