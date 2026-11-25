//! Explorador de Arquivos otimizado e não destrutivo do Pulse.
//!
//! - Todo I/O roda em `tauri::async_runtime::spawn_blocking` com `yield_now()`
//!   cooperativo para não travar a interface nem gerar micro-stutter no sistema.
//! - Ações de remoção enviam exclusivamente para a Lixeira do sistema
//!   (`SHFileOperationW` no Windows, `gio trash`/XDG Trash no Linux, Finder no macOS),
//!   nunca realizando exclusão permanente destrutiva.
//! - Métricas indisponíveis (ex.: capacidade de volume sem acesso) retornam
//!   `None` em vez de zero fictício.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "linux", test))]
use std::env;
use std::{
    collections::VecDeque,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_DIR_ENTRIES: usize = 5_000;
const MAX_COUNT_CAP: usize = 50_000;
const DIR_YIELD_INTERVAL: usize = 64;
const MAX_LIST_DURATION: Duration = Duration::from_millis(1500);

const SEARCH_MAX_DEPTH: usize = 4;
const SEARCH_MAX_FILES_TOTAL: usize = 4_000;
const SEARCH_TIMEOUT: Duration = Duration::from_millis(1500);
const SEARCH_YIELD_INTERVAL: usize = 15;
const SEARCH_DEFAULT_LIMIT: usize = 200;
const SEARCH_MAX_LIMIT: usize = 500;
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(30);

struct CachedSearchEntry {
    name: String,
    name_lower: String,
    path: PathBuf,
    is_dir: bool,
    is_symlink: bool,
    is_hidden: bool,
    extension: Option<String>,
}

struct CachedDirIndex {
    root_key: String,
    built_at: Instant,
    entries: Vec<CachedSearchEntry>,
}

static SEARCH_INDEX_CACHE: OnceLock<Mutex<Option<CachedDirIndex>>> = OnceLock::new();

fn search_cache() -> &'static Mutex<Option<CachedDirIndex>> {
    SEARCH_INDEX_CACHE.get_or_init(|| Mutex::new(None))
}

fn invalidate_search_cache() {
    if let Ok(mut guard) = search_cache().lock() {
        *guard = None;
    }
}

const MAX_TEXT_PREVIEW_BYTES: usize = 64 * 1024;
const MAX_IMAGE_PREVIEW_BYTES: u64 = 5 * 1024 * 1024;
const PINS_FILE_NAME: &str = "explorer-pins.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerDrive {
    pub path: String,
    pub label: String,
    pub kind: String,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuickAccessFolder {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PinnedFolder {
    pub name: String,
    pub path: String,
    pub pinned_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuickAccessData {
    pub drives: Vec<ExplorerDrive>,
    pub user_folders: Vec<QuickAccessFolder>,
    pub pins: Vec<PinnedFolder>,
    pub default_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_hidden: bool,
    pub size_bytes: Option<u64>,
    pub modified_unix_ms: Option<i64>,
    pub extension: Option<String>,
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BreadcrumbSegment {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListing {
    pub path: String,
    pub parent_path: Option<String>,
    pub breadcrumbs: Vec<BreadcrumbSegment>,
    pub entries: Vec<ExplorerEntry>,
    pub truncated: bool,
    pub total_seen: usize,
    pub dir_count: usize,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_hidden: bool,
    pub readonly: bool,
    pub size_bytes: Option<u64>,
    pub modified_unix_ms: Option<i64>,
    pub created_unix_ms: Option<i64>,
    pub extension: Option<String>,
    pub kind: String,
    pub text_snippet: Option<String>,
    pub text_truncated: bool,
    pub image_data_url: Option<String>,
    pub note: Option<String>,
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn system_time_to_unix_ms(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
}

fn same_path(a: &str, b: &str) -> bool {
    let norm_a = normalize_for_compare(a);
    let norm_b = normalize_for_compare(b);
    #[cfg(windows)]
    {
        norm_a.eq_ignore_ascii_case(&norm_b)
    }
    #[cfg(not(windows))]
    {
        norm_a == norm_b
    }
}

fn normalize_for_compare(raw: &str) -> String {
    let trimmed = raw.trim();
    #[cfg(windows)]
    {
        let replaced = trimmed.replace('/', "\\");
        // Mantém `C:\` com barra final, mas remove barras extras em subpastas.
        if replaced.len() == 2 && replaced.as_bytes()[1] == b':' {
            return format!("{replaced}\\");
        }
        if replaced.len() > 3 {
            return replaced.trim_end_matches('\\').to_string();
        }
        replaced
    }
    #[cfg(not(windows))]
    {
        if trimmed == "/" {
            "/".to_string()
        } else {
            trimmed.trim_end_matches('/').to_string()
        }
    }
}

/// Normaliza o caminho digitado pelo usuário antes de consultar o sistema de arquivos.
pub fn normalize_input_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    #[cfg(windows)]
    {
        let mut s = trimmed.replace('/', "\\");
        // No Windows, `C:` sem barra significa diretório atual do drive, não a raiz.
        if s.len() == 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic() {
            s.push('\\');
        }
        if s.len() > 3 && !s.starts_with("\\\\") {
            s = s.trim_end_matches('\\').to_string();
        }
        s
    }
    #[cfg(not(windows))]
    {
        if trimmed == "/" {
            "/".to_string()
        } else {
            let clean = trimmed.trim_end_matches('/');
            if clean.is_empty() {
                "/".to_string()
            } else {
                clean.to_string()
            }
        }
    }
}

/// Valida nomes de pastas e arquivos para criação ou renomeação.
/// Rejeita caminhos relativos (`..`, `.`), separadores e caracteres reservados do Windows.
pub fn validate_entry_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("O nome não pode ficar vazio.".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("Nome inválido: '.' e '..' são reservados pelo sistema.".to_string());
    }
    if trimmed.len() > 255 {
        return Err("O nome excede o limite de 255 caracteres.".to_string());
    }
    if trimmed.ends_with('.') {
        return Err("O nome não pode terminar com ponto final.".to_string());
    }
    const FORBIDDEN_CHARS: [char; 9] = ['/', '\\', '<', '>', ':', '"', '|', '?', '*'];
    if let Some(ch) = trimmed
        .chars()
        .find(|c| *c == '\0' || c.is_control() || FORBIDDEN_CHARS.contains(c))
    {
        return Err(format!(
            "Caractere inválido no nome ('{}'). Não use / \\ : * ? \" < > |.",
            ch.escape_default()
        ));
    }
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    let upper = stem.to_ascii_uppercase();
    const RESERVED_NAMES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED_NAMES.contains(&upper.as_str()) {
        return Err(format!(
            "'{trimmed}' é um nome reservado do sistema e não pode ser usado."
        ));
    }
    Ok(trimmed.to_string())
}

/// Constrói a trilha de breadcrumbs a partir de um caminho absoluto.
pub fn build_breadcrumbs(path: &Path) -> Vec<BreadcrumbSegment> {
    let mut segments = Vec::new();
    let mut acc = PathBuf::new();
    let mut seen_prefix = false;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                acc.push(prefix.as_os_str());
                seen_prefix = true;
            }
            Component::RootDir => {
                acc.push(component.as_os_str());
                let full = acc.to_string_lossy().into_owned();
                let label = if seen_prefix {
                    full.trim_end_matches(['\\', '/']).to_string()
                } else {
                    "/".to_string()
                };
                segments.push(BreadcrumbSegment {
                    name: if label.is_empty() {
                        full.clone()
                    } else {
                        label
                    },
                    path: full,
                });
            }
            Component::Normal(part) => {
                acc.push(part);
                segments.push(BreadcrumbSegment {
                    name: part.to_string_lossy().into_owned(),
                    path: acc.to_string_lossy().into_owned(),
                });
            }
            Component::CurDir | Component::ParentDir => {}
        }
    }

    if segments.is_empty() {
        let raw = path.to_string_lossy().into_owned();
        if !raw.is_empty() {
            segments.push(BreadcrumbSegment {
                name: raw.clone(),
                path: raw,
            });
        }
    }

    segments
}

/// Rejeita caminhos que contenham `..` antes de qualquer operação destrutiva ou de
/// renomeação. Sem essa checagem `C:\Users\..` passa pela guarda de raiz (o `parent()`
/// bruto não é vazio) e é resolvido para `C:\` pela API do sistema.
fn reject_parent_components(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Caminho inválido: referências .. não são permitidas.".to_string());
    }
    Ok(())
}

/// Resolve o diretório pai, retornando `None` para raízes (`/` ou `C:\`).
pub fn resolve_parent_path(path: &Path) -> Option<String> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().into_owned())
}

fn is_hidden_entry(name: &str, metadata: Option<&fs::Metadata>) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Some(meta) = metadata {
            if (meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN) != 0 {
                return true;
            }
        }
        name.starts_with('.')
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        name.starts_with('.')
    }
}

fn build_entry(
    path: &Path,
    name: String,
    file_type: Option<fs::FileType>,
    meta: Option<&fs::Metadata>,
) -> ExplorerEntry {
    let is_symlink = file_type.is_some_and(|ft| ft.is_symlink());
    let is_dir = meta
        .map(|m| m.is_dir())
        .unwrap_or_else(|| file_type.is_some_and(|ft| ft.is_dir()));
    let is_hidden = is_hidden_entry(&name, meta);
    let size_bytes = if is_dir { None } else { meta.map(|m| m.len()) };
    let modified_unix_ms = meta
        .and_then(|m| m.modified().ok())
        .and_then(system_time_to_unix_ms);
    let readonly = meta.is_some_and(|m| m.permissions().readonly());
    let extension = if is_dir {
        None
    } else {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
    };

    ExplorerEntry {
        name,
        path: path.to_string_lossy().into_owned(),
        is_dir,
        is_symlink,
        is_hidden,
        size_bytes,
        modified_unix_ms,
        extension,
        readonly,
    }
}

fn default_start_path(drives: &[ExplorerDrive]) -> String {
    if let Some(home) = crate::storage::paths::user_home() {
        if home.is_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    if let Some(first) = drives.first() {
        return first.path.clone();
    }
    #[cfg(windows)]
    {
        "C:\\".to_string()
    }
    #[cfg(not(windows))]
    {
        "/".to_string()
    }
}

// ============================================================================
// Unidades / Pontos de Montagem por plataforma
// ============================================================================

#[cfg(windows)]
fn collect_drives() -> Vec<ExplorerDrive> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };

    let bitmask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();

    if bitmask != 0 {
        for i in 0..26u32 {
            if (bitmask & (1u32 << i)) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let path = format!("{letter}:\\");
            let wide: Vec<u16> = OsStr::new(&path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let drive_type = unsafe { GetDriveTypeW(PCWSTR::from_raw(wide.as_ptr())) };
            // 2 = DRIVE_REMOVABLE, 3 = DRIVE_FIXED, 4 = DRIVE_REMOTE, 6 = DRIVE_RAMDISK
            let kind = match drive_type {
                2 => "removable",
                3 => "fixed",
                4 => "network",
                6 => "ramdisk",
                _ => continue,
            };

            let mut vol_buf = [0u16; 260];
            let vol_ok = unsafe {
                GetVolumeInformationW(
                    PCWSTR::from_raw(wide.as_ptr()),
                    Some(&mut vol_buf),
                    None,
                    None,
                    None,
                    None,
                )
            }
            .is_ok();

            let vol_name = if vol_ok {
                let len = vol_buf
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(vol_buf.len());
                String::from_utf16_lossy(&vol_buf[..len]).trim().to_string()
            } else {
                String::new()
            };

            let label = if !vol_name.is_empty() {
                format!("{vol_name} ({letter}:)")
            } else {
                match kind {
                    "network" => format!("Unidade de Rede ({letter}:)"),
                    "removable" => format!("Unidade Removível ({letter}:)"),
                    _ => format!("Disco Local ({letter}:)"),
                }
            };

            let mut free_avail: u64 = 0;
            let mut total_bytes: u64 = 0;
            let space_ok = unsafe {
                GetDiskFreeSpaceExW(
                    PCWSTR::from_raw(wide.as_ptr()),
                    Some(&mut free_avail),
                    Some(&mut total_bytes),
                    None,
                )
            }
            .is_ok();

            let (total, free) = if space_ok && total_bytes > 0 {
                (Some(total_bytes), Some(free_avail))
            } else {
                (None, None)
            };

            drives.push(ExplorerDrive {
                path,
                label,
                kind: kind.to_string(),
                total_bytes: total,
                free_bytes: free,
            });
        }
    }

    if drives.is_empty() {
        for letter in b'C'..=b'Z' {
            let p = format!("{}:\\", letter as char);
            if Path::new(&p).exists() {
                drives.push(ExplorerDrive {
                    path: p,
                    label: format!("Disco Local ({}:)", letter as char),
                    kind: "fixed".to_string(),
                    total_bytes: None,
                    free_bytes: None,
                });
            }
        }
    }

    drives
}

#[cfg(target_os = "linux")]
fn statvfs_capacity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    let fragment = if stats.f_frsize > 0 {
        stats.f_frsize as u64
    } else {
        stats.f_bsize as u64
    };
    #[allow(clippy::unnecessary_cast)]
    let total = (stats.f_blocks as u64).checked_mul(fragment)?;
    #[allow(clippy::unnecessary_cast)]
    let free = (stats.f_bavail as u64).saturating_mul(fragment);
    (total > 0).then_some((total, free))
}

#[cfg(target_os = "linux")]
fn collect_drives() -> Vec<ExplorerDrive> {
    // Reutiliza os volumes do usuário detectados por `disk_analyzer::mounts` e
    // complementa pontos de montagem em `/mnt/*` (inclusive unidades WSL/DrvFS).
    let mut drives: Vec<ExplorerDrive> = crate::disk_analyzer::mounts::system_drives()
        .into_iter()
        .map(|d| ExplorerDrive {
            kind: if d.path == "/" {
                "system".to_string()
            } else {
                "mount".to_string()
            },
            path: d.path,
            label: d.label,
            total_bytes: d.total_bytes,
            free_bytes: d.free_bytes,
        })
        .collect();

    if let Ok(entries) = fs::read_dir("/mnt") {
        let mut mnt_entries: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        mnt_entries.sort();
        for mnt in mnt_entries {
            let path_str = mnt.to_string_lossy().into_owned();
            if drives.iter().any(|d| d.path == path_str) {
                continue;
            }
            let cap = statvfs_capacity(&mnt);
            let name = mnt
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path_str.clone());
            drives.push(ExplorerDrive {
                label: format!("{name} ({path_str})"),
                path: path_str,
                kind: "mount".to_string(),
                total_bytes: cap.map(|(t, _)| t),
                free_bytes: cap.map(|(_, f)| f),
            });
        }
    }

    if let Some(cap) = Path::new("/home")
        .is_dir()
        .then(|| statvfs_capacity(Path::new("/home")))
        .flatten()
    {
        if !drives.iter().any(|d| d.path == "/home") {
            drives.push(ExplorerDrive {
                path: "/home".to_string(),
                label: "Pastas pessoais (/home)".to_string(),
                kind: "mount".to_string(),
                total_bytes: Some(cap.0),
                free_bytes: Some(cap.1),
            });
        }
    }

    drives
}

#[cfg(target_os = "macos")]
fn collect_drives() -> Vec<ExplorerDrive> {
    let mut drives: Vec<ExplorerDrive> = crate::disk_analyzer::mounts::system_drives()
        .into_iter()
        .map(|d| ExplorerDrive {
            kind: if d.path == "/" {
                "system".to_string()
            } else {
                "mount".to_string()
            },
            path: d.path,
            label: d.label,
            total_bytes: d.total_bytes,
            free_bytes: d.free_bytes,
        })
        .collect();
    if drives.is_empty() {
        drives.push(ExplorerDrive {
            path: "/".to_string(),
            label: "Macintosh HD (/)".to_string(),
            kind: "system".to_string(),
            total_bytes: None,
            free_bytes: None,
        });
    }
    drives
}

#[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
fn collect_drives() -> Vec<ExplorerDrive> {
    vec![ExplorerDrive {
        path: "/".to_string(),
        label: "Sistema (/)".to_string(),
        kind: "system".to_string(),
        total_bytes: None,
        free_bytes: None,
    }]
}

fn collect_user_folders() -> Vec<QuickAccessFolder> {
    let home = crate::storage::paths::user_home();
    let mut out = Vec::new();

    if let Some(ref home_path) = home {
        if home_path.is_dir() {
            out.push(QuickAccessFolder {
                id: "home".to_string(),
                name: "Início".to_string(),
                path: home_path.to_string_lossy().into_owned(),
            });
        }
    }

    type UserFolderSpec = (
        &'static str,
        &'static str,
        fn() -> Option<PathBuf>,
        &'static str,
    );
    let specs: [UserFolderSpec; 6] = [
        ("desktop", "Área de Trabalho", dirs::desktop_dir, "Desktop"),
        ("downloads", "Downloads", dirs::download_dir, "Downloads"),
        ("documents", "Documentos", dirs::document_dir, "Documents"),
        ("pictures", "Imagens", dirs::picture_dir, "Pictures"),
        ("videos", "Vídeos", dirs::video_dir, "Videos"),
        ("music", "Músicas", dirs::audio_dir, "Music"),
    ];

    for (id, label, known_fn, fallback) in specs {
        let candidate = known_fn().or_else(|| home.as_ref().map(|h| h.join(fallback)));
        let Some(dir) = candidate else { continue };
        if !dir.is_dir() {
            continue;
        }
        // Em distros XDG, pastas desativadas podem apontar para a própria pasta pessoal.
        if home.as_ref().is_some_and(|h| h == &dir) {
            continue;
        }
        let path_str = dir.to_string_lossy().into_owned();
        if out
            .iter()
            .any(|existing| same_path(&existing.path, &path_str))
        {
            continue;
        }
        out.push(QuickAccessFolder {
            id: id.to_string(),
            name: label.to_string(),
            path: path_str,
        });
    }

    out
}

// ============================================================================
// Persistência de pastas fixadas (explorer-pins.json)
// ============================================================================

fn pins_file_path() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join(PINS_FILE_NAME)
}

pub fn load_pins_from(file: &Path) -> Vec<PinnedFolder> {
    let Ok(content) = fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(mut pins) = serde_json::from_str::<Vec<PinnedFolder>>(&content) else {
        return Vec::new();
    };
    pins.retain(|p| !p.path.trim().is_empty());
    pins
}

pub fn save_pins_to(file: &Path, pins: &[PinnedFolder]) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Não foi possível criar diretório de configuração: {e}"))?;
    }
    let json = serde_json::to_string_pretty(pins)
        .map_err(|e| format!("Não foi possível serializar favoritos do explorador: {e}"))?;
    let temp = file.with_extension("json.tmp");
    fs::write(&temp, json)
        .map_err(|e| format!("Não foi possível gravar favoritos do explorador: {e}"))?;
    fs::rename(&temp, file)
        .map_err(|e| format!("Não foi possível finalizar favoritos do explorador: {e}"))?;
    Ok(())
}

pub fn toggle_pin_in(file: &Path, raw_path: &str) -> Result<Vec<PinnedFolder>, String> {
    let clean = normalize_input_path(raw_path);
    if clean.is_empty() {
        return Err("O caminho da pasta não pode ficar vazio.".to_string());
    }

    let mut pins = load_pins_from(file);
    if let Some(pos) = pins.iter().position(|p| same_path(&p.path, &clean)) {
        pins.remove(pos);
        save_pins_to(file, &pins)?;
        return Ok(pins);
    }

    let target = Path::new(&clean);
    if !target.is_dir() {
        return Err("Somente pastas existentes podem ser fixadas no acesso rápido.".to_string());
    }

    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| clean.clone());

    pins.push(PinnedFolder {
        name,
        path: clean,
        pinned_at_unix_ms: now_unix_ms(),
    });
    save_pins_to(file, &pins)?;
    Ok(pins)
}

// ============================================================================
// Listagem, Busca e Pré-visualização (I/O síncrono para spawn_blocking)
// ============================================================================

pub fn list_directory_sync(raw_path: &str, show_hidden: bool) -> Result<DirectoryListing, String> {
    let started = Instant::now();
    let normalized = normalize_input_path(raw_path);
    let target = if normalized.is_empty() {
        let drives = collect_drives();
        PathBuf::from(default_start_path(&drives))
    } else {
        PathBuf::from(&normalized)
    };

    let dir_meta = fs::metadata(&target).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("Pasta não encontrada: {}", target.display()),
        std::io::ErrorKind::PermissionDenied => {
            format!("Permissão negada ao acessar: {}", target.display())
        }
        _ => format!("Não foi possível acessar '{}': {e}", target.display()),
    })?;
    if !dir_meta.is_dir() {
        return Err(format!("O caminho '{}' não é uma pasta.", target.display()));
    }

    let read_dir = fs::read_dir(&target).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            format!("Permissão negada ao listar '{}'.", target.display())
        }
        _ => format!("Não foi possível ler a pasta '{}': {e}", target.display()),
    })?;

    let mut entries = Vec::new();
    let mut seen_raw: usize = 0;
    let mut total_seen: usize = 0;
    let mut truncated = false;

    for item in read_dir {
        seen_raw += 1;
        if seen_raw.is_multiple_of(DIR_YIELD_INTERVAL) {
            std::thread::yield_now();
        }

        let Ok(dir_entry) = item else {
            continue;
        };

        if entries.len() >= MAX_DIR_ENTRIES {
            truncated = true;
            total_seen += 1;
            if total_seen >= MAX_COUNT_CAP || started.elapsed() >= MAX_LIST_DURATION {
                break;
            }
            continue;
        }

        let name = dir_entry.file_name().to_string_lossy().into_owned();
        let entry_path = dir_entry.path();
        let file_type = dir_entry.file_type().ok();
        let meta = dir_entry
            .metadata()
            .ok()
            .or_else(|| fs::symlink_metadata(&entry_path).ok());

        let entry = build_entry(&entry_path, name, file_type, meta.as_ref());
        if !show_hidden && entry.is_hidden {
            continue;
        }

        total_seen += 1;
        entries.push(entry);
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });

    let dir_count = entries.iter().filter(|e| e.is_dir).count();
    let file_count = entries.len().saturating_sub(dir_count);

    Ok(DirectoryListing {
        path: target.to_string_lossy().into_owned(),
        parent_path: resolve_parent_path(&target),
        breadcrumbs: build_breadcrumbs(&target),
        entries,
        truncated,
        total_seen,
        dir_count,
        file_count,
    })
}

pub fn search_in_dir_sync(
    raw_root: &str,
    query: &str,
    max_results: Option<usize>,
) -> Result<Vec<ExplorerEntry>, String> {
    let raw_query = query.trim().to_lowercase();
    if raw_query.is_empty() {
        return Ok(Vec::new());
    }

    let normalized = normalize_input_path(raw_root);
    let root = PathBuf::from(&normalized);
    let meta = fs::metadata(&root)
        .map_err(|e| format!("Não foi possível acessar '{}': {e}", root.display()))?;
    if !meta.is_dir() {
        return Err(format!("O caminho '{}' não é uma pasta.", root.display()));
    }

    let limit = max_results
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);

    // Separa filtros rápidos (`ext:pdf`) de termos livres multi-palavra.
    let mut ext_filter: Option<String> = None;
    let mut terms: Vec<&str> = Vec::new();
    for token in raw_query.split_whitespace() {
        if let Some(ext) = token.strip_prefix("ext:") {
            let clean = ext.trim_start_matches('.').trim();
            if !clean.is_empty() {
                ext_filter = Some(clean.to_owned());
            }
        } else {
            terms.push(token);
        }
    }

    let root_key = normalize_for_compare(&normalized).to_lowercase();
    let use_cached = search_cache()
        .lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().and_then(|idx| {
                (idx.root_key == root_key && idx.built_at.elapsed() < SEARCH_CACHE_TTL)
                    .then_some(true)
            })
        })
        .unwrap_or(false);

    if !use_cached {
        // Constrói índice rápido da subárvore respeitando limites de I/O e yield_now.
        let deadline = Instant::now() + SEARCH_TIMEOUT;
        let mut files_checked: usize = 0;
        let mut indexed = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back((root.clone(), 0usize));

        while let Some((dir_path, depth)) = queue.pop_front() {
            if depth >= SEARCH_MAX_DEPTH
                || files_checked >= SEARCH_MAX_FILES_TOTAL
                || Instant::now() >= deadline
            {
                break;
            }
            let Ok(read_dir) = fs::read_dir(&dir_path) else {
                continue;
            };
            for item in read_dir {
                if files_checked >= SEARCH_MAX_FILES_TOTAL || Instant::now() >= deadline {
                    break;
                }
                files_checked += 1;
                if files_checked.is_multiple_of(SEARCH_YIELD_INTERVAL) {
                    std::thread::yield_now();
                }
                let Ok(entry) = item else { continue };
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let name_lower = name.to_lowercase();
                let entry_path = entry.path();
                let is_dir = file_type.is_dir();
                let extension = if is_dir {
                    None
                } else {
                    entry_path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase())
                        .filter(|e| !e.is_empty())
                };
                indexed.push(CachedSearchEntry {
                    is_hidden: name.starts_with('.'),
                    name,
                    name_lower: name_lower.clone(),
                    path: entry_path.clone(),
                    is_dir,
                    is_symlink: false,
                    extension,
                });
                if is_dir
                    && depth + 1 < SEARCH_MAX_DEPTH
                    && !matches!(
                        name_lower.as_str(),
                        ".git" | "node_modules" | "$recycle.bin" | "system volume information"
                    )
                {
                    queue.push_back((entry_path, depth + 1));
                }
            }
        }

        if let Ok(mut guard) = search_cache().lock() {
            *guard = Some(CachedDirIndex {
                root_key: root_key.clone(),
                built_at: Instant::now(),
                entries: indexed,
            });
        }
    }

    let mut scored: Vec<(u8, ExplorerEntry)> = Vec::new();
    if let Ok(guard) = search_cache().lock() {
        if let Some(ref idx) = *guard {
            for item in &idx.entries {
                if let Some(ref wanted_ext) = ext_filter {
                    if item.extension.as_deref() != Some(wanted_ext.as_str()) {
                        continue;
                    }
                }
                if !terms.iter().all(|t| item.name_lower.contains(t)) {
                    continue;
                }
                let rank = if terms.len() == 1 && item.name_lower == terms[0] {
                    0u8
                } else if terms
                    .first()
                    .is_some_and(|t| item.name_lower.starts_with(t))
                {
                    1u8
                } else {
                    2u8
                };
                let entry_meta = fs::metadata(&item.path).ok();
                let size_bytes = if item.is_dir {
                    None
                } else {
                    entry_meta.as_ref().map(|m| m.len())
                };
                let modified_unix_ms = entry_meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(system_time_to_unix_ms);
                let readonly = entry_meta
                    .as_ref()
                    .is_some_and(|m| m.permissions().readonly());

                scored.push((
                    rank,
                    ExplorerEntry {
                        name: item.name.clone(),
                        path: item.path.to_string_lossy().into_owned(),
                        is_dir: item.is_dir,
                        is_symlink: item.is_symlink,
                        is_hidden: item.is_hidden,
                        size_bytes,
                        modified_unix_ms,
                        extension: item.extension.clone(),
                        readonly,
                    },
                ));
                if scored.len() >= limit * 2 {
                    break;
                }
            }
        }
    }

    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.is_dir.cmp(&a.1.is_dir))
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
            .then_with(|| a.1.path.cmp(&b.1.path))
    });
    scored.truncate(limit);

    Ok(scored.into_iter().map(|(_, entry)| entry).collect())
}

fn image_mime_for_ext(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

fn looks_like_binary(sample: &[u8]) -> bool {
    let check_len = sample.len().min(4096);
    if check_len == 0 {
        return false;
    }
    let slice = &sample[..check_len];
    if slice.contains(&0) {
        return true;
    }
    let suspicious = slice
        .iter()
        .filter(|&&b| b < 0x09 || (b > 0x0D && b < 0x20 && b != 0x1B))
        .count();
    suspicious * 20 > check_len
}

fn utf8_prefix_lossy(bytes: &[u8]) -> String {
    // Ajusta até 3 bytes no final caso o corte de 64 KB tenha dividido um caractere multibyte.
    let mut end = bytes.len();
    for back in 0..=3 {
        let candidate = bytes.len().saturating_sub(back);
        if std::str::from_utf8(&bytes[..candidate]).is_ok() {
            end = candidate;
            break;
        }
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub fn preview_file_sync(raw_path: &str) -> Result<FilePreview, String> {
    let normalized = normalize_input_path(raw_path);
    let target = PathBuf::from(&normalized);

    let sym_meta = fs::symlink_metadata(&target)
        .map_err(|e| format!("Item não encontrado '{}': {e}", target.display()))?;
    let is_symlink = sym_meta.file_type().is_symlink();
    let meta = fs::metadata(&target).unwrap_or(sym_meta);

    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| target.to_string_lossy().into_owned());
    let is_dir = meta.is_dir();
    let is_hidden = is_hidden_entry(&name, Some(&meta));
    let readonly = meta.permissions().readonly();
    let size_bytes = if is_dir { None } else { Some(meta.len()) };
    let modified_unix_ms = meta.modified().ok().and_then(system_time_to_unix_ms);
    let created_unix_ms = meta.created().ok().and_then(system_time_to_unix_ms);
    let extension = if is_dir {
        None
    } else {
        target
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .filter(|e| !e.is_empty())
    };

    if !is_dir && !meta.is_file() {
        // FIFO, socket ou dispositivo: abrir bloquearia a thread do pool para sempre.
        return Ok(FilePreview {
            path: target.to_string_lossy().into_owned(),
            name,
            is_dir: false,
            is_symlink,
            is_hidden,
            readonly,
            size_bytes,
            modified_unix_ms,
            created_unix_ms,
            extension: None,
            kind: "special".to_string(),
            text_snippet: None,
            text_truncated: false,
            image_data_url: None,
            note: Some(
                "Arquivo especial do sistema (FIFO, socket ou dispositivo): sem pré-visualização."
                    .to_string(),
            ),
        });
    }

    if is_dir {
        return Ok(FilePreview {
            path: target.to_string_lossy().into_owned(),
            name,
            is_dir: true,
            is_symlink,
            is_hidden,
            readonly,
            size_bytes: None,
            modified_unix_ms,
            created_unix_ms,
            extension: None,
            kind: "directory".to_string(),
            text_snippet: None,
            text_truncated: false,
            image_data_url: None,
            note: Some("Pasta do sistema de arquivos.".to_string()),
        });
    }

    if let Some(mime) = extension.as_deref().and_then(image_mime_for_ext) {
        let len = meta.len();
        if len > 0 && len <= MAX_IMAGE_PREVIEW_BYTES {
            if let Ok(bytes) = fs::read(&target) {
                let encoded = BASE64.encode(&bytes);
                return Ok(FilePreview {
                    path: target.to_string_lossy().into_owned(),
                    name,
                    is_dir: false,
                    is_symlink,
                    is_hidden,
                    readonly,
                    size_bytes: Some(len),
                    modified_unix_ms,
                    created_unix_ms,
                    extension,
                    kind: "image".to_string(),
                    text_snippet: None,
                    text_truncated: false,
                    image_data_url: Some(format!("data:{mime};base64,{encoded}")),
                    note: None,
                });
            }
        }
        return Ok(FilePreview {
            path: target.to_string_lossy().into_owned(),
            name,
            is_dir: false,
            is_symlink,
            is_hidden,
            readonly,
            size_bytes: Some(len),
            modified_unix_ms,
            created_unix_ms,
            extension,
            kind: "binary".to_string(),
            text_snippet: None,
            text_truncated: false,
            image_data_url: None,
            note: Some(if len == 0 {
                "Arquivo de imagem vazio.".to_string()
            } else {
                "Imagem excede o limite de 5 MB para pré-visualização leve.".to_string()
            }),
        });
    }

    let file = fs::File::open(&target).map_err(|e| {
        format!(
            "Não foi possível abrir '{}' para leitura: {e}",
            target.display()
        )
    })?;
    let mut buf = Vec::new();
    file.take((MAX_TEXT_PREVIEW_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("Falha ao ler trecho do arquivo: {e}"))?;

    if looks_like_binary(&buf) {
        return Ok(FilePreview {
            path: target.to_string_lossy().into_owned(),
            name,
            is_dir: false,
            is_symlink,
            is_hidden,
            readonly,
            size_bytes,
            modified_unix_ms,
            created_unix_ms,
            extension,
            kind: "binary".to_string(),
            text_snippet: None,
            text_truncated: false,
            image_data_url: None,
            note: Some("Arquivo binário sem pré-visualização textual.".to_string()),
        });
    }

    let text_truncated =
        buf.len() > MAX_TEXT_PREVIEW_BYTES || meta.len() > MAX_TEXT_PREVIEW_BYTES as u64;
    let slice = &buf[..buf.len().min(MAX_TEXT_PREVIEW_BYTES)];
    let snippet = utf8_prefix_lossy(slice);

    Ok(FilePreview {
        path: target.to_string_lossy().into_owned(),
        name,
        is_dir: false,
        is_symlink,
        is_hidden,
        readonly,
        size_bytes,
        modified_unix_ms,
        created_unix_ms,
        extension,
        kind: "text".to_string(),
        text_snippet: Some(snippet),
        text_truncated,
        image_data_url: None,
        note: None,
    })
}

// ============================================================================
// Ações seguras: Abrir, Terminal, Criar Pasta, Renomear, Mover para Lixeira
// ============================================================================

pub fn open_entry_sync(raw_path: &str) -> Result<(), String> {
    let normalized = normalize_input_path(raw_path);
    let target = PathBuf::from(&normalized);
    if !target.exists() {
        return Err("O item não existe ou não pôde ser acessado.".to_string());
    }

    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let wide_path: Vec<u16> = OsStr::new(&target)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let ret = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR::from_raw(wide_path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if (ret.0 as usize) <= 32 {
            return Err(format!(
                "O Windows não conseguiu abrir '{}' (código {}).",
                target.display(),
                ret.0 as usize
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    {
        crate::disk_analyzer::desktop::open_with_default_app(&target)
    }
}

pub fn open_in_terminal_sync(raw_path: &str) -> Result<(), String> {
    let normalized = normalize_input_path(raw_path);
    let candidate = PathBuf::from(&normalized);
    if !candidate.exists() {
        return Err("O caminho informado não existe.".to_string());
    }
    let dir = if candidate.is_dir() {
        candidate
    } else {
        candidate
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Não foi possível determinar a pasta do item.".to_string())?
    };
    if !dir.is_dir() {
        return Err("O destino não é uma pasta válida.".to_string());
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        // Tenta Windows Terminal (`wt.exe -d <dir>`) e cai para `powershell.exe` limpo.
        // O caminho vai por `current_dir` + `-d .` porque `wt.exe` interpreta `;`
        // como delimitador de subcomandos mesmo dentro de aspas.
        if Command::new("wt.exe")
            .current_dir(&dir)
            .arg("-d")
            .arg(".")
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        Command::new("powershell.exe")
            .current_dir(&dir)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Não foi possível abrir o terminal no Windows: {e}"))
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};

        let desktop = env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_ascii_uppercase();
        let candidates: &[(&str, &[&str])] = if desktop.contains("KDE") {
            &[
                ("konsole", &["--workdir"]),
                ("gnome-terminal", &[]),
                ("xfce4-terminal", &[]),
                ("kitty", &[]),
                ("alacritty", &[]),
                ("wezterm", &["start", "--cwd"]),
                ("x-terminal-emulator", &[]),
                ("xterm", &[]),
            ]
        } else if desktop.contains("XFCE") {
            &[
                ("xfce4-terminal", &[]),
                ("gnome-terminal", &[]),
                ("konsole", &["--workdir"]),
                ("kitty", &[]),
                ("alacritty", &[]),
                ("wezterm", &["start", "--cwd"]),
                ("x-terminal-emulator", &[]),
                ("xterm", &[]),
            ]
        } else {
            &[
                ("gnome-terminal", &[]),
                ("konsole", &["--workdir"]),
                ("xfce4-terminal", &[]),
                ("kitty", &[]),
                ("alacritty", &[]),
                ("wezterm", &["start", "--cwd"]),
                ("x-terminal-emulator", &[]),
                ("xterm", &[]),
            ]
        };

        for (bin, args) in candidates {
            let mut cmd = Command::new(bin);
            if !args.is_empty() {
                cmd.args(*args).arg(&dir);
            }
            cmd.current_dir(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Ok(mut child) = cmd.spawn() {
                let _ = std::thread::Builder::new()
                    .name("pulse-explorer-term".into())
                    .spawn(move || {
                        let _ = child.wait();
                    });
                return Ok(());
            }
        }
        Err("Nenhum emulador de terminal suportado foi encontrado no PATH.".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};

        let mut child = Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Não foi possível abrir o Terminal no macOS: {e}"))?;
        let _ = std::thread::Builder::new()
            .name("pulse-explorer-term".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(())
    }

    #[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
    {
        let _ = dir;
        Err("Abertura de terminal não suportada nesta plataforma.".to_string())
    }
}

pub fn create_folder_sync(parent_path: &str, name: &str) -> Result<ExplorerEntry, String> {
    let valid_name = validate_entry_name(name)?;
    let normalized_parent = normalize_input_path(parent_path);
    let parent = PathBuf::from(&normalized_parent);
    if !parent.is_dir() {
        return Err(format!(
            "A pasta de destino '{}' não existe.",
            parent.display()
        ));
    }

    let target = parent.join(&valid_name);
    if fs::symlink_metadata(&target).is_ok() {
        return Err(format!(
            "Já existe um item chamado '{valid_name}' nesta pasta."
        ));
    }

    fs::create_dir(&target).map_err(|e| format!("Não foi possível criar a pasta: {e}"))?;
    invalidate_search_cache();
    let meta = fs::metadata(&target).ok();
    Ok(build_entry(&target, valid_name, None, meta.as_ref()))
}

pub fn rename_entry_sync(raw_path: &str, new_name: &str) -> Result<ExplorerEntry, String> {
    let valid_name = validate_entry_name(new_name)?;
    let normalized = normalize_input_path(raw_path);
    let source = PathBuf::from(&normalized);
    reject_parent_components(&source)?;

    if fs::symlink_metadata(&source).is_err() {
        return Err("O item selecionado não existe mais.".to_string());
    }
    let parent = source
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "Não é permitido renomear uma unidade raiz.".to_string())?;

    let target = parent.join(&valid_name);
    let current_name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    if current_name == valid_name {
        let meta = fs::symlink_metadata(&source).ok();
        return Ok(build_entry(&source, valid_name, None, meta.as_ref()));
    }

    let case_only_change = {
        #[cfg(windows)]
        {
            current_name.eq_ignore_ascii_case(&valid_name)
        }
        #[cfg(not(windows))]
        {
            false
        }
    };

    // Garante que nunca sobrescreve um item existente no destino.
    if !case_only_change && fs::symlink_metadata(&target).is_ok() {
        return Err(format!(
            "Já existe outro item chamado '{valid_name}' nesta pasta."
        ));
    }

    fs::rename(&source, &target).map_err(|e| format!("Não foi possível renomear: {e}"))?;
    invalidate_search_cache();
    let meta = fs::symlink_metadata(&target).ok();
    Ok(build_entry(&target, valid_name, None, meta.as_ref()))
}

#[cfg(windows)]
fn strip_verbatim_prefix(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.into_owned()
    }
}

/// Move o item para a Lixeira do sistema. Nunca remove permanentemente.
pub fn trash_entry_sync(raw_path: &str) -> Result<(), String> {
    invalidate_search_cache();
    let normalized = normalize_input_path(raw_path);
    let target = PathBuf::from(&normalized);
    reject_parent_components(&target)?;

    if fs::symlink_metadata(&target).is_err() {
        return Err("O item não foi encontrado.".to_string());
    }
    if target.parent().is_none_or(|p| p.as_os_str().is_empty()) {
        return Err("Não é permitido mover uma unidade raiz para a Lixeira.".to_string());
    }

    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
            FOF_WANTNUKEWARNING, FO_DELETE, SHFILEOPSTRUCTW,
        };

        // `FOF_ALLOWUNDO` sozinho não garante Lixeira: em unidade de rede, volume sem
        // Lixeira ativa ou arquivo maior que o limite, o Windows apaga de forma
        // definitiva. `FOF_WANTNUKEWARNING` transforma esse caso em aviso explícito
        // (o usuário pode cancelar) e caminhos UNC são recusados de saída.
        if let Some(Component::Prefix(prefix)) = target.components().next() {
            if matches!(
                prefix.kind(),
                std::path::Prefix::UNC(_, _) | std::path::Prefix::VerbatimUNC(_, _)
            ) {
                return Err(
                    "Unidades de rede não têm Lixeira: o Pulse não apaga esse item permanentemente."
                        .to_string(),
                );
            }
        }

        let abs = std::path::absolute(&target).unwrap_or_else(|_| target.clone());
        let clean = strip_verbatim_prefix(&abs);
        // `pFrom` em `SHFileOperationW` exige string UTF-16 terminada por dois NUL (`\0\0`).
        let mut wide: Vec<u16> = OsStr::new(&clean).encode_wide().collect();
        wide.push(0);
        wide.push(0);

        let mut op = SHFILEOPSTRUCTW {
            hwnd: std::ptr::null_mut(),
            wFunc: FO_DELETE,
            pFrom: wide.as_ptr(),
            pTo: std::ptr::null(),
            fFlags: (FOF_ALLOWUNDO
                | FOF_NOCONFIRMATION
                | FOF_SILENT
                | FOF_NOERRORUI
                | FOF_WANTNUKEWARNING) as u16,
            fAnyOperationsAborted: 0,
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: std::ptr::null(),
        };

        let status = unsafe { SHFileOperationW(&mut op) };
        if status != 0 {
            return Err(format!(
                "Não foi possível mover para a Lixeira do Windows (código 0x{status:X})."
            ));
        }
        if op.fAnyOperationsAborted != 0 {
            return Err("A operação de envio para a Lixeira foi cancelada.".to_string());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;

        let abs = if target.is_absolute() {
            target.clone()
        } else {
            env::current_dir()
                .map(|cwd| cwd.join(&target))
                .unwrap_or_else(|_| target.clone())
        };

        if let Ok(output) = Command::new("gio").arg("trash").arg(&abs).output() {
            if output.status.success() {
                return Ok(());
            }
        }

        // Fallback XDG Trash nativo (reversível, sem remoção destrutiva).
        let home = crate::storage::paths::user_home()
            .ok_or_else(|| "Pasta pessoal não encontrada para localizar a Lixeira.".to_string())?;
        let trash_root = env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"))
            .join("Trash");
        crate::uninstall::uninstall_unix::move_to_trash_in(&abs, &trash_root).map(|_| ())
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        let abs = if target.is_absolute() {
            target.clone()
        } else {
            env::current_dir()
                .map(|cwd| cwd.join(&target))
                .unwrap_or_else(|_| target.clone())
        };
        let path_str = abs.to_string_lossy().into_owned();
        if !Path::new("/usr/bin/osascript").exists() {
            return Err("osascript não está disponível para mover para o Lixo.".to_string());
        }
        let output = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "set alvo to POSIX file (item 1 of argv)",
                "-e",
                "tell application \"Finder\" to delete alvo",
                "-e",
                "end run",
                &path_str,
            ])
            .output()
            .map_err(|e| format!("Falha ao acionar o Finder para mover ao Lixo: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if err.is_empty() {
                "O Finder não conseguiu mover o item para o Lixo.".to_string()
            } else {
                format!("O Finder recusou mover para o Lixo: {err}")
            });
        }
        Ok(())
    }

    #[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
    {
        let _ = target;
        Err("Lixeira do sistema não suportada nesta plataforma.".to_string())
    }
}

// ============================================================================
// Comandos Tauri assíncronos (spawn_blocking + auditoria em crate::debug)
// ============================================================================

#[tauri::command]
pub async fn explorer_get_quick_access() -> Result<QuickAccessData, String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let drives = collect_drives();
        let user_folders = collect_user_folders();
        let pins = load_pins_from(&pins_file_path());
        let default_path = default_start_path(&drives);
        QuickAccessData {
            drives,
            user_folders,
            pins,
            default_path,
        }
    })
    .await
    .map_err(|e| e.to_string());
    crate::debug::record_result(
        "file_explorer_quick_access",
        started.elapsed().as_millis(),
        &result,
        "Falha ao carregar acesso rápido do explorador",
    );
    result
}

#[tauri::command]
pub async fn explorer_list_directory(
    path: String,
    show_hidden: bool,
) -> Result<DirectoryListing, String> {
    let started = Instant::now();
    let result =
        tauri::async_runtime::spawn_blocking(move || list_directory_sync(&path, show_hidden))
            .await
            .map_err(|e| e.to_string())
            .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_list_dir",
        started.elapsed().as_millis(),
        &result,
        "Falha ao listar diretório",
    );
    result
}

#[tauri::command]
pub async fn explorer_search_in_dir(
    root: String,
    query: String,
    max_results: Option<usize>,
) -> Result<Vec<ExplorerEntry>, String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || {
        search_in_dir_sync(&root, &query, max_results)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_search",
        started.elapsed().as_millis(),
        &result,
        "Falha ao pesquisar na pasta",
    );
    result
}

#[tauri::command]
pub async fn explorer_preview_file(path: String) -> Result<FilePreview, String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || preview_file_sync(&path))
        .await
        .map_err(|e| e.to_string())
        .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_preview",
        started.elapsed().as_millis(),
        &result,
        "Falha ao gerar pré-visualização",
    );
    result
}

#[tauri::command]
pub async fn explorer_open_entry(path: String) -> Result<(), String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || open_entry_sync(&path))
        .await
        .map_err(|e| e.to_string())
        .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_open",
        started.elapsed().as_millis(),
        &result,
        "Falha ao abrir item do explorador",
    );
    result
}

#[tauri::command]
pub async fn explorer_open_in_terminal(path: String) -> Result<(), String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || open_in_terminal_sync(&path))
        .await
        .map_err(|e| e.to_string())
        .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_terminal",
        started.elapsed().as_millis(),
        &result,
        "Falha ao abrir terminal na pasta",
    );
    result
}

#[tauri::command]
pub async fn explorer_create_folder(
    parent_path: String,
    name: String,
) -> Result<ExplorerEntry, String> {
    let started = Instant::now();
    let result =
        tauri::async_runtime::spawn_blocking(move || create_folder_sync(&parent_path, &name))
            .await
            .map_err(|e| e.to_string())
            .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_create_folder",
        started.elapsed().as_millis(),
        &result,
        "Falha ao criar pasta",
    );
    result
}

#[tauri::command]
pub async fn explorer_rename_entry(
    path: String,
    new_name: String,
) -> Result<ExplorerEntry, String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || rename_entry_sync(&path, &new_name))
        .await
        .map_err(|e| e.to_string())
        .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_rename",
        started.elapsed().as_millis(),
        &result,
        "Falha ao renomear item",
    );
    result
}

#[tauri::command]
pub async fn explorer_trash_entry(path: String) -> Result<(), String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || trash_entry_sync(&path))
        .await
        .map_err(|e| e.to_string())
        .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_trash",
        started.elapsed().as_millis(),
        &result,
        "Falha ao mover item para a Lixeira",
    );
    result
}

#[tauri::command]
pub async fn explorer_toggle_pin(path: String) -> Result<Vec<PinnedFolder>, String> {
    let started = Instant::now();
    let result =
        tauri::async_runtime::spawn_blocking(move || toggle_pin_in(&pins_file_path(), &path))
            .await
            .map_err(|e| e.to_string())
            .and_then(|inner| inner);
    crate::debug::record_result(
        "file_explorer_toggle_pin",
        started.elapsed().as_millis(),
        &result,
        "Falha ao atualizar pastas fixadas",
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_workspace(label: &str) -> PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!(
            "pulse_file_explorer_{label}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("criar diretório temporário de teste");
        dir
    }

    #[test]
    fn validate_entry_name_rejects_invalid_and_reserved_names() {
        assert_eq!(validate_entry_name("  Projetos  ").unwrap(), "Projetos");
        assert!(validate_entry_name("").is_err());
        assert!(validate_entry_name("   ").is_err());
        assert!(validate_entry_name(".").is_err());
        assert!(validate_entry_name("..").is_err());
        assert!(validate_entry_name("a/b").is_err());
        assert!(validate_entry_name("a\\b").is_err());
        assert!(validate_entry_name("foo:bar").is_err());
        assert!(validate_entry_name("foo*bar").is_err());
        assert!(validate_entry_name("foo?").is_err());
        assert!(validate_entry_name("pasta.").is_err());
        assert!(validate_entry_name("CON").is_err());
        assert!(validate_entry_name("nul.txt").is_err());
        assert!(validate_entry_name("COM1").is_err());
        assert!(validate_entry_name("arquivo-valido_2026.md").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn breadcrumbs_and_parent_work_on_unix_paths() {
        let path = Path::new("/home/dev/Documentos");
        let crumbs = build_breadcrumbs(path);
        assert_eq!(
            crumbs,
            vec![
                BreadcrumbSegment {
                    name: "/".to_string(),
                    path: "/".to_string()
                },
                BreadcrumbSegment {
                    name: "home".to_string(),
                    path: "/home".to_string()
                },
                BreadcrumbSegment {
                    name: "dev".to_string(),
                    path: "/home/dev".to_string()
                },
                BreadcrumbSegment {
                    name: "Documentos".to_string(),
                    path: "/home/dev/Documentos".to_string()
                },
            ]
        );
        assert_eq!(resolve_parent_path(path).as_deref(), Some("/home/dev"));
        assert_eq!(resolve_parent_path(Path::new("/")), None);
    }

    #[cfg(windows)]
    #[test]
    fn breadcrumbs_and_parent_work_on_windows_paths() {
        let path = Path::new(r"C:\Users\dev\Documentos");
        let crumbs = build_breadcrumbs(path);
        assert_eq!(
            crumbs,
            vec![
                BreadcrumbSegment {
                    name: "C:".to_string(),
                    path: r"C:\".to_string()
                },
                BreadcrumbSegment {
                    name: "Users".to_string(),
                    path: r"C:\Users".to_string()
                },
                BreadcrumbSegment {
                    name: "dev".to_string(),
                    path: r"C:\Users\dev".to_string()
                },
                BreadcrumbSegment {
                    name: "Documentos".to_string(),
                    path: r"C:\Users\dev\Documentos".to_string()
                },
            ]
        );
        assert_eq!(
            resolve_parent_path(path).as_deref(),
            Some(r"C:\Users\dev")
        );
        assert_eq!(resolve_parent_path(Path::new(r"C:\")), None);
        assert_eq!(normalize_input_path("D:"), r"D:\");
    }

    #[test]
    fn list_directory_sorts_dirs_first_and_respects_show_hidden() {
        let root = temp_workspace("list");
        fs::create_dir_all(root.join("Zebra")).unwrap();
        fs::create_dir_all(root.join("alpha")).unwrap();
        fs::create_dir_all(root.join(".oculto")).unwrap();
        fs::write(root.join("nota.txt"), b"conteudo").unwrap();
        fs::write(root.join(".env"), b"SECRET=1").unwrap();

        let visible_only = list_directory_sync(&root.to_string_lossy(), false).unwrap();
        let names: Vec<&str> = visible_only
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "Zebra", "nota.txt"]);
        assert_eq!(visible_only.dir_count, 2);
        assert_eq!(visible_only.file_count, 1);
        assert!(!visible_only.truncated);

        let with_hidden = list_directory_sync(&root.to_string_lossy(), true).unwrap();
        assert!(with_hidden.entries.iter().any(|e| e.name == ".oculto"));
        assert!(with_hidden.entries.iter().any(|e| e.name == ".env"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_in_dir_finds_nested_entries_within_depth_guardrail() {
        let root = temp_workspace("search");
        fs::create_dir_all(root.join("nivel1/nivel2/nivel3")).unwrap();
        fs::create_dir_all(root.join("nivel1/nivel2/nivel3/nivel4/nivel5")).unwrap();
        fs::write(root.join("nivel1/relatorio-mensal.txt"), b"ok").unwrap();
        fs::write(root.join("nivel1/nivel2/nivel3/relatorio-anual.pdf"), b"ok").unwrap();
        fs::write(
            root.join("nivel1/nivel2/nivel3/nivel4/nivel5/relatorio-profundo.txt"),
            b"ok",
        )
        .unwrap();

        let hits = search_in_dir_sync(&root.to_string_lossy(), "RELATORIO", Some(20)).unwrap();
        let hit_names: Vec<&str> = hits.iter().map(|e| e.name.as_str()).collect();
        assert!(hit_names.contains(&"relatorio-mensal.txt"));
        assert!(hit_names.contains(&"relatorio-anual.pdf"));
        assert!(!hit_names.contains(&"relatorio-profundo.txt"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_parent_dir_traversal_in_destructive_operations() {
        // `..` nunca pode chegar às APIs de Lixeira/renomeação: o `parent()` bruto
        // de `C:\Users\..` não é vazio, mas o sistema resolve para a raiz.
        let base = std::env::temp_dir();
        let traversal = format!("{}/..", base.to_string_lossy());
        assert!(trash_entry_sync(&traversal).is_err());
        assert!(rename_entry_sync(&traversal, "x.txt").is_err());
        assert!(reject_parent_components(Path::new(&traversal)).is_err());
        assert!(reject_parent_components(&base).is_ok());
    }

#[test]
    fn create_rename_preview_and_pins_preserve_safety_invariants() {
        let root = temp_workspace("ops");
        let created = create_folder_sync(&root.to_string_lossy(), "Documentos").unwrap();
        assert!(created.is_dir);
        assert!(create_folder_sync(&root.to_string_lossy(), "Documentos").is_err());

        let file_a = root.join("a.txt");
        let file_b = root.join("b.txt");
        fs::write(&file_a, "Olá Pulse!".as_bytes()).unwrap();
        fs::write(&file_b, b"nao sobrescrever").unwrap();

        // Não sobrescreve arquivo existente ao renomear
        assert!(rename_entry_sync(&file_a.to_string_lossy(), "b.txt").is_err());

        let renamed = rename_entry_sync(&file_a.to_string_lossy(), "c.txt").unwrap();
        assert_eq!(renamed.name, "c.txt");
        assert!(!file_a.exists());

        let preview = preview_file_sync(&renamed.path).unwrap();
        assert_eq!(preview.kind, "text");
        assert_eq!(preview.text_snippet.as_deref(), Some("Olá Pulse!"));
        assert!(!preview.text_truncated);

        let pins_file = root.join("pins.json");
        let pinned = toggle_pin_in(&pins_file, &created.path).unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].name, "Documentos");
        let unpinned = toggle_pin_in(&pins_file, &created.path).unwrap();
        assert!(unpinned.is_empty());

        let _ = fs::remove_dir_all(&root);
    }
}
