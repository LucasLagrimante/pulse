//! Resíduos fora do Windows: pastas de dados **na pasta pessoal** com nome
//! exato do app. Submódulo de `residue`: árvore, snapshot, revalidação e
//! remoção são os mesmos do Windows; aqui ficam só as regras de caminho.
//!
//! - Linux: `~/.config`, `~/.local/share`, `~/.cache`, `~/.local/state`,
//!   `~/.var/app/<id>` (só Flatpak) e `~/snap/<nome>` (só Snap);
//! - macOS: `~/Library/Application Support`, `Caches`, `Logs` (nome ou
//!   bundle id) e `Containers`, `Preferences`, `HTTPStorages`, `WebKit`,
//!   `Saved Application State` (só bundle id). TODO(macos): validar em hardware.
//!
//! Nunca fora do home, nunca a própria raiz XDG, nunca nome genérico
//! (`applications`, `icons`, `fonts`...), nunca link simbólico e nunca uma
//! pasta cujo nome também identifica **outro** programa instalado.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{build_folder_node, normalize_display_name, UninstallTargets, TREE_MAX_ENTRIES};
use crate::uninstall::installed::InstalledApp;
use crate::uninstall::installed_unix::{parse_id, SOURCE_FLATPAK, SOURCE_MAC_APP, SOURCE_SNAP};

/// Entradas lidas por raiz de dados (`~/.config` costuma ter centenas).
const ROOT_SCAN_MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRule {
    /// Qualquer nome exato do app (id técnico ou nome de exibição).
    AnyName,
    /// Só o id técnico, e só para apps da origem indicada.
    IdOnly(&'static str),
    /// `<bundle id>.savedState` (macOS).
    SavedState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResidueRoot {
    pub path: PathBuf,
    pub label: &'static str,
    pub rule: NameRule,
}

pub fn linux_residue_roots(home: &Path) -> Vec<ResidueRoot> {
    vec![
        ResidueRoot {
            path: home.join(".config"),
            label: "~/.config",
            rule: NameRule::AnyName,
        },
        ResidueRoot {
            path: home.join(".local/share"),
            label: "~/.local/share",
            rule: NameRule::AnyName,
        },
        ResidueRoot {
            path: home.join(".cache"),
            label: "~/.cache",
            rule: NameRule::AnyName,
        },
        ResidueRoot {
            path: home.join(".local/state"),
            label: "~/.local/state",
            rule: NameRule::AnyName,
        },
        ResidueRoot {
            path: home.join(".var/app"),
            label: "~/.var/app",
            rule: NameRule::IdOnly(SOURCE_FLATPAK),
        },
        ResidueRoot {
            path: home.join("snap"),
            label: "~/snap",
            rule: NameRule::IdOnly(SOURCE_SNAP),
        },
    ]
}

pub fn macos_residue_roots(home: &Path) -> Vec<ResidueRoot> {
    let library = home.join("Library");
    vec![
        ResidueRoot {
            path: library.join("Application Support"),
            label: "~/Library/Application Support",
            rule: NameRule::AnyName,
        },
        ResidueRoot {
            path: library.join("Caches"),
            label: "~/Library/Caches",
            rule: NameRule::AnyName,
        },
        ResidueRoot {
            path: library.join("Logs"),
            label: "~/Library/Logs",
            rule: NameRule::AnyName,
        },
        ResidueRoot {
            path: library.join("Containers"),
            label: "~/Library/Containers",
            rule: NameRule::IdOnly(SOURCE_MAC_APP),
        },
        ResidueRoot {
            path: library.join("Preferences"),
            label: "~/Library/Preferences",
            rule: NameRule::IdOnly(SOURCE_MAC_APP),
        },
        ResidueRoot {
            path: library.join("HTTPStorages"),
            label: "~/Library/HTTPStorages",
            rule: NameRule::IdOnly(SOURCE_MAC_APP),
        },
        ResidueRoot {
            path: library.join("WebKit"),
            label: "~/Library/WebKit",
            rule: NameRule::IdOnly(SOURCE_MAC_APP),
        },
        ResidueRoot {
            path: library.join("Saved Application State"),
            label: "~/Library/Saved Application State",
            rule: NameRule::SavedState,
        },
    ]
}

pub fn residue_roots(home: &Path) -> Vec<ResidueRoot> {
    #[cfg(target_os = "macos")]
    {
        macos_residue_roots(home)
    }
    #[cfg(not(target_os = "macos"))]
    {
        linux_residue_roots(home)
    }
}

/// Nomes de pasta que pertencem à estrutura do desktop, nunca a um app.
const FORBIDDEN_NAMES: &[&str] = &[
    "applications",
    "autostart",
    "icons",
    "fonts",
    "mime",
    "menus",
    "desktop-directories",
    "trash",
    "flatpak",
    "snap",
    "systemd",
    "dconf",
    "gtk-2.0",
    "gtk-3.0",
    "gtk-4.0",
    "kde",
    "kde4",
    "kdedefaults",
    "plasma",
    "pulse",
    "pipewire",
    "wireplumber",
    "fontconfig",
    "keyrings",
    "gnome",
    "gvfs",
    "gvfs-metadata",
    "tracker",
    "tracker3",
    "environment.d",
    "xdg-desktop-portal",
    "share",
    "state",
    "cache",
    "config",
    "local",
    "bin",
    "lib",
    "app",
    "documents",
    "downloads",
    "desktop",
    "pictures",
    "music",
    "videos",
    "templates",
    "public",
    "library",
    "application support",
    "caches",
    "preferences",
    "containers",
    "logs",
    "apple",
    "com.apple",
    "tmp",
    "temp",
    "common",
    "shared",
];

pub fn is_forbidden_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." || trimmed.contains('/') {
        return true;
    }
    let lowered = trimmed.to_lowercase();
    FORBIDDEN_NAMES.contains(&lowered.as_str())
}

/// Normalização lexical (`.`/`..`, barras repetidas e final) sem seguir link
/// nem exigir que o caminho exista.
pub fn normalize(path: &Path) -> String {
    let absolute = path.has_root();
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Raiz ou dentro dela, comparando componentes inteiros e com caixa exata
/// (sistemas de arquivos unix diferenciam maiúsculas).
pub fn is_inside_tree(root: &str, candidate: &str) -> bool {
    let root = normalize(Path::new(root));
    let candidate = normalize(Path::new(candidate));
    if root.is_empty() || candidate.is_empty() || root == "/" {
        return false;
    }
    candidate == root || candidate.starts_with(&format!("{root}/"))
}

/// Pastas que nunca são resíduo: estrutura do sistema e do home.
pub fn shell_folder_patterns_for(home: Option<&Path>) -> Vec<String> {
    let mut patterns: BTreeSet<String> = [
        "/",
        "/home",
        "/root",
        "/usr",
        "/etc",
        "/var",
        "/opt",
        "/tmp",
        "/Users",
        "/Applications",
        "/System",
        "/Library",
    ]
    .iter()
    .map(|item| item.to_string())
    .collect();
    if let Some(home) = home {
        patterns.insert(normalize(home));
        for relative in [
            ".local",
            ".var",
            ".local/bin",
            ".local/share/applications",
            ".local/share/icons",
            ".local/share/Trash",
            ".local/share/flatpak",
            "Applications",
            "Library",
            ".Trash",
        ] {
            patterns.insert(normalize(&home.join(relative)));
        }
        for root in linux_residue_roots(home)
            .into_iter()
            .chain(macos_residue_roots(home))
        {
            patterns.insert(normalize(&root.path));
        }
    }
    for dir in [
        dirs::desktop_dir(),
        dirs::document_dir(),
        dirs::download_dir(),
        dirs::picture_dir(),
        dirs::audio_dir(),
        dirs::video_dir(),
        dirs::template_dir(),
        dirs::public_dir(),
    ]
    .into_iter()
    .flatten()
    {
        patterns.insert(normalize(&dir));
    }
    patterns.into_iter().collect()
}

pub fn shell_folder_patterns() -> Vec<String> {
    shell_folder_patterns_for(dirs::home_dir().as_deref())
}

/// Gate puro de segurança: `None` quando a remoção é aceitável.
pub fn veto_with_home(
    normalized: &str,
    home: &Path,
    shell_patterns: &[String],
    other_install_dirs: &[String],
) -> Option<String> {
    let text = normalized.trim();
    if text.is_empty() {
        return Some("Caminho vazio".to_string());
    }
    if !text.starts_with('/') {
        return Some("Caminho não absoluto".to_string());
    }
    if text.contains('*')
        || text.contains('?')
        || text.split('/').any(|part| part == ".." || part == ".")
    {
        return Some("Caminho com curinga ou travessia de diretório".to_string());
    }
    let candidate = normalize(Path::new(text));
    if candidate == "/" {
        return Some("A raiz do sistema nunca é removida".to_string());
    }
    let roots = residue_roots(home);
    let Some(root) = roots.iter().find(|root| {
        let root_text = normalize(&root.path);
        candidate.starts_with(&format!("{root_text}/"))
    }) else {
        return Some(
            "Fora das pastas de dados do usuário; o Pulse só remove resíduos dentro delas"
                .to_string(),
        );
    };
    if shell_patterns.contains(&candidate) {
        return Some("Pasta conhecida do sistema".to_string());
    }
    let root_text = normalize(&root.path);
    let item_name = candidate[root_text.len() + 1..]
        .split('/')
        .next()
        .unwrap_or_default();
    if is_forbidden_name(item_name) {
        return Some(format!("Pasta compartilhada do ambiente ({item_name})"));
    }
    for other in other_install_dirs {
        let other = normalize(Path::new(other));
        if other.is_empty() || other == "/" {
            continue;
        }
        if candidate == other {
            return Some("É a pasta de instalação de outro programa".to_string());
        }
        if other.starts_with(&format!("{candidate}/")) {
            return Some("Contém a instalação de outro programa".to_string());
        }
    }
    None
}

pub fn folder_deletion_veto(
    normalized: &str,
    shell_patterns: &[String],
    other_install_dirs: &[String],
) -> Option<String> {
    let Some(home) = dirs::home_dir() else {
        return Some("Pasta pessoal não encontrada".to_string());
    };
    veto_with_home(normalized, &home, shell_patterns, other_install_dirs)
}

/// Id técnico do app (o nome que Flatpak/Snap/macOS usam nas pastas dele).
pub fn primary_id(app: &InstalledApp) -> Option<String> {
    let parsed = parse_id(&app.id)?;
    match parsed.source.as_str() {
        SOURCE_FLATPAK => parsed.key.split('/').nth(1).map(str::to_string),
        SOURCE_SNAP => Some(parsed.key),
        SOURCE_MAC_APP => app
            .match_names
            .first()
            .filter(|name| name.contains('.'))
            .cloned(),
        _ => None,
    }
}

/// Nomes exatos aceitos em raízes `AnyName`. Flatpak roda em sandbox: os
/// dados dele só ficam em `~/.var/app/<id>`, então o nome de exibição não
/// conta (casaria com a pasta de um app nativo homônimo).
pub fn candidate_names(app: &InstalledApp) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        let trimmed = value.trim();
        if is_forbidden_name(trimmed) || names.iter().any(|name| name.eq_ignore_ascii_case(trimmed))
        {
            return;
        }
        names.push(trimmed.to_string());
    };
    for name in &app.match_names {
        push(name);
    }
    let is_flatpak = app.source.as_deref() == Some(SOURCE_FLATPAK);
    if !is_flatpak {
        push(&normalize_display_name(&app.display_name));
    }
    names
}

/// Nome (minúsculo) → programa, para todos os **outros** apps instalados.
pub fn other_app_names(app_id: &str, apps: &[InstalledApp]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for other in apps.iter().filter(|other| other.id != app_id) {
        for name in candidate_names(other) {
            map.entry(name.to_lowercase())
                .or_insert_with(|| other.display_name.clone());
        }
        if let Some(id) = primary_id(other) {
            map.entry(id.to_lowercase())
                .or_insert_with(|| other.display_name.clone());
        }
    }
    map
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataFolder {
    pub path: PathBuf,
    pub reason: String,
    pub blocked: Option<String>,
}

fn accepted_names(rule: NameRule, app: &InstalledApp, names: &[String]) -> Vec<String> {
    match rule {
        NameRule::AnyName => names.to_vec(),
        NameRule::IdOnly(source) => {
            if app.source.as_deref() == Some(source) {
                primary_id(app).into_iter().collect()
            } else {
                Vec::new()
            }
        }
        NameRule::SavedState => {
            if app.source.as_deref() == Some(SOURCE_MAC_APP) {
                primary_id(app)
                    .map(|id| vec![format!("{id}.savedState")])
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
    }
}

/// Fonte única de "pastas de dados com o nome do app" — usada pelos
/// resíduos e pela medição de tamanho, para as duas nunca divergirem. Só
/// diretórios reais (link simbólico fica de fora) e com o veto aplicado.
pub fn data_folder_candidates(
    app: &InstalledApp,
    home: &Path,
    shell_patterns: &[String],
    other_dirs: &[String],
    other_names: &HashMap<String, String>,
) -> Vec<DataFolder> {
    let names = candidate_names(app);
    let mut found: Vec<DataFolder> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for root in residue_roots(home) {
        let accepted = accepted_names(root.rule, app, &names);
        if accepted.is_empty() {
            continue;
        }
        let Ok(iter) = fs::read_dir(&root.path) else {
            continue;
        };
        for entry in iter.flatten().take(ROOT_SCAN_MAX_ENTRIES) {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let Some(matched) = accepted
                .iter()
                .find(|name| name.eq_ignore_ascii_case(&file_name))
            else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            let normalized = normalize(&path);
            if !seen.insert(normalized.clone()) {
                continue;
            }
            let blocked =
                veto_with_home(&normalized, home, shell_patterns, other_dirs).or_else(|| {
                    other_names.get(&file_name.to_lowercase()).map(|other| {
                        format!("O nome também identifica outro programa instalado ({other})")
                    })
                });
            found.push(DataFolder {
                path,
                reason: format!("Nome exato do aplicativo ({matched}) em {}", root.label),
                blocked,
            });
        }
    }
    found
}

/// Varredura de resíduos do app (etapa 1 e revarredura após a remoção).
pub fn scan(app: &InstalledApp) -> UninstallTargets {
    let mut targets = UninstallTargets {
        app_id: app.id.clone(),
        display_name: app.display_name.clone(),
        publisher: app.publisher.clone(),
        version: app.version.clone(),
        uninstall_string: app.uninstall_string.clone(),
        install_location: app.install_location.clone(),
        is_msi: false,
        registry_keys: Vec::new(),
        folders: Vec::new(),
        skipped_reason: None,
    };
    let Some(home) = dirs::home_dir() else {
        targets.skipped_reason =
            Some("Pasta pessoal não encontrada; resíduos não foram analisados.".to_string());
        return targets;
    };
    let shell_patterns = shell_folder_patterns_for(Some(&home));
    let inventory = crate::uninstall::installed::collect_cached().apps;
    let other_dirs = super::other_install_dirs_excluding(&app.id);
    let other_names = other_app_names(&app.id, &inventory);
    for folder in data_folder_candidates(app, &home, &shell_patterns, &other_dirs, &other_names) {
        let normalized = normalize(&folder.path);
        let mut budget = TREE_MAX_ENTRIES;
        targets.folders.push(build_folder_node(
            &folder.path,
            &normalized,
            folder.reason,
            folder.blocked,
            0,
            &mut budget,
        ));
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uninstall::installed_unix::{base_app, SOURCE_DEB};

    fn home() -> PathBuf {
        PathBuf::from("/home/teste")
    }

    fn shell() -> Vec<String> {
        shell_folder_patterns_for(Some(&home()))
    }

    #[test]
    fn normalizacao_lexical() {
        assert_eq!(
            normalize(Path::new("/home/u/.config/App/")),
            "/home/u/.config/App"
        );
        assert_eq!(
            normalize(Path::new("/home/u/./.config//App")),
            "/home/u/.config/App"
        );
        assert_eq!(
            normalize(Path::new("/home/u/.config/x/../App")),
            "/home/u/.config/App"
        );
        assert_eq!(normalize(Path::new("/")), "/");
    }

    #[test]
    fn contencao_de_arvore_por_componente() {
        assert!(is_inside_tree(
            "/home/u/.config/Foto",
            "/home/u/.config/Foto/sub/a"
        ));
        assert!(is_inside_tree(
            "/home/u/.config/Foto",
            "/home/u/.config/Foto"
        ));
        assert!(!is_inside_tree(
            "/home/u/.config/Foto",
            "/home/u/.config/Fotografia"
        ));
        assert!(!is_inside_tree(
            "/home/u/.config/Foto",
            "/home/u/.config/foto"
        ));
        assert!(!is_inside_tree("/", "/etc"));
        assert!(!is_inside_tree("", "/etc"));
    }

    #[test]
    fn veto_fora_do_home_e_nas_raizes_xdg() {
        let home = home();
        for path in [
            "/etc/foo",
            "/usr/share/foo",
            "/home/teste",
            "/home/teste/.config",
            "/home/teste/.local/share",
            "/home/teste/.var/app",
            "/home/teste/Documentos/App",
            "/home/teste/.ssh",
            "/",
            "",
            "relativo/App",
            "/home/teste/.config/*",
            "/home/teste/.config/../.ssh",
        ] {
            assert!(
                veto_with_home(path, &home, &shell(), &[]).is_some(),
                "{path} deveria ser bloqueado"
            );
        }
    }

    #[test]
    fn veto_em_nome_generico_e_em_pasta_de_outro_programa() {
        let home = home();
        assert!(veto_with_home(
            "/home/teste/.local/share/applications",
            &home,
            &shell(),
            &[]
        )
        .is_some());
        assert!(veto_with_home("/home/teste/.config/autostart", &home, &shell(), &[]).is_some());
        assert!(veto_with_home("/home/teste/.local/share/flatpak", &home, &shell(), &[]).is_some());
        let other = vec!["/home/teste/.local/share/Outro/bin".to_string()];
        assert!(
            veto_with_home("/home/teste/.local/share/Outro", &home, &shell(), &other).is_some()
        );
        assert!(veto_with_home("/home/teste/.config/MeuApp", &home, &shell(), &other).is_none());
        assert!(veto_with_home("/home/teste/.var/app/org.x.App", &home, &shell(), &[]).is_none());
    }

    #[test]
    fn nomes_do_flatpak_sao_so_o_id() {
        let mut app = base_app(
            SOURCE_FLATPAK,
            "system",
            "app/com.rtosta.zapzap/x86_64/stable",
            "ZapZap".to_string(),
            String::new(),
        );
        app.match_names = vec!["com.rtosta.zapzap".to_string()];
        assert_eq!(primary_id(&app).as_deref(), Some("com.rtosta.zapzap"));
        assert_eq!(candidate_names(&app), vec!["com.rtosta.zapzap".to_string()]);
        assert_eq!(
            accepted_names(
                NameRule::IdOnly(SOURCE_FLATPAK),
                &app,
                &candidate_names(&app)
            ),
            vec!["com.rtosta.zapzap".to_string()]
        );
        assert!(
            accepted_names(NameRule::IdOnly(SOURCE_SNAP), &app, &candidate_names(&app)).is_empty()
        );
    }

    #[test]
    fn nomes_do_deb_incluem_pacote_atalho_e_nome_de_exibicao() {
        let mut app = base_app(
            SOURCE_DEB,
            "system",
            "gimp",
            "GIMP 2.10".to_string(),
            String::new(),
        );
        app.match_names = vec![
            "gimp".to_string(),
            "gimp".to_string(),
            "applications".to_string(),
        ];
        let names = candidate_names(&app);
        assert_eq!(
            names,
            vec!["gimp".to_string()],
            "duplicado e genérico saem; GIMP == gimp"
        );
        assert!(primary_id(&app).is_none());
    }

    #[test]
    fn pastas_de_dados_reais_com_bloqueio_por_conflito_de_nome() {
        let home = std::env::temp_dir().join(format!("pulse-residue-unix-{}", std::process::id()));
        fs::create_dir_all(home.join(".config/meuapp/sub")).unwrap();
        fs::create_dir_all(home.join(".cache/MeuApp")).unwrap();
        fs::create_dir_all(home.join(".config/meuapp-extra")).unwrap();
        fs::create_dir_all(home.join(".local/share/compartilhado")).unwrap();
        fs::create_dir_all(home.join(".var/app/meuapp")).unwrap();
        fs::create_dir_all(home.join(".local/state")).unwrap();
        std::os::unix::fs::symlink(
            home.join(".config/meuapp"),
            home.join(".local/state/meuapp"),
        )
        .ok();

        let mut app = base_app(
            SOURCE_DEB,
            "system",
            "meuapp",
            "Meu App".to_string(),
            String::new(),
        );
        app.match_names = vec!["meuapp".to_string(), "compartilhado".to_string()];
        let mut others = HashMap::new();
        others.insert("compartilhado".to_string(), "Outro".to_string());
        let shell = shell_folder_patterns_for(Some(&home));
        let found = data_folder_candidates(&app, &home, &shell, &[], &others);
        fs::remove_dir_all(&home).ok();

        let paths: Vec<String> = found.iter().map(|folder| normalize(&folder.path)).collect();
        let base = normalize(&home);
        assert!(paths.contains(&format!("{base}/.config/meuapp")));
        assert!(
            paths.contains(&format!("{base}/.cache/MeuApp")),
            "nome exato sem diferenciar caixa"
        );
        assert!(
            !paths.iter().any(|path| path.ends_with("meuapp-extra")),
            "prefixo não é nome exato"
        );
        assert!(
            !paths.iter().any(|path| path.contains(".var/app")),
            "deb não usa ~/.var/app"
        );
        assert!(
            !paths.iter().any(|path| path.contains(".local/state")),
            "link simbólico fica de fora"
        );
        let shared = found
            .iter()
            .find(|folder| folder.path.ends_with("compartilhado"))
            .expect("pasta com conflito aparece, mas bloqueada");
        assert!(shared.blocked.as_deref().unwrap().contains("Outro"));
        assert!(found
            .iter()
            .filter(|folder| !folder.path.ends_with("compartilhado"))
            .all(|folder| folder.blocked.is_none()));
    }
}
