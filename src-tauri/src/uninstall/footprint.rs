//! Pegada real em disco dos programas instalados.
//!
//! `EstimatedSize` do registro é escrito pelo instalador no dia da instalação
//! e nunca mais atualizado — navegadores, jogos e apps com auto-update crescem
//! muito além dele. Aqui medimos de verdade, com custo controlado:
//!
//! - **pasta do programa**: a declarada em `InstallLocation` ou, quando falta,
//!   uma pasta **deduzida** (pasta do executável do `DisplayIcon` ou pasta com
//!   o nome exato em Program Files) — sempre rotulada como deduzida;
//! - **dados**: pastas com o nome exato em AppData/ProgramData, pela mesma
//!   regra da desinstalação (`residue::named_folder_candidates`), somadas à
//!   parte para não misturar programa com cache/perfil;
//! - modo rápido em lotes (orçamento de entradas + prazo por app e por lote;
//!   estourou, o valor vira **limite inferior**, nunca número inventado) e
//!   modo completo sob demanda, um app por vez;
//! - `spawn_blocking`, `yield_now` a cada 15 entradas, sem seguir junction e
//!   com o veto de pastas de sistema/compartilhadas; cache de 10 minutos.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

#[cfg(windows)]
use super::installed::parse_app_id;
use super::installed::{collect_cached, InstalledApp};
#[cfg(windows)]
use super::registry::RegHandle;
#[cfg(windows)]
use super::residue::{candidate_names, folder_deletion_veto, named_folder_candidates};
use super::residue::{is_reparse_point, normalize, normalize_windows, shell_folder_patterns};

/// Entradas lidas por pasta antes de desistir no modo rápido (cobre apps
/// grandes comuns; jogos com centenas de milhares de arquivos viram mínimo).
const PER_APP_MAX_ENTRIES: usize = 60_000;
/// Prazo de uma pasta dentro do lote, no modo rápido.
const PER_APP_DEADLINE: Duration = Duration::from_millis(1_500);
/// Prazo total de um lote: a UI pede o próximo lote só depois deste.
const BATCH_DEADLINE: Duration = Duration::from_millis(4_000);
/// Máximo de apps aceitos por chamada.
const BATCH_MAX_APPS: usize = 24;
/// Modo completo (pedido pelo usuário para um app): teto alto, mas ainda finito.
const FULL_MAX_ENTRIES: usize = 3_000_000;
const FULL_DEADLINE: Duration = Duration::from_secs(120);
const YIELD_EVERY: usize = 15;
const CACHE_TTL: Duration = Duration::from_secs(600);
/// Texto do protocolo com a UI: o app volta para a fila do próximo lote.
pub const DEFERRED: &str = "Medição adiada";

/// Subpastas genéricas onde o executável costuma morar; a pasta do programa
/// é a de cima (`App\bin\app.exe` → `App`).
const GENERIC_EXE_DIRS: &[&str] = &["bin", "bin64", "bin32", "x64", "x86", "app", "current"];
/// Pastas que guardam instaladores/desinstaladores, não o programa: medir a
/// pasta minúscula do desinstalador e chamar de "tamanho" seria falso.
const NOT_PROGRAM_SEGMENTS: &[&str] = &[
    "package cache",
    "installshield installation information",
    "installer",
    "common files",
    "temp",
    "windowsapps",
    "windows",
    "system32",
    "syswow64",
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LocationSource {
    /// `InstallLocation` gravado pelo instalador.
    Declared,
    /// Inferida pelo Pulse (DisplayIcon ou nome exato em Program Files).
    Deduced,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppFootprint {
    pub app_id: String,
    /// Bytes somados na pasta do programa; `None` quando não houve medição.
    pub measured_bytes: Option<u64>,
    /// `false` quando o orçamento/prazo acabou: `measured_bytes` é mínimo.
    pub complete: bool,
    pub file_count: u64,
    pub install_location: Option<String>,
    pub location_source: Option<LocationSource>,
    /// Soma das pastas de dados (AppData/ProgramData); `None` = nenhuma achada.
    pub data_bytes: Option<u64>,
    pub data_complete: bool,
    pub data_folders: Vec<String>,
    /// `true` quando veio do modo completo (sem o teto do modo rápido).
    pub full_scan: bool,
    /// Motivo honesto quando não há medição (sem pasta, pasta vetada, etc.).
    pub unavailable_reason: Option<String>,
}

#[derive(Clone)]
struct CachedMeasure {
    bytes: u64,
    files: u64,
    complete: bool,
    at: Instant,
}

static CACHE: Mutex<Option<HashMap<String, CachedMeasure>>> = Mutex::new(None);
static FULL_RUNNING: AtomicBool = AtomicBool::new(false);

fn cache_get(key: &str) -> Option<CachedMeasure> {
    let guard = CACHE.lock().ok()?;
    let hit = guard.as_ref()?.get(key)?.clone();
    (hit.at.elapsed() < CACHE_TTL).then_some(hit)
}

fn cache_put(key: String, value: CachedMeasure) {
    if let Ok(mut guard) = CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        // Um resultado completo nunca é rebaixado por um parcial mais novo.
        if let Some(existing) = map.get(&key) {
            if existing.complete && !value.complete && existing.at.elapsed() < CACHE_TTL {
                return;
            }
        }
        map.insert(key, value);
    }
}

/// Resultado da soma limitada: (bytes, arquivos, completo?).
pub fn measure_bounded(root: &Path, max_entries: usize, deadline: Instant) -> (u64, u64, bool) {
    let mut total = 0u64;
    let mut files = 0u64;
    let mut visited = 0usize;
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(iter) = fs::read_dir(&current) else {
            continue;
        };
        for entry in iter.flatten() {
            visited += 1;
            if visited.is_multiple_of(YIELD_EVERY) {
                std::thread::yield_now();
                if Instant::now() >= deadline {
                    return (total, files, false);
                }
            }
            if visited > max_entries {
                return (total, files, false);
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                // Junction/symlink pode apontar para outra árvore (ou para si).
                if !kind.is_symlink() && !is_reparse_point(&entry.path()) {
                    stack.push(entry.path());
                }
            } else if kind.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                    files += 1;
                }
            }
        }
    }
    (total, files, true)
}

/// Vários instaladores gravam `InstallLocation` entre aspas (`"C:\\Program Files\\X"`);
/// sem tirá-las o caminho parece relativo e o veto recusa a medição.
pub fn clean_install_location(raw: &str) -> &str {
    raw.trim().trim_matches('"').trim()
}

/// Pasta do programa a partir do executável do `DisplayIcon`
/// (`"C:\\Apps\\X\\bin\\x.exe",0` → `C:\\Apps\\X`). Recusa desinstaladores e
/// pastas de cache de instalador/sistema. Função pura (não toca o disco).
pub fn program_dir_from_exe(raw: &str) -> Option<String> {
    let mut text = raw.trim().trim_matches('"').trim().to_string();
    // Índice de ícone: `x.exe,0` / `x.exe,-101`.
    if let Some((head, tail)) = text.rsplit_once(',') {
        if tail
            .trim()
            .trim_start_matches('-')
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            text = head.trim().trim_matches('"').to_string();
        }
    }
    let normalized = normalize_windows(Path::new(&text));
    let (dir, file) = normalized.rsplit_once('\\')?;
    let file = file.to_lowercase();
    if !file.ends_with(".exe") || file.starts_with("unins") || file.starts_with("uninst") {
        return None;
    }
    let mut dir = dir.to_string();
    if let Some((parent, base)) = dir.rsplit_once('\\') {
        if GENERIC_EXE_DIRS.contains(&base.to_lowercase().as_str())
            && parent.matches('\\').count() >= 1
        {
            dir = parent.to_string();
        }
    }
    let segments: Vec<String> = dir.split('\\').map(|part| part.to_lowercase()).collect();
    // Precisa estar ao menos em `X:\\Algo\\Programa` ou `X:\\Programa` (não raiz).
    if segments.len() < 2 || segments[1..].iter().any(|part| part.is_empty()) {
        return None;
    }
    if segments
        .iter()
        .any(|part| NOT_PROGRAM_SEGMENTS.contains(&part.as_str()) || part.starts_with('{'))
    {
        return None;
    }
    Some(dir)
}

#[derive(Clone, Copy)]
enum Mode {
    Quick { batch_deadline: Instant },
    Full,
}

/// Mede uma pasta respeitando o modo. `None` = sem tempo neste lote.
fn measure_dir(path: &Path, mode: Mode) -> Option<CachedMeasure> {
    #[cfg(windows)]
    let key = normalize(path).to_lowercase();
    #[cfg(unix)]
    let key = normalize(path);
    match mode {
        Mode::Quick { batch_deadline } => {
            if let Some(hit) = cache_get(&key) {
                return Some(hit);
            }
            let now = Instant::now();
            if now >= batch_deadline {
                return None;
            }
            let deadline = (now + PER_APP_DEADLINE).min(batch_deadline);
            let (bytes, files, complete) = measure_bounded(path, PER_APP_MAX_ENTRIES, deadline);
            let value = CachedMeasure {
                bytes,
                files,
                complete,
                at: Instant::now(),
            };
            cache_put(key, value.clone());
            Some(value)
        }
        Mode::Full => {
            let (bytes, files, complete) =
                measure_bounded(path, FULL_MAX_ENTRIES, Instant::now() + FULL_DEADLINE);
            let value = CachedMeasure {
                bytes,
                files,
                complete,
                at: Instant::now(),
            };
            cache_put(key, value.clone());
            Some(value)
        }
    }
}

struct Context {
    apps: Vec<InstalledApp>,
    shell_patterns: Vec<String>,
    /// `(app_id, pasta normalizada)` de quem declarou `InstallLocation`.
    declared: Vec<(String, String)>,
    #[cfg(unix)]
    home: Option<PathBuf>,
}

impl Context {
    fn load() -> Self {
        let apps = collect_cached().apps;
        let declared = apps
            .iter()
            .filter_map(|app| {
                let location = clean_install_location(app.install_location.as_deref()?);
                (!location.is_empty()).then(|| (app.id.clone(), normalize(Path::new(location))))
            })
            .collect();
        Context {
            apps,
            shell_patterns: shell_folder_patterns(),
            declared,
            #[cfg(unix)]
            home: dirs::home_dir(),
        }
    }

    fn others(&self, app_id: &str) -> Vec<String> {
        self.declared
            .iter()
            .filter(|(id, _)| id != app_id)
            .map(|(_, dir)| dir.clone())
            .collect()
    }
}

#[cfg(windows)]
fn display_icon(app: &InstalledApp) -> Option<String> {
    let (hive, view, subkey) = parse_app_id(&app.id)?;
    RegHandle::open(hive, &subkey, view)
        .ok()?
        .query_string("DisplayIcon")
}

/// Pasta deduzida aceitável: existe, não é link, não é vetada e não é a
/// pasta de outro programa.
#[cfg(windows)]
fn usable_dir(dir: &str, ctx: &Context, others: &[String]) -> bool {
    let path = Path::new(dir);
    path.is_dir()
        && !is_reparse_point(path)
        && folder_deletion_veto(dir, &ctx.shell_patterns, others).is_none()
        && !ctx
            .declared
            .iter()
            .any(|(_, other)| other.eq_ignore_ascii_case(dir))
}

#[cfg(windows)]
fn deduce_program_dir(app: &InstalledApp, ctx: &Context, others: &[String]) -> Option<String> {
    if let Some(dir) = display_icon(app).as_deref().and_then(program_dir_from_exe) {
        if usable_dir(&dir, ctx, others) {
            return Some(dir);
        }
    }
    let names = candidate_names(app);
    if let Some(dir) = named_folder_candidates(&names, &ctx.shell_patterns, others)
        .into_iter()
        .filter(|candidate| candidate.is_program)
        .map(|candidate| normalize(&candidate.path))
        .find(|dir| usable_dir(dir, ctx, others))
    {
        return Some(dir);
    }
    // `Program Files\<Fabricante>\<Programa>` (ex.: `Google\Google Earth Pro`):
    // só nome exato, e só confere existência — custo de um `stat` por tentativa.
    let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|variable| std::env::var_os(variable).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .collect();
    for vendor in vendor_folder_names(app.publisher.as_deref()) {
        for root in &roots {
            for name in &names {
                let dir = normalize(&root.join(&vendor).join(name));
                if usable_dir(&dir, ctx, others) {
                    return Some(dir);
                }
            }
        }
    }
    None
}

/// Nomes de pasta plausíveis para o fabricante: o texto inteiro e sem sufixo
/// societário ("Google LLC" → "Google LLC", "Google"). Vazio quando não há.
pub fn vendor_folder_names(publisher: Option<&str>) -> Vec<String> {
    const SUFFIXES: &[&str] = &[
        "inc",
        "inc.",
        "llc",
        "ltd",
        "ltd.",
        "corp",
        "corp.",
        "corporation",
        "co.",
        "gmbh",
        "s.a.",
        "sa",
        "ag",
        "limited",
        "pty",
        "pte",
    ];
    let Some(text) = publisher.map(str::trim).filter(|text| !text.is_empty()) else {
        return Vec::new();
    };
    let clean = text.trim_end_matches(['.', ',']).to_string();
    let mut words: Vec<&str> = clean.split_whitespace().collect();
    while words.len() > 1 {
        let last = words[words.len() - 1].trim_end_matches(',').to_lowercase();
        if SUFFIXES.contains(&last.as_str()) {
            words.pop();
        } else {
            break;
        }
    }
    let stripped = words.join(" ").trim_end_matches(',').to_string();
    let mut names = vec![clean.clone()];
    if !stripped.is_empty() && !stripped.eq_ignore_ascii_case(&clean) {
        names.push(stripped);
    }
    // Nomes genéricos demais nunca viram pasta de fabricante.
    names.retain(|name| {
        !["microsoft", "windows", "microsoft corporation"].contains(&name.to_lowercase().as_str())
    });
    names
}

fn blank(app_id: &str) -> AppFootprint {
    AppFootprint {
        app_id: app_id.to_string(),
        measured_bytes: None,
        complete: false,
        file_count: 0,
        install_location: None,
        location_source: None,
        data_bytes: None,
        data_complete: false,
        data_folders: Vec::new(),
        full_scan: false,
        unavailable_reason: None,
    }
}

fn unavailable(app_id: &str, location: Option<String>, reason: &str) -> AppFootprint {
    AppFootprint {
        install_location: location,
        unavailable_reason: Some(reason.to_string()),
        ..blank(app_id)
    }
}

fn measure_one(ctx: &Context, app_id: &str, mode: Mode) -> AppFootprint {
    #[cfg(windows)]
    {
        measure_one_windows(ctx, app_id, mode)
    }
    #[cfg(unix)]
    {
        measure_one_unix(ctx, app_id, mode)
    }
}

/// Soma o tamanho real de uma lista de caminhos absolutos (ex.: `.list` do
/// dpkg), contando apenas arquivos regulares (ignora diretórios e symlinks).
#[cfg(unix)]
pub fn measure_file_list<'a, I>(paths: I, max_entries: usize, deadline: Instant) -> (u64, u64, bool)
where
    I: IntoIterator<Item = &'a str>,
{
    let mut total = 0u64;
    let mut files = 0u64;
    let mut visited = 0usize;
    for raw in paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() || !trimmed.starts_with('/') {
            continue;
        }
        visited += 1;
        if visited.is_multiple_of(YIELD_EVERY) {
            std::thread::yield_now();
            if Instant::now() >= deadline {
                return (total, files, false);
            }
        }
        if visited > max_entries {
            return (total, files, false);
        }
        let Ok(meta) = fs::symlink_metadata(trimmed) else {
            continue;
        };
        if meta.file_type().is_file() {
            total = total.saturating_add(meta.len());
            files += 1;
        }
    }
    (total, files, true)
}

#[cfg(target_os = "linux")]
fn measure_deb_package(binary_name: &str, mode: Mode) -> Option<Result<CachedMeasure, String>> {
    const MAX_LIST_BYTES: u64 = 16 * 1024 * 1024;
    let key = format!("deb-list:{binary_name}");
    if matches!(mode, Mode::Quick { .. }) {
        if let Some(hit) = cache_get(&key) {
            return Some(Ok(hit));
        }
    }
    let Some(list_path) = super::packages_linux::dpkg_list_file(binary_name) else {
        return Some(Err(
            "Lista de arquivos do pacote não encontrada em /var/lib/dpkg/info".to_string(),
        ));
    };
    let (max_entries, deadline) = match mode {
        Mode::Quick { batch_deadline } => {
            let now = Instant::now();
            if now >= batch_deadline {
                return None;
            }
            (
                PER_APP_MAX_ENTRIES,
                (now + PER_APP_DEADLINE).min(batch_deadline),
            )
        }
        Mode::Full => (FULL_MAX_ENTRIES, Instant::now() + FULL_DEADLINE),
    };
    let Ok(meta) = fs::metadata(&list_path) else {
        return Some(Err(
            "Não foi possível ler a lista de arquivos do pacote".to_string()
        ));
    };
    if meta.len() > MAX_LIST_BYTES {
        return Some(Err(
            "Lista de arquivos do pacote excede o limite de leitura".to_string(),
        ));
    }
    let Ok(bytes) = fs::read(&list_path) else {
        return Some(Err(
            "Não foi possível ler a lista de arquivos do pacote".to_string()
        ));
    };
    let content = String::from_utf8_lossy(&bytes);
    let (total, files, complete) = measure_file_list(content.lines(), max_entries, deadline);
    let value = CachedMeasure {
        bytes: total,
        files,
        complete,
        at: Instant::now(),
    };
    cache_put(key, value.clone());
    Some(Ok(value))
}

#[cfg(target_os = "linux")]
fn is_valid_flatpak_dir(path: &Path, home: Option<&Path>) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let mut allowed = vec![PathBuf::from("/var/lib/flatpak/app")];
    if let Some(home) = home {
        allowed.push(home.join(".local/share/flatpak/app"));
    }
    allowed.iter().any(|root| root == parent) && path.is_dir() && !is_reparse_point(path)
}

#[cfg(target_os = "linux")]
fn is_valid_snap_dir(path: &Path) -> bool {
    path.parent() == Some(Path::new("/snap")) && path.is_dir() && !is_reparse_point(path)
}

#[cfg(unix)]
fn measure_one_unix(ctx: &Context, app_id: &str, mode: Mode) -> AppFootprint {
    use super::installed_unix::{
        parse_id, SOURCE_APPIMAGE, SOURCE_DEB, SOURCE_FLATPAK, SOURCE_MAC_APP, SOURCE_SNAP,
    };

    let Some(app) = ctx.apps.iter().find(|app| app.id == app_id) else {
        return unavailable(app_id, None, "Programa não está mais no inventário");
    };
    let mut result = blank(app_id);
    let parsed = parse_id(&app.id);

    // 1. Instalação do programa conforme a origem.
    match parsed.as_ref().map(|id| id.source.as_str()) {
        #[cfg(target_os = "linux")]
        Some(SOURCE_DEB) => {
            let key = &parsed.as_ref().unwrap().key;
            match measure_deb_package(key, mode) {
                Some(Ok(measure)) => {
                    result.measured_bytes = Some(measure.bytes);
                    result.complete = measure.complete;
                    result.file_count = measure.files;
                    result.location_source = Some(LocationSource::Declared);
                }
                Some(Err(reason)) => result.unavailable_reason = Some(reason),
                None => return unavailable(app_id, None, DEFERRED),
            }
        }
        #[cfg(target_os = "linux")]
        Some(SOURCE_APPIMAGE) => {
            let key = &parsed.as_ref().unwrap().key;
            let path = Path::new(key);
            result.install_location = Some(key.clone());
            match fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_file() => {
                    result.measured_bytes = Some(meta.len());
                    result.complete = true;
                    result.file_count = 1;
                    result.location_source = Some(LocationSource::Declared);
                }
                _ => {
                    result.unavailable_reason =
                        Some("O arquivo AppImage não existe mais ou virou um link".to_string());
                }
            }
        }
        #[cfg(target_os = "linux")]
        Some(SOURCE_FLATPAK) => {
            if let Some(dir) = app.install_location.as_deref() {
                let normalized = normalize(Path::new(dir));
                let path = Path::new(&normalized);
                result.install_location = Some(normalized.clone());
                let shared = ctx
                    .declared
                    .iter()
                    .any(|(id, other)| id != app_id && other == &normalized);
                if shared {
                    result.unavailable_reason =
                        Some("Pasta compartilhada com outro programa".to_string());
                } else if !is_valid_flatpak_dir(path, ctx.home.as_deref()) {
                    result.unavailable_reason = Some(
                        "Pasta de implantação do Flatpak não encontrada ou inválida".to_string(),
                    );
                } else {
                    match measure_dir(path, mode) {
                        Some(measure) => {
                            result.measured_bytes = Some(measure.bytes);
                            result.complete = measure.complete;
                            result.file_count = measure.files;
                            result.location_source = Some(LocationSource::Declared);
                        }
                        None => return unavailable(app_id, result.install_location, DEFERRED),
                    }
                }
            } else {
                result.unavailable_reason =
                    Some("Pasta de implantação do Flatpak não encontrada".to_string());
            }
        }
        #[cfg(target_os = "linux")]
        Some(SOURCE_SNAP) => {
            if let Some(dir) = app.install_location.as_deref() {
                let normalized = normalize(Path::new(dir));
                let path = Path::new(&normalized);
                result.install_location = Some(normalized.clone());
                if !is_valid_snap_dir(path) {
                    result.unavailable_reason =
                        Some("Pasta de montagem do Snap não encontrada ou inválida".to_string());
                } else {
                    match measure_dir(path, mode) {
                        Some(measure) => {
                            result.measured_bytes = Some(measure.bytes);
                            result.complete = measure.complete;
                            result.file_count = measure.files;
                            result.location_source = Some(LocationSource::Declared);
                        }
                        None => return unavailable(app_id, result.install_location, DEFERRED),
                    }
                }
            } else {
                result.unavailable_reason =
                    Some("Pasta de montagem do Snap não encontrada".to_string());
            }
        }
        Some(SOURCE_MAC_APP) => {
            // TODO(macos): validar em hardware.
            let key = &parsed.as_ref().unwrap().key;
            let path = Path::new(key);
            result.install_location = Some(key.clone());
            let roots: Vec<PathBuf> = super::apps_macos::app_roots(ctx.home.as_deref())
                .into_iter()
                .map(|(root, _)| root)
                .collect();
            match super::uninstall_unix::validate_mac_app_path(path, &roots) {
                Ok(()) => match measure_dir(path, mode) {
                    Some(measure) => {
                        result.measured_bytes = Some(measure.bytes);
                        result.complete = measure.complete;
                        result.file_count = measure.files;
                        result.location_source = Some(LocationSource::Declared);
                    }
                    None => return unavailable(app_id, result.install_location, DEFERRED),
                },
                Err(reason) => result.unavailable_reason = Some(reason),
            }
        }
        _ => {
            result.unavailable_reason =
                Some("Origem de instalação sem medição de disco suportada".to_string());
        }
    }

    // 2. Pastas de dados do usuário com o nome exato (mesma regra dos resíduos).
    if let Some(home) = &ctx.home {
        let others = ctx.others(app_id);
        let other_names = super::residue::unix::other_app_names(app_id, &ctx.apps);
        let mut seen: Vec<String> = Vec::new();
        let mut data_total = 0u64;
        let mut data_complete = true;
        for folder in super::residue::unix::data_folder_candidates(
            app,
            home,
            &ctx.shell_patterns,
            &others,
            &other_names,
        ) {
            if folder.blocked.is_some() {
                continue;
            }
            let dir = normalize(&folder.path);
            let inside_program = result
                .install_location
                .as_deref()
                .is_some_and(|loc| super::residue::unix::is_inside_tree(loc, &dir));
            if inside_program || seen.contains(&dir) {
                continue;
            }
            let path = Path::new(&dir);
            if !path.is_dir() || is_reparse_point(path) {
                continue;
            }
            match measure_dir(path, mode) {
                Some(measure) => {
                    data_total = data_total.saturating_add(measure.bytes);
                    data_complete &= measure.complete;
                }
                None => return unavailable(app_id, result.install_location, DEFERRED),
            }
            seen.push(dir.clone());
            result.data_folders.push(dir);
        }
        if !result.data_folders.is_empty() {
            result.data_bytes = Some(data_total);
            result.data_complete = data_complete;
        }
    }

    result.full_scan = matches!(mode, Mode::Full);
    result
}

#[cfg(windows)]
fn measure_one_windows(ctx: &Context, app_id: &str, mode: Mode) -> AppFootprint {
    let Some(app) = ctx.apps.iter().find(|app| app.id == app_id) else {
        return unavailable(app_id, None, "Programa não está mais no inventário");
    };
    let others = ctx.others(app_id);
    let mut result = blank(app_id);

    // 1. Pasta do programa: declarada ou deduzida.
    let declared = ctx
        .declared
        .iter()
        .find(|(id, _)| id == app_id)
        .map(|(_, dir)| dir.clone());
    let program = match declared {
        Some(dir) => {
            let shared = ctx
                .declared
                .iter()
                .any(|(id, other)| id != app_id && other.eq_ignore_ascii_case(&dir));
            let path = Path::new(&dir);
            let reason = if shared {
                Some("Pasta compartilhada com outro programa".to_string())
            } else if let Some(veto) = folder_deletion_veto(&dir, &ctx.shell_patterns, &others) {
                Some(format!("Pasta não medida: {veto}"))
            } else if !path.is_dir() {
                Some("A pasta de instalação não existe mais".to_string())
            } else if is_reparse_point(path) {
                Some("A pasta de instalação é um link".to_string())
            } else {
                None
            };
            result.install_location = Some(dir.clone());
            match reason {
                Some(reason) => {
                    result.unavailable_reason = Some(reason);
                    None
                }
                None => {
                    result.location_source = Some(LocationSource::Declared);
                    Some(dir)
                }
            }
        }
        None => match deduce_program_dir(app, ctx, &others) {
            Some(dir) => {
                result.install_location = Some(dir.clone());
                result.location_source = Some(LocationSource::Deduced);
                Some(dir)
            }
            None => {
                result.unavailable_reason = Some(
                    "O instalador não informou a pasta e não foi possível deduzi-la".to_string(),
                );
                None
            }
        },
    };

    if let Some(dir) = &program {
        match measure_dir(Path::new(dir), mode) {
            Some(measure) => {
                result.measured_bytes = Some(measure.bytes);
                result.complete = measure.complete;
                result.file_count = measure.files;
            }
            None => return unavailable(app_id, result.install_location, DEFERRED),
        }
    }

    // 2. Pastas de dados com o nome exato (mesma regra da desinstalação).
    let mut named = app.clone();
    if program.is_some() {
        named.install_location = program.clone();
    }
    let program_prefix = program
        .as_ref()
        .map(|dir| format!("{}\\", dir.to_lowercase()));
    let mut seen: Vec<String> = Vec::new();
    let mut data_total = 0u64;
    let mut data_complete = true;
    for candidate in named_folder_candidates(&candidate_names(&named), &ctx.shell_patterns, &others)
    {
        if candidate.is_program {
            continue;
        }
        let dir = normalize(&candidate.path);
        let key = dir.to_lowercase();
        let inside_program = program.as_ref().is_some_and(|p| key == p.to_lowercase())
            || program_prefix
                .as_ref()
                .is_some_and(|prefix| key.starts_with(prefix));
        if inside_program || seen.contains(&key) {
            continue;
        }
        let path = Path::new(&dir);
        if !path.is_dir() || is_reparse_point(path) {
            continue;
        }
        match measure_dir(path, mode) {
            Some(measure) => {
                data_total = data_total.saturating_add(measure.bytes);
                data_complete &= measure.complete;
            }
            None => return unavailable(app_id, result.install_location, DEFERRED),
        }
        seen.push(key);
        result.data_folders.push(dir);
    }
    if !result.data_folders.is_empty() {
        result.data_bytes = Some(data_total);
        result.data_complete = data_complete;
    }
    result.full_scan = matches!(mode, Mode::Full);
    result
}

pub fn measure_apps(app_ids: &[String]) -> Vec<AppFootprint> {
    let ctx = Context::load();
    let mode = Mode::Quick {
        batch_deadline: Instant::now() + BATCH_DEADLINE,
    };
    app_ids
        .iter()
        .take(BATCH_MAX_APPS)
        .map(|app_id| measure_one(&ctx, app_id, mode))
        .collect()
}

/// Mede programa + dados de até `BATCH_MAX_APPS` apps no modo rápido.
#[tauri::command]
pub async fn measure_installed_app_sizes(app_ids: Vec<String>) -> Vec<AppFootprint> {
    tauri::async_runtime::spawn_blocking(move || measure_apps(&app_ids))
        .await
        .unwrap_or_default()
}

/// Libera a trava do modo completo mesmo se a medição entrar em pânico.
struct FullGuard;

impl Drop for FullGuard {
    fn drop(&mut self) {
        FULL_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Medição completa de um único app, pedida pelo usuário na linha "parcial".
/// Uma por vez: duas varreduras pesadas simultâneas disputariam o disco.
#[tauri::command]
pub async fn measure_installed_app_size_full(app_id: String) -> Result<AppFootprint, String> {
    if FULL_RUNNING.swap(true, Ordering::SeqCst) {
        return Err(
            "Já existe uma medição completa em andamento. Aguarde ela terminar.".to_string(),
        );
    }
    let guard = FullGuard;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        measure_one(&Context::load(), &app_id, Mode::Full)
    })
    .await
    .map_err(|_| "A medição completa foi interrompida.".to_string())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soma_arquivos_recursivamente_e_marca_completo() {
        let root = std::env::temp_dir().join(format!("pulse-footprint-{}", std::process::id()));
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("x.bin"), vec![0u8; 100]).unwrap();
        fs::write(nested.join("y.bin"), vec![0u8; 50]).unwrap();

        let (bytes, files, complete) =
            measure_bounded(&root, 1_000, Instant::now() + Duration::from_secs(5));
        fs::remove_dir_all(&root).ok();

        assert_eq!(bytes, 150);
        assert_eq!(files, 2);
        assert!(complete);
    }

    #[test]
    fn remove_aspas_do_install_location() {
        assert_eq!(
            clean_install_location(" \"C:\\Program Files\\AnyDesk\" "),
            "C:\\Program Files\\AnyDesk"
        );
        assert_eq!(clean_install_location("C:\\Apps\\X"), "C:\\Apps\\X");
        assert_eq!(clean_install_location("\"\""), "");
    }

    #[test]
    fn deduz_pasta_do_programa_pelo_executavel_do_icone() {
        assert_eq!(
            program_dir_from_exe("\"C:\\Program Files\\Foto\\foto.exe\",0").as_deref(),
            Some("C:\\Program Files\\Foto")
        );
        // Executável em `bin` → a pasta do programa é a de cima.
        assert_eq!(
            program_dir_from_exe("D:\\Jogos\\Galaxia\\bin\\galaxia.exe,-101").as_deref(),
            Some("D:\\Jogos\\Galaxia")
        );
    }

    #[test]
    fn nao_deduz_pasta_de_desinstalador_cache_ou_sistema() {
        for raw in [
            "C:\\Program Files\\Foto\\unins000.exe",
            "C:\\ProgramData\\Package Cache\\{ABC}\\setup.exe",
            "C:\\Program Files (x86)\\InstallShield Installation Information\\{GUID}\\setup.exe",
            "C:\\Windows\\System32\\msiexec.exe",
            "C:\\Program Files\\Common Files\\Vendor\\tool.exe",
            "C:\\Windows\\Installer\\{GUID}\\icon.ico",
            "C:\\app.exe",
        ] {
            assert_eq!(program_dir_from_exe(raw), None, "{raw}");
        }
    }

    #[test]
    fn nome_do_fabricante_sem_sufixo_societario() {
        assert_eq!(
            vendor_folder_names(Some("Google LLC")),
            vec!["Google LLC", "Google"]
        );
        assert_eq!(
            vendor_folder_names(Some("Blackmagic Design")),
            vec!["Blackmagic Design"]
        );
        assert_eq!(
            vendor_folder_names(Some("Acme, Inc.")),
            vec!["Acme, Inc", "Acme"]
        );
        assert!(vendor_folder_names(Some("Microsoft Corporation")).is_empty());
        assert!(vendor_folder_names(None).is_empty());
    }

    #[test]
    fn orcamento_estourado_vira_limite_inferior() {
        let root = std::env::temp_dir().join(format!("pulse-footprint-cap-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for index in 0..10 {
            fs::write(root.join(format!("{index}.bin")), vec![0u8; 10]).unwrap();
        }

        let (bytes, _, complete) =
            measure_bounded(&root, 3, Instant::now() + Duration::from_secs(5));
        fs::remove_dir_all(&root).ok();

        assert!(!complete);
        assert!(bytes <= 30);
    }

    #[cfg(unix)]
    #[test]
    fn measure_file_list_ignora_diretorios_e_symlinks() {
        let root =
            std::env::temp_dir().join(format!("pulse-footprint-list-{}", std::process::id()));
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let file_a = root.join("a.bin");
        let file_b = sub.join("b.bin");
        let link = root.join("link.bin");
        fs::write(&file_a, vec![0u8; 40]).unwrap();
        fs::write(&file_b, vec![0u8; 25]).unwrap();
        std::os::unix::fs::symlink(&file_a, &link).unwrap();

        let paths = [
            root.to_string_lossy().to_string(),
            sub.to_string_lossy().to_string(),
            file_a.to_string_lossy().to_string(),
            file_b.to_string_lossy().to_string(),
            link.to_string_lossy().to_string(),
            "/caminho/que/nao/existe/pulse".to_string(),
        ];
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let (bytes, files, complete) =
            measure_file_list(refs, 100, Instant::now() + Duration::from_secs(5));
        fs::remove_dir_all(&root).ok();

        assert_eq!(bytes, 65);
        assert_eq!(files, 2);
        assert!(complete);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn medicao_real_de_pacote_deb_e_do_inventario() {
        if super::super::packages_linux::dpkg_list_file("dpkg").is_some() {
            let measured = measure_deb_package(
                "dpkg",
                Mode::Quick {
                    batch_deadline: Instant::now() + Duration::from_secs(5),
                },
            )
            .expect("dentro do prazo")
            .expect("dpkg.list deve ser legível");
            assert!(measured.bytes > 0);
            assert!(measured.files > 0);
        }
        let snapshot = collect_cached();
        if let Some(first) = snapshot.apps.first() {
            let results = measure_apps(std::slice::from_ref(&first.id));
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].app_id, first.id);
            assert!(
                results[0].measured_bytes.is_some() || results[0].unavailable_reason.is_some(),
                "ou mede ou explica o motivo"
            );
        }
    }
}
