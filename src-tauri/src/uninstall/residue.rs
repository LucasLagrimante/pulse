//! Rastreamento de resíduos (registro e pastas) e sua remoção segura.
//!
//! ## Princípio: prova por nome exato, nunca por semelhança
//!
//! A decisão conservadora combinada com o usuário é: um resíduo só entra na
//! lista quando o **nome exato** da chave/pasta coincide com o nome real da
//! instalação (`InstallLocation` / executável do desinstalador) ou com o
//! `DisplayName` normalizado. Nada é oferecido "por ser parecido".
//!
//! ## Por que isso importa mais do que parece
//!
//! Apagar `C:\Program Files\Common Files` ou `HKLM\SOFTWARE\Microsoft` quebra
//! o Windows silenciosamente e a Recuperação de Arquivos do Windows **não**
//! devolve chaves de registro. Por isso todo item passa por três filtros:
//!
//! 1. `folder_deletion_veto` — sistema, pasta compartilhada, link simbólico,
//!    ancestral de outro app instalado;
//! 2. `matches_snapshot` — o conteúdo é exatamente o que a varredura viu;
//! 3. confirmação explícita do usuário (nada vem marcado).
//!
//! ## Rastreabilidade
//!
//! Cada candidato carrega `reason` e `confidence`, e o snapshot registra o que
//! existia no momento da varredura. Nada é apagado sem que o backend possa
//! provar que o item é o mesmo que o usuário viu.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::installed::{parse_app_id, InstalledApp};
use super::registry::{Hive, RegHandle, RegistryView, ERROR_FILE_NOT_FOUND_CODE};

/// Profundidade e volume da árvore enviada ao front. Além disso a árvore é
/// podada — mandar 40 mil nós para o DOM trava a janela.
const TREE_MAX_DEPTH: usize = 12;
const TREE_MAX_ENTRIES: usize = 4000;
/// Um diretório com mais que isso é Surely "AppData\Temp" ou similar: exige
/// confirmação extra, então entra como item único sem lista de filhos.
const TREE_CHILDREN_CAP: usize = 400;
/// Orçamentos de remoção: passado isso, a operação para e reporta o que falta.
const REMOVE_DEADLINE: Duration = Duration::from_secs(60);
const REMOVE_YIELD_EVERY: usize = 32;
/// Quantidade de itens levados no snapshot de uma pasta. Acima disso o snapshot
/// fica marcado como incompleto e a remoção é recusada.
const SNAPSHOT_MAX_ENTRIES: usize = 20_000;
/// Teto de itens percorridos ao medir o tamanho de uma pasta.
const MEASURE_MAX_ENTRIES: usize = 20_000;

pub const CONFIDENCE_HIGH: &str = "high";
pub const CONFIDENCE_MEDIUM: &str = "medium";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegistryKeyRef {
    pub id: String,
    pub hive: String,
    pub view: String,
    pub path: String,
    pub reason: String,
    pub confidence: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FolderNode>>,
    /// Texto explicando por que a remoção é proibida; ausente = removível.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallTargets {
    pub app_id: String,
    pub display_name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub uninstall_string: Option<String>,
    pub install_location: Option<String>,
    pub is_msi: bool,
    pub registry_keys: Vec<RegistryKeyRef>,
    pub folders: Vec<FolderNode>,
    pub skipped_reason: Option<String>,
}

/// Raízes de registro ondeclé de app costuma ficar por baixo de um nome de
/// fabricante. Nome exato: nenhum "começa com", nenhum curinga.
const VENDOR_REGISTRY_ROOTS: &[&str] = &[
    "Adobe",
    "Dropbox",
    "Git",
    "Google",
    "JavaSoft",
    "Mozilla",
    "NVIDIA Corporation",
    "OpenVPN",
    "Spotify",
    "TeamViewer",
    "The Settlers",
    "WinRAR",
    "7-Zip",
];

/// Subchaves de registro que **nunca** podem ser oferecidas como resíduo, mesmo
/// com nome exato: pertencem ao Windows ou são compartilhadas por todo o sistema.
const REGISTRY_NEVER_SCAN: &[&str] = &[
    "Classes",
    "Microsoft",
    "Microsoft Corporation",
    "Policies",
    "WOW6432Node",
    "Windows",
    "Windows NT",
];

pub fn never_scan_registry_root(name: &str) -> bool {
    REGISTRY_NEVER_SCAN
        .iter()
        .any(|blocked| name.eq_ignore_ascii_case(blocked))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Pastas conhecidas do shell. Comparar por conjunto de componentes elimina
/// qualquer variação de barra final, caixa ou caminho curto.
pub fn shell_folder_patterns() -> Vec<String> {
    let mut patterns: BTreeSet<String> = BTreeSet::new();
    for variable in [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "WINDIR",
    ] {
        if let Some(path) = env_path(variable) {
            patterns.insert(path.to_string_lossy().to_lowercase());
        }
    }
    patterns.extend(
        [
            r"C:\Windows",
            r"C:\Program Files",
            r"C:\Program Files (x86)",
            r"C:\ProgramData",
            r"C:\Users",
        ]
        .iter()
        .map(|value| value.to_lowercase()),
    );
    patterns.into_iter().collect()
}

/// Subpastas que vivem dentro de uma raiz do sistema e são compartilhadas por
/// qualquer software instalado.
const SHARED_SUBFOLDERS: &[&str] = &[
    "common files",
    "windowsapps",
    "uninstall tools",
    "windows defender",
    "windows defender advanced threat protection",
    "windows nt",
    "microsoft shared",
    "windows installer",
    "installer",
    "package cache",
    "microsoft.net",
    "dotnet",
    "reference assemblies",
];

/// Segmentos que nunca podem ser nome de resíduo de app.
const SYSTEM_SEGMENTS: &[&str] = &["windows", "system32", "syswow64", "boot", "recovery"];

/// Motivos de veto, em ordem de severidade. Retorna `None` quando a remoção é
/// aceitável. Função pura: é o gate de segurança e precisa ser testável sem
/// tocar o disco.
pub fn folder_deletion_veto(
    normalized: &str,
    shell_patterns: &[String],
    other_install_dirs: &[String],
) -> Option<String> {
    let lowered = normalized.trim_end_matches('\\').to_lowercase();

    if lowered.is_empty() {
        return Some("Caminho vazio".to_string());
    }
    // Absoluto no Windows = letra de unidade + ":\\" (`C:\\...`). Um caminho
    // iniciado por "\\\\" é UNC de rede: fora do escopo deste módulo.
    let has_drive = lowered
        .chars()
        .nth(1)
        .map(|character| character == ':')
        .unwrap_or(false);
    let starts_with_slash = lowered.starts_with('\\');
    if !has_drive || starts_with_slash {
        return Some("Caminho não absoluto do Windows".to_string());
    }
    if lowered.contains("..") || lowered.contains("*") || lowered.contains("?") {
        return Some("Caminho com curinga ou travessia de diretório".to_string());
    }
    // Raiz de unidade ("c:\") nunca é resíduo de app. `get(3..)` porque
    // `trim_end_matches` já comeu a barra: "c:\" chega aqui como "c:" e o
    // slice `lowered[3..]` entra em pânico.
    if !lowered.get(3..).unwrap_or_default().contains('\\') {
        return Some("A raiz da unidade nunca é removida".to_string());
    }

    let segments: Vec<&str> = lowered
        .split('\\')
        .filter(|part| !part.is_empty())
        .collect();
    for &blocked in SYSTEM_SEGMENTS {
        if segments.contains(&blocked) {
            return Some(format!("Caminho do sistema ({})", blocked));
        }
    }
    for &shared in SHARED_SUBFOLDERS {
        if segments.contains(&shared) {
            return Some(format!("Pasta compartilhada ({})", shared));
        }
    }

    for pattern in shell_patterns {
        if lowered == *pattern {
            return Some("Pasta conhecida do Windows".to_string());
        }
    }

    for other in other_install_dirs {
        let other = other.trim_end_matches('\\').to_lowercase();
        if other.is_empty() {
            continue;
        }
        if lowered == other {
            return Some("É a pasta de instalação de outro programa".to_string());
        }
        if lowered.len() < other.len() && other.starts_with(&lowered) {
            return Some("Contém a instalação de outro programa".to_string());
        }
    }

    None
}

/// Link simbólico/junction: seguir um para fora pode apagar a árvore original.
pub fn is_reparse_point(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// Normaliza separadores, remove barra final e resolve `.` — sem canonicalizar,
/// porque `canonicalize` exigiria que o caminho já existisse (e pode seguir
/// link, que é justamente o que queremos recusar).
pub fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(&text).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            // `Prefix("C:")` e `RootDir("\\")` separados produziriam
            // `C:\\Program Files` com barra duplicada, quebrando a comparação
            // com as pastas conhecidas do shell e com o veto por ancestral.
            Component::Prefix(prefix) => {
                parts.push(prefix.as_os_str().to_string_lossy().to_string())
            }
            Component::RootDir => {
                if parts.is_empty() {
                    parts.push("\\".to_string());
                }
            }
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
        }
    }
    let joined = parts.join("\\");
    let collapsed = joined.replace("\\\\", "\\");
    if collapsed.len() > 3 {
        collapsed.trim_end_matches('\\').to_string()
    } else {
        collapsed
    }
}

/// Nomes exatos que identificam este app. Fontes, em ordem de confiança:
///
/// 1. basename do `InstallLocation` (o nome real da pasta do produto);
/// 2. basename do executável do desinstalador (`unins000.exe` → `unins000.exe`
///    não serve, mas `C:\Program Files\Foto\unins.exe` → `Foto`);
/// 3. `DisplayName` normalizado e, se diferentes, sem sufixo de versão.
pub fn candidate_names(app: &InstalledApp) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut push = |value: String| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() || names.iter().any(|name| name.eq_ignore_ascii_case(&trimmed)) {
            return;
        }
        if is_forbidden_name(&trimmed) {
            return;
        }
        names.push(trimmed);
    };

    if let Some(location) = &app.install_location {
        let normalized = normalize(Path::new(location));
        // `InstallLocation` apontando para dentro do Windows não gera
        // candidato: "C:\Windows\System32"-produziria o nome "System32", que
        // é exatamente o nome de pasta que o veto de sistema proíbe apagar.
        let inside_system = normalized
            .split('\\')
            .any(|segment| SYSTEM_SEGMENTS.contains(&segment.to_lowercase().as_str()));
        if !inside_system {
            if let Some(basename) = normalized.rsplit('\\').next() {
                push(basename.to_string());
            }
        }
    }
    if let Some(command) = &app.uninstall_string {
        let quoted = command.split('"').collect::<Vec<_>>();
        let program = if quoted.len() >= 2 {
            quoted[1].to_string()
        } else {
            command
                .split(char::is_whitespace)
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let normalized = normalize(Path::new(&program));
        if let Some(parent) = normalized.rsplit_once('\\').map(|(head, _)| head) {
            if let Some(basename) = parent.rsplit('\\').next() {
                push(basename.to_string());
            }
        }
    }
    push(normalize_display_name(&app.display_name));

    names
}

/// `DisplayName` costuma vir como "Foto Studio 3.1.2 (x64)". A forma
/// normalizada é usada apenas como nome **exato** de chave/pasta — nunca como
/// padrão de busca.
pub fn normalize_display_name(display_name: &str) -> String {
    let without_parenthesized = strip_parenthesized(display_name);
    let mut text = without_parenthesized.trim().to_string();
    if let Some(position) = text.rfind(' ') {
        let tail = &text[position + 1..];
        // Só remove quando o sufixo parece versão ("3.1.2", "23.01").
        // "Adobe Photoshop 2024" é nome de produto: o ano faz parte do
        // diretório real e removê-lo faria o resíduo real não casar.
        if tail.contains('.')
            && tail
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
            && tail.chars().any(|character| character.is_ascii_digit())
        {
            text = text[..position].trim().to_string();
        }
    }
    text
}

fn strip_parenthesized(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut depth = 0usize;
    for character in value.chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => output.push(character),
            _ => {}
        }
    }
    output
}

fn is_forbidden_name(name: &str) -> bool {
    const BLOCKED: &[&str] = &[
        "windows",
        "microsoft",
        "program files",
        "program files (x86)",
        "programdata",
        "users",
        "desktop",
        "documents",
        "downloads",
        "pictures",
        "music",
        "videos",
        "temp",
        "tmp",
        "common",
        "shared",
        "packages",
        "windowsapps",
        "roaming",
        "local",
        "appdata",
        "installshield",
    ];
    let lowered = name.trim().to_lowercase();
    BLOCKED.iter().any(|blocked| lowered == *blocked)
}

/// Só uma chave Called exatamente com o nome do produto conta como resíduo.
pub fn registry_name_matches(candidate_name: &str, names: &[String]) -> bool {
    names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(candidate_name))
}

/// Pastas de instalação de todos os **outros** programas: é o que impede o
/// Pulse de oferecer a remoção de algo que outro app ainda usa.
pub fn other_install_dirs_excluding(app_id: &str) -> Vec<String> {
    super::installed::collect_cached()
        .apps
        .into_iter()
        .filter(|other| other.id != app_id)
        .filter_map(|other| {
            other
                .install_location
                .map(|location| normalize(Path::new(&location)))
        })
        .collect()
}

fn other_install_dirs(app: &InstalledApp) -> Vec<String> {
    other_install_dirs_excluding(&app.id)
}

pub fn scan(app: &InstalledApp) -> UninstallTargets {
    let names = candidate_names(app);
    let shell_patterns = shell_folder_patterns();
    let other_dirs = other_install_dirs(app);

    let registry_keys = scan_registry(app, &names);
    let folders = scan_folders(app, &names, &shell_patterns, &other_dirs);

    UninstallTargets {
        app_id: app.id.clone(),
        display_name: app.display_name.clone(),
        publisher: app.publisher.clone(),
        version: app.version.clone(),
        uninstall_string: app.uninstall_string.clone(),
        install_location: app.install_location.clone(),
        is_msi: app.is_msi,
        skipped_reason: None,
        registry_keys,
        folders,
    }
}

fn scan_registry(app: &InstalledApp, names: &[String]) -> Vec<RegistryKeyRef> {
    let Some((app_hive, app_view, app_subkey)) = parse_app_id(&app.id) else {
        return Vec::new();
    };
    let mut found: Vec<RegistryKeyRef> = Vec::new();

    // 1. A própria chave de desinstalação, se o instalador não a removeu. É o
    // resíduo mais confiável que existe: o próprio produto a criou.
    if let Some(parent) = app_subkey
        .rsplit_once('\\')
        .map(|(head, _)| head.to_string())
    {
        if parent.contains("CurrentVersion\\Uninstall") {
            found.push(registry_ref(
                app_hive,
                app_view,
                &app_subkey,
                "Chave de desinstalação que o próprio instalador criou e não removeu",
                CONFIDENCE_HIGH,
            ));
        }
        let _ = parent;
    }

    // 2. Subchaves diretas com nome exato, nas raízes de software.
    for (hive, view) in [
        (Hive::LocalMachine, RegistryView::Native64),
        (Hive::LocalMachine, RegistryView::Native32),
        (Hive::CurrentUser, RegistryView::Native64),
        (Hive::CurrentUser, RegistryView::Native32),
    ] {
        let software = match RegHandle::open(hive, "SOFTWARE", view) {
            Ok(handle) => handle,
            Err(_) => continue,
        };
        let keys = match software.enum_keys() {
            Ok(keys) => keys,
            Err(_) => continue,
        };
        for key in keys {
            if never_scan_registry_root(&key) {
                continue;
            }
            if registry_name_matches(&key, names) {
                found.push(registry_ref(
                    hive,
                    view,
                    &format!("SOFTWARE\\{}", key),
                    &format!("Chave de software com o nome exato do aplicativo ({})", key),
                    CONFIDENCE_HIGH,
                ));
                continue;
            }
            let vendor = match VENDOR_REGISTRY_ROOTS
                .iter()
                .find(|vendor| vendor.eq_ignore_ascii_case(&key))
            {
                Some(vendor) => *vendor,
                None => continue,
            };
            // Subchave do fabricante com nome exato — só quando o fabricante
            // não tem chave própria com esse nome, senão a remoção poderia
            // levar apps vizinhos junto.
            let vendor_path = format!("SOFTWARE\\{}", key);
            let Ok(vendor_handle) = RegHandle::open(hive, &vendor_path, view) else {
                continue;
            };
            let Ok(children) = vendor_handle.enum_keys() else {
                continue;
            };
            for child in children {
                if never_scan_registry_root(&child) || !registry_name_matches(&child, names) {
                    continue;
                }
                let path = format!("{}\\{}", vendor_path, child);
                if found
                    .iter()
                    .any(|item| item.path.eq_ignore_ascii_case(&path))
                {
                    continue;
                }
                found.push(registry_ref(
                    hive,
                    view,
                    &path,
                    &format!(
                        "Chave do fabricante {} com o nome exato do aplicativo",
                        vendor
                    ),
                    CONFIDENCE_MEDIUM,
                ));
            }
        }
    }

    found
}

fn registry_ref(
    hive: Hive,
    view: RegistryView,
    subkey: &str,
    reason: &str,
    confidence: &str,
) -> RegistryKeyRef {
    RegistryKeyRef {
        id: format!("{}|{}|{}", hive.label(), subkey, view.label()),
        hive: hive.label().to_string(),
        view: view.label().to_string(),
        path: format!("{}\\{}", hive.label(), subkey),
        reason: reason.to_string(),
        confidence: confidence.to_string(),
        exists: RegHandle::exists(hive, subkey, view),
    }
}

/// Pasta com o nome exato do app numa raiz conhecida. `is_program` separa a
/// pasta do programa (Program Files, `LocalAppData\Programs`) das pastas de
/// dados (AppData, ProgramData) — a medição de tamanho mostra as duas à parte.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedFolder {
    pub path: PathBuf,
    pub label: &'static str,
    pub is_program: bool,
}

/// Fonte única de "pastas com o nome do app": usada tanto pela desinstalação
/// (resíduos) quanto pela medição de tamanho, para as duas nunca divergirem.
/// Só raízes conhecidas, nunca o perfil do usuário (onde "Documentos" mora);
/// candidatos vetados ficam de fora. Não verifica existência.
pub fn named_folder_candidates(
    names: &[String],
    shell_patterns: &[String],
    other_dirs: &[String],
) -> Vec<NamedFolder> {
    let well_known: Vec<(PathBuf, &'static str)> = [
        ("ProgramFiles", "Program Files"),
        ("ProgramFiles(x86)", "Program Files (x86)"),
        ("PROGRAMDATA", "ProgramData"),
        ("LOCALAPPDATA", "AppData local"),
        ("APPDATA", "AppData roaming"),
    ]
    .iter()
    .filter_map(|(variable, label)| env_path(variable).map(|path| (path, *label)))
    .collect();

    let mut found = Vec::new();
    for name in names {
        if is_forbidden_name(name) {
            continue;
        }
        for (root, label) in &well_known {
            let direct = root.join(name);
            let (candidate, is_program) = if *label == "AppData local" {
                let programs = root.join("Programs").join(name);
                if programs.exists() {
                    (programs, true)
                } else {
                    (direct, false)
                }
            } else {
                (direct, label.starts_with("Program Files"))
            };
            if folder_deletion_veto(&normalize(&candidate), shell_patterns, other_dirs).is_some() {
                continue;
            }
            found.push(NamedFolder {
                path: candidate,
                label,
                is_program,
            });
        }
    }
    found
}

fn scan_folders(
    app: &InstalledApp,
    names: &[String],
    shell_patterns: &[String],
    other_dirs: &[String],
) -> Vec<FolderNode> {
    let mut roots: Vec<(PathBuf, String)> = Vec::new();

    if let Some(location) = &app.install_location {
        let normalized = normalize(Path::new(location));
        roots.push((
            PathBuf::from(&normalized),
            "Pasta de instalação declarada pelo programa".to_string(),
        ));
    }

    for candidate in named_folder_candidates(names, shell_patterns, other_dirs) {
        roots.push((
            candidate.path,
            format!("Nome exato do aplicativo dentro de {}", candidate.label),
        ));
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut nodes: Vec<FolderNode> = Vec::new();
    for (path, reason) in roots {
        let normalized = normalize(&path);
        if normalized.is_empty() || !seen.insert(normalized.to_lowercase()) {
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        let blocked = folder_deletion_veto(&normalized, shell_patterns, other_dirs).or_else(|| {
            if is_reparse_point(&path) {
                Some("Link simbólico ou ponto de junção".to_string())
            } else {
                None
            }
        });
        let mut budget = TREE_MAX_ENTRIES;
        nodes.push(build_folder_node(
            &path,
            &normalized,
            reason,
            blocked,
            0,
            &mut budget,
        ));
    }
    nodes
}

/// Constrói a árvore com orçamentos globais: `budget` é o número de nós ainda
/// disponíveis e é decrescido a cada nó emitido.
pub fn build_folder_node(
    path: &Path,
    normalized: &str,
    reason: String,
    blocked: Option<String>,
    depth: usize,
    budget: &mut usize,
) -> FolderNode {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| normalized.to_string());

    if depth >= TREE_MAX_DEPTH {
        return FolderNode {
            id: normalized.to_string(),
            name,
            kind: "folder".to_string(),
            path: normalized.to_string(),
            size_bytes: None,
            child_count: None,
            children: None,
            blocked_reason: blocked,
            truncated: Some("Profundidade máxima da árvore atingida".to_string()),
        };
    }
    if *budget == 0 {
        return FolderNode {
            id: normalized.to_string(),
            name,
            kind: "folder".to_string(),
            path: normalized.to_string(),
            size_bytes: None,
            child_count: None,
            children: None,
            blocked_reason: blocked,
            truncated: Some("Limite de itens da árvore atingido".to_string()),
        };
    }
    *budget -= 1;

    let mut entries: Vec<(PathBuf, String, bool, u64, Option<String>)> = Vec::new();
    match fs::read_dir(path) {
        Ok(iter) => {
            for entry in iter.flatten() {
                if *budget == 0 {
                    break;
                }
                let child_path = entry.path();
                let child_normalized = normalize(&child_path);
                let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
                let size = if is_dir {
                    0
                } else {
                    entry.metadata().map(|meta| meta.len()).unwrap_or(0)
                };
                let link = is_reparse_point(&child_path);
                let child_blocked = if link {
                    Some("Link simbólico ou ponto de junção".to_string())
                } else {
                    None
                };
                entries.push((child_path, child_normalized, is_dir, size, child_blocked));
            }
        }
        Err(error) => {
            return FolderNode {
                id: normalized.to_string(),
                name,
                kind: "folder".to_string(),
                path: normalized.to_string(),
                size_bytes: None,
                child_count: None,
                children: None,
                blocked_reason: Some(format!("Pasta não pôde ser lida ({})", error.kind())),
                truncated: None,
            };
        }
    }

    entries.sort_by(|left, right| right.3.cmp(&left.3).then_with(|| left.1.cmp(&right.1)));
    let overflow = entries.len().saturating_sub(TREE_CHILDREN_CAP);
    entries.truncate(TREE_CHILDREN_CAP);
    let direct_children = entries.len() as u32;

    let mut children = Vec::with_capacity(entries.len());
    for (child_path, child_normalized, is_dir, size, child_blocked) in entries {
        // Não descer em link/junction: o alvo pode estar em outra árvore.
        if is_dir && child_blocked.is_none() {
            children.push(build_folder_node(
                &child_path,
                &child_normalized,
                format!("{} — conteúdo da pasta", reason),
                child_blocked,
                depth + 1,
                budget,
            ));
        } else {
            children.push(FolderNode {
                id: child_normalized.clone(),
                name: Path::new(&child_normalized)
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_else(|| child_normalized.clone()),
                kind: "file".to_string(),
                path: child_normalized,
                size_bytes: Some(size),
                child_count: None,
                children: None,
                blocked_reason: child_blocked,
                truncated: None,
            });
        }
    }

    let mut measure_budget = MEASURE_MAX_ENTRIES;
    let (total, _) = measure_tree(path, &mut measure_budget);
    // Orçamento estourado devolve `None`, não 0: "0 B" seria zero fictício.
    let size_bytes = if measure_budget == 0 {
        None
    } else {
        Some(total)
    };

    FolderNode {
        id: normalized.to_string(),
        name,
        kind: "folder".to_string(),
        path: normalized.to_string(),
        size_bytes,
        child_count: Some(direct_children),
        children: Some(children),
        blocked_reason: blocked,
        truncated: if overflow > 0 {
            Some(format!(
                "{} itens adicionais não listados (limite de {} por pasta)",
                overflow, TREE_CHILDREN_CAP
            ))
        } else {
            None
        },
    }
}

/// Soma de tamanhos da subárvore com teto: um diretório gigante não pode
/// custar minutos de I/O só para a UI exibir um número.
pub fn measure_tree(path: &Path, budget: &mut usize) -> (u64, u32) {
    let mut total = 0u64;
    let mut count = 0u32;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Ok(iter) = fs::read_dir(&current) else {
            continue;
        };
        for entry in iter.flatten() {
            if *budget == 0 {
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            count += 1;
            if kind.is_dir() {
                if !is_reparse_point(&entry.path()) {
                    stack.push(entry.path());
                }
            } else {
                total = total.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0));
            }
        }
    }
    (total, count)
}

// ---------------------------------------------------------------------------
// Remoção
// ---------------------------------------------------------------------------

/// Estado do que existia quando o usuário viu a lista.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEntry {
    pub id: String,
    pub kind: String,
    pub existed: bool,
    /// Caminhos **relativos** de todo o conteúdo, recursivamente. Comparar só
    /// os filhos da raiz deixaria passar arquivo novo criado em subdiretório
    /// (`logs\instalacao.log`), que é justamente o erro caro.
    pub child_names: Vec<String>,
    /// `false` quando o conteúdo não coube no orçamento: aí a remoção é
    /// recusada, porque "não vi tudo" não é prova de que não mudou.
    pub complete: bool,
}

pub fn snapshot_registry(keys: &[RegistryKeyRef]) -> Vec<SnapshotEntry> {
    keys.iter()
        .map(|key| {
            // Id malformado vira "não existia": abrir uma subchave vazia
            // resolveria para a própria raiz do hive e a remoção passaria.
            let existed = match parse_app_id(&key.id) {
                Some((hive, view, subkey)) => RegHandle::exists(hive, &subkey, view),
                None => false,
            };
            SnapshotEntry {
                id: key.id.clone(),
                kind: "registry".to_string(),
                existed,
                child_names: Vec::new(),
                complete: true,
            }
        })
        .collect()
}

pub fn snapshot_folder(root: &Path) -> SnapshotEntry {
    let mut child_names: Vec<String> = Vec::new();
    let mut budget = SNAPSHOT_MAX_ENTRIES;
    if root.is_dir() {
        collect_relative(root, root, &mut child_names, &mut budget);
    }
    child_names.sort();
    SnapshotEntry {
        id: normalize(root),
        kind: "folder".to_string(),
        existed: root.is_dir(),
        child_names,
        complete: budget > 0,
    }
}

fn collect_relative(base: &Path, directory: &Path, out: &mut Vec<String>, budget: &mut usize) {
    let Ok(iter) = fs::read_dir(directory) else {
        return;
    };
    for entry in iter.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let path = entry.path();
        let relative = path
            .strip_prefix(base)
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|_| normalize(&path));
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        out.push(relative.clone());
        if is_dir && !is_reparse_point(&path) {
            collect_relative(base, &path, out, budget);
        }
    }
}

/// O item ainda é exatamente o que a varredura mostrou? Se o uninstaller criou
/// arquivos novos na pasta (log, cache), o conjunto muda e a remoção é
/// recusada — é o momento em que apagar por engano destruiria algo novo.
pub fn matches_snapshot(snapshot: &SnapshotEntry, current: &SnapshotEntry) -> bool {
    if snapshot.kind != current.kind || snapshot.id != current.id {
        return false;
    }
    if !snapshot.existed || !snapshot.complete || !current.complete {
        return false;
    }
    let before: BTreeSet<String> = snapshot.child_names.iter().cloned().collect();
    let now: BTreeSet<String> = current.child_names.iter().cloned().collect();
    before == now
}

/// `Ok(())` quando a chave não existe mais: já foi removida é sucesso, não
/// erro. `Err` traz o motivo real — sem elevação em HKLM, a remoção é recusada
/// pelo Windows e isso precisa aparecer na tela.
pub fn remove_registry_key(key: &RegistryKeyRef) -> Result<(), String> {
    let Some((hive, view, subkey)) = parse_app_id(&key.id) else {
        return Err("Identificador de chave inválido".to_string());
    };
    match RegHandle::delete_key_tree(hive, &subkey, view) {
        Ok(()) => Ok(()),
        Err(super::registry::ERROR_ACCESS_DENIED_CODE) => {
            Err("Permissão negada pelo Windows: HKLM exige o Pulse como administrador".to_string())
        }
        Err(code) if code == ERROR_FILE_NOT_FOUND_CODE => Ok(()),
        Err(code) => Err(format!("Falha ao remover a chave (código {})", code)),
    }
}

/// Remoção recursiva com caminho longo, controlador de orçamento e dispense de I/O.
pub fn remove_folder(
    root: &Path,
    deadline: Instant,
    shell_patterns: &[String],
    other_dirs: &[String],
) -> Result<u64, String> {
    if !root.is_dir() {
        return Ok(0);
    }
    // Revalidação imediatamente antes de escrever no disco: entre a varredura
    // e a limpeza outro programa pode ter sido instalado nesta pasta.
    if let Some(reason) = folder_deletion_veto(&normalize(root), shell_patterns, other_dirs) {
        return Err(format!("Remoção bloqueada: {}", reason));
    }
    if is_reparse_point(root) {
        return Err("A pasta virou link simbólico após a varredura".to_string());
    }
    if Instant::now() >= deadline {
        return Err("Tempo limite de remoção atingido".to_string());
    }

    let mut freed = 0u64;
    let mut processed = 0usize;
    // A ordem importa: remover a árvore mais profunda primeiro evita deixar
    // diretórios para trás.
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut directories: Vec<PathBuf> = Vec::new();

    while let Some(current) = stack.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) => {
                return Err(format!(
                    "Leitura falhou em {} ({})",
                    current.display(),
                    error.kind()
                ))
            }
        };
        for entry in entries.flatten() {
            processed += 1;
            if processed.is_multiple_of(REMOVE_YIELD_EVERY) {
                // Varredura recursiva em disco dispara o filtro de antivírus;
                // ceder a vez mantém o cursor do sistema responsivo.
                std::thread::yield_now();
            }
            if Instant::now() >= deadline {
                return Err("Tempo limite de remoção atingido".to_string());
            }
            let path = entry.path();
            let kind = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            if kind && !is_reparse_point(&path) {
                stack.push(path);
                continue;
            }
            if kind {
                // Junção: remove o próprio link, nunca o alvo.
                remove_link(&path);
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {
                    freed =
                        freed.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0))
                }
                Err(error) => {
                    clear_readonly(&path);
                    match fs::remove_file(&path) {
                        Ok(()) => {
                            freed = freed.saturating_add(
                                entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                            );
                        }
                        Err(_) => {
                            if path.exists() {
                                return Err(format!(
                                    "Arquivo protegido não pôde ser removido: {} ({})",
                                    path.display(),
                                    error.kind()
                                ));
                            }
                        }
                    }
                }
            }
        }
        directories.push(current);
    }

    for directory in directories.into_iter().rev() {
        clear_readonly(&directory);
        if directory.exists() {
            match fs::remove_dir(&directory) {
                Ok(()) => {}
                Err(_) => {
                    if directory.exists() {
                        return Err(format!(
                            "Pasta não pôde ser removida: {}",
                            directory.display()
                        ));
                    }
                }
            }
        }
    }

    Ok(freed)
}

fn remove_link(path: &Path) {
    let _ = fs::remove_dir(path);
}

#[allow(clippy::permissions_set_readonly_false)]
fn clear_readonly(path: &Path) {
    let permissions = match fs::metadata(path) {
        Ok(metadata) => metadata.permissions(),
        Err(_) => return,
    };
    if !permissions.readonly() {
        return;
    }
    let mut writable = permissions.clone();
    writable.set_readonly(false);
    let _ = fs::set_permissions(path, writable);
}

pub fn now_deadline() -> Instant {
    Instant::now() + REMOVE_DEADLINE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Vec<String> {
        vec![
            r"c:\program files".to_string(),
            r"c:\program files (x86)".to_string(),
            r"c:\programdata".to_string(),
            r"c:\users".to_string(),
            r"c:\windows".to_string(),
            r"c:\users\dev\appdata\local".to_string(),
            r"c:\users\dev\appdata\roaming".to_string(),
            r"c:\users\dev".to_string(),
            r"c:\users\dev\appdata".to_string(),
        ]
    }

    #[test]
    fn veto_em_raiz_de_unidade() {
        assert!(folder_deletion_veto(r"c:\", &shell(), &[]).is_some());
    }

    #[test]
    fn veto_em_caminho_do_sistema() {
        for path in [
            r"C:\Windows",
            r"C:\Windows\System32\drivers",
            r"C:\Program Files\Common Files",
            r"C:\Program Files (x86)\Common Files",
            r"C:\Program Files\WindowsApps",
        ] {
            assert!(
                folder_deletion_veto(path, &shell(), &[]).is_some(),
                "{path} deveria ser bloqueado"
            );
        }
    }

    #[test]
    fn veto_em_pasta_conhecida_do_shell() {
        assert!(folder_deletion_veto(r"C:\Program Files", &shell(), &[]).is_some());
        assert!(folder_deletion_veto(r"C:\Users\DEV\AppData\Local", &shell(), &[]).is_some());
    }

    #[test]
    fn veto_em_pasta_de_outro_programa() {
        let other = vec![r"c:\program files\outro app".to_string()];
        assert!(folder_deletion_veto(r"C:\Program Files\Outro App", &shell(), &other).is_some());
        assert!(folder_deletion_veto(r"C:\Program Files", &shell(), &other).is_some());
        assert!(folder_deletion_veto(r"C:\Program Files\Meu App", &shell(), &other).is_none());
    }

    #[test]
    fn veto_em_caminho_relativo_ou_com_curinga() {
        assert!(folder_deletion_veto("Program Files\\App", &shell(), &[]).is_some());
        assert!(folder_deletion_veto(r"C:\Program Files\*", &shell(), &[]).is_some());
        assert!(folder_deletion_veto(r"C:\..\Windows", &shell(), &[]).is_some());
        assert!(folder_deletion_veto("", &shell(), &[]).is_some());
    }

    #[test]
    fn nome_normalizado_de_display() {
        assert_eq!(
            normalize_display_name("Foto Studio 3.1.2 (x64)"),
            "Foto Studio"
        );
        assert_eq!(normalize_display_name("7-Zip 23.01 (x64)"), "7-Zip");
        assert_eq!(normalize_display_name("Notepad++"), "Notepad++");
        assert_eq!(
            normalize_display_name("Adobe Photoshop 2024"),
            "Adobe Photoshop 2024"
        );
    }

    #[test]
    fn normalize_remove_barra_final_e_pontos() {
        assert_eq!(
            normalize(Path::new(r"C:\Program Files\App\")),
            r"C:\Program Files\App"
        );
        assert_eq!(
            normalize(Path::new(r"C:\Program Files\App\.\Bin")),
            r"C:\Program Files\App\Bin"
        );
    }

    #[test]
    fn nomes_candidatos_venem_do_install_dir_e_do_desinstalador() {
        let app = InstalledApp {
            id: "HKLM|SOFTWARE\\X\\Uninstall\\Foto|64".to_string(),
            hive: "HKLM".to_string(),
            view: "64".to_string(),
            registry_path: "HKLM\\SOFTWARE\\X\\Uninstall\\Foto".to_string(),
            display_name: "Foto Studio 3.1.2".to_string(),
            publisher: Some("Empresa".to_string()),
            version: None,
            install_date: None,
            install_location: Some(r"C:\Program Files\Foto".to_string()),
            estimated_size_kb: None,
            uninstall_string: Some(r"C:\Program Files\Foto\unins000.exe".to_string()),
            is_msi: false,
            icon: None,
        };
        let names = candidate_names(&app);
        assert!(names.contains(&"Foto".to_string()));
        assert!(names.contains(&"Foto Studio".to_string()));
    }

    #[test]
    fn nomes_genericos_sao_descartados() {
        let app = InstalledApp {
            id: "HKLM|SOFTWARE\\X\\Uninstall\\P|64".to_string(),
            hive: "HKLM".to_string(),
            view: "64".to_string(),
            registry_path: "HKLM\\P".to_string(),
            display_name: "Windows".to_string(),
            publisher: None,
            version: None,
            install_date: None,
            install_location: Some(r"C:\Windows\System32".to_string()),
            estimated_size_kb: None,
            uninstall_string: None,
            is_msi: false,
            icon: None,
        };
        assert!(candidate_names(&app).is_empty());
    }

    #[test]
    fn matching_de_registro_e_exato_e_case_insensitive() {
        let names = vec!["Foto".to_string(), "Foto Studio".to_string()];
        assert!(registry_name_matches("foto", &names));
        assert!(registry_name_matches("FOTO STUDIO", &names));
        assert!(!registry_name_matches("Fotografia", &names));
        assert!(!registry_name_matches("Studio", &names));
    }

    #[test]
    fn raizes_de_fabricante_bloqueadas_para_varredura() {
        assert!(never_scan_registry_root("Microsoft"));
        assert!(never_scan_registry_root("classes"));
        assert!(!never_scan_registry_root("NVIDIA Corporation"));
    }

    #[test]
    fn snapshot_detecta_conteudo_novo_e_recusa_remocao() {
        let snapshot = SnapshotEntry {
            id: r"C:\Program Files\App".to_string(),
            kind: "folder".to_string(),
            existed: true,
            child_names: vec!["app.exe".to_string()],
            complete: true,
        };
        let unchanged = snapshot.clone();
        assert!(matches_snapshot(&snapshot, &unchanged));
        let mut changed = snapshot.clone();
        changed.child_names.push("novo.log".to_string());
        assert!(!matches_snapshot(&snapshot, &changed));
    }

    #[test]
    fn snapshot_de_item_inexistente_nao_permite_remocao() {
        let snapshot = SnapshotEntry {
            id: r"C:\Program Files\App".to_string(),
            kind: "folder".to_string(),
            existed: false,
            child_names: Vec::new(),
            complete: true,
        };
        let current = snapshot.clone();
        assert!(!matches_snapshot(&snapshot, &current));
    }

    #[test]
    fn snapshot_incompleto_nunca_autoriza_remocao() {
        let snapshot = SnapshotEntry {
            id: r"C:\Program Files\App".to_string(),
            kind: "folder".to_string(),
            existed: true,
            child_names: Vec::new(),
            complete: false,
        };
        let mut current = snapshot.clone();
        current.complete = true;
        assert!(!matches_snapshot(&snapshot, &current));
    }

    #[test]
    fn snapshot_rejeita_tipo_divergente() {
        let snapshot = SnapshotEntry {
            id: "x".to_string(),
            kind: "folder".to_string(),
            existed: true,
            child_names: Vec::new(),
            complete: true,
        };
        let mut current = snapshot.clone();
        current.kind = "registry".to_string();
        assert!(!matches_snapshot(&snapshot, &current));
    }
}
