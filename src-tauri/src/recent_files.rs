use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_RESULTS: usize = 200;
const MAX_AGE_DAYS: u64 = 30;
const MAX_DEPTH: usize = 3;
const MAX_DIRS_TOTAL: usize = 150;
const MAX_FILES_TOTAL: usize = 2500;
const MAX_SCAN_DURATION: Duration = Duration::from_millis(1500);

const FOLDERS: &[&str] = &[
    "Downloads",
    "Desktop",
    "Documents",
    "Pictures",
    "Videos",
    "Music",
];
const IGNORED: &[&str] = &[
    "node_modules",
    ".git",
    "__pycache__",
    "venv",
    ".venv",
    "dist",
    "build",
    "$RECYCLE.BIN",
    ".cache",
    "AppData",
    "Local Settings",
    "Application Data",
    "target",
    "vendor",
    ".next",
    ".nuxt",
    ".cargo",
    ".rustup",
    ".vscode",
    ".idea",
    "OneDriveTemp",
    "VirtualBox VMs",
    ".gradle",
    ".m2",
    "bin",
    "obj",
];

static CACHE: Mutex<Option<(Vec<RecentFile>, Instant)>> = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(60);

fn data_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
}

fn custom_folders_file() -> PathBuf {
    data_dir().join("custom-scan-folders.json")
}

pub fn load_custom_folders() -> Vec<String> {
    let file = custom_folders_file();
    if let Ok(content) = fs::read_to_string(&file) {
        if let Ok(folders) = serde_json::from_str::<Vec<String>>(&content) {
            return folders;
        }
    }
    Vec::new()
}

pub fn save_custom_folders(folders: &[String]) -> Result<(), String> {
    let dir = data_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Não foi possível criar diretório de configuração: {e}"))?;
    let content = serde_json::to_string_pretty(folders)
        .map_err(|e| format!("Erro ao serializar pastas: {e}"))?;
    fs::write(custom_folders_file(), content)
        .map_err(|e| format!("Não foi possível salvar pastas personalizadas: {e}"))?;
    Ok(())
}

struct ScanBudget {
    directories_visited: usize,
    files_checked: usize,
    deadline: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecentFile {
    pub path: String,
    pub name: String,
    pub category: String,
    pub size_bytes: u64,
    pub modified_at_unix_ms: i64,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecentFilesQuery {
    pub limit: Option<usize>,
    pub query: Option<String>,
    pub category: Option<String>,
}

fn category(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "py" | "js" | "ts" | "jsx" | "tsx" | "sh" | "ps1" | "java" | "c" | "cpp" | "h" | "go"
        | "rs" | "json" | "yaml" | "yml" | "toml" | "sql" => Some("code"),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => Some("image"),
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "mp3" | "wav" | "flac" | "m4a" | "ogg" => {
            Some("media")
        }
        "pdf" | "docx" | "xlsx" | "pptx" | "txt" | "md" => Some("document"),
        "exe" | "msi" | "zip" | "rar" | "7z" | "iso" => Some("installer"),
        _ => None,
    }
}

fn ignored(path: &Path) -> bool {
    path.components()
        .any(|c| IGNORED.iter().any(|x| c.as_os_str() == *x))
}

fn default_roots() -> Vec<PathBuf> {
    let user_profile = env::var_os("USERPROFILE").map(PathBuf::from);
    let known_folders: [fn() -> Option<PathBuf>; 6] = [
        dirs::download_dir,
        dirs::desktop_dir,
        dirs::document_dir,
        dirs::picture_dir,
        dirs::video_dir,
        dirs::audio_dir,
    ];

    known_folders
        .into_iter()
        .zip(FOLDERS)
        .filter_map(|(known_folder, fallback_name)| {
            known_folder().or_else(|| {
                user_profile
                    .as_ref()
                    .map(|profile| profile.join(fallback_name))
            })
        })
        .collect()
}

fn roots() -> Vec<PathBuf> {
    let mut all = default_roots();

    for custom in load_custom_folders() {
        let p = PathBuf::from(&custom);
        if !all.iter().any(|root| root == &p) {
            all.push(p);
        }
    }
    all
}

fn scan(
    root: &Path,
    out: &mut Vec<RecentFile>,
    cutoff: SystemTime,
    depth: usize,
    budget: &mut ScanBudget,
) {
    if depth > MAX_DEPTH
        || out.len() >= MAX_RESULTS
        || budget.directories_visited >= MAX_DIRS_TOTAL
        || budget.files_checked >= MAX_FILES_TOTAL
        || Instant::now() >= budget.deadline
        || ignored(root)
    {
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    budget.directories_visited += 1;

    for entry in entries.flatten() {
        if out.len() >= MAX_RESULTS
            || budget.files_checked >= MAX_FILES_TOTAL
            || Instant::now() >= budget.deadline
        {
            break;
        }

        budget.files_checked += 1;

        // Yield cooperativo a cada 15 arquivos para não saturar filas de I/O do kernel e evitar travamentos no mouse
        if budget.files_checked.is_multiple_of(15) {
            std::thread::yield_now();
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        // Ignora symlinks e junções NTFS (ex: Documents\My Music -> Music) para evitar loops e duplicatas
        if file_type.is_symlink() {
            continue;
        }

        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        if name_str.starts_with('.') || name_str.starts_with('$') {
            continue;
        }

        let path = entry.path();
        if ignored(&path) {
            continue;
        }

        if file_type.is_dir() {
            scan(&path, out, cutoff, depth + 1, budget);
            continue;
        }

        let Some(cat) = category(&path) else {
            continue;
        };

        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < cutoff {
            continue;
        }
        let ms = modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        out.push(RecentFile {
            path: path.to_string_lossy().into_owned(),
            name: name_str.into_owned(),
            category: cat.into(),
            size_bytes: meta.len(),
            modified_at_unix_ms: ms,
        });
    }
}

fn scan_all() -> Vec<RecentFile> {
    let mut files = Vec::new();
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(MAX_AGE_DAYS * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut budget = ScanBudget {
        directories_visited: 0,
        files_checked: 0,
        deadline: Instant::now() + MAX_SCAN_DURATION,
    };
    for root in roots() {
        if budget.directories_visited >= MAX_DIRS_TOTAL
            || budget.files_checked >= MAX_FILES_TOTAL
            || Instant::now() >= budget.deadline
            || files.len() >= MAX_RESULTS
        {
            break;
        }
        scan(&root, &mut files, cutoff, 0, &mut budget);
    }
    files.sort_by(|a, b| {
        b.modified_at_unix_ms
            .cmp(&a.modified_at_unix_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    files
}

fn get_cached_or_scan(force_refresh: bool) -> Vec<RecentFile> {
    if !force_refresh {
        if let Ok(guard) = CACHE.lock() {
            if let Some((ref files, instant)) = *guard {
                if instant.elapsed() < CACHE_TTL {
                    return files.clone();
                }
            }
        }
    }

    let files = scan_all();
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((files.clone(), Instant::now()));
    }
    files
}

fn collect(q: RecentFilesQuery, force_refresh: bool) -> Vec<RecentFile> {
    let files = get_cached_or_scan(force_refresh);
    let needle = q.query.map(|x| x.to_ascii_lowercase());
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_RESULTS);
    files
        .into_iter()
        .filter(|f| q.category.as_ref().is_none_or(|c| c == &f.category))
        .filter(|f| {
            needle
                .as_ref()
                .is_none_or(|n| f.path.to_ascii_lowercase().contains(n))
        })
        .take(limit)
        .collect()
}

#[tauri::command]
pub async fn list_recent_files(query: Option<RecentFilesQuery>) -> Result<Vec<RecentFile>, String> {
    let started = Instant::now();
    let result =
        tauri::async_runtime::spawn_blocking(move || collect(query.unwrap_or_default(), false))
            .await
            .map_err(|e| e.to_string());
    crate::debug::record(
        "recent_files",
        started.elapsed().as_millis(),
        result.as_ref().map(|_| ()).map_err(|_| "Falha na consulta"),
    );
    result
}

#[tauri::command]
pub async fn refresh_recent_files() -> Result<Vec<RecentFile>, String> {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || {
        collect(
            RecentFilesQuery {
                limit: Some(MAX_RESULTS),
                ..Default::default()
            },
            true,
        )
    })
    .await
    .map_err(|e| e.to_string());
    crate::debug::record(
        "refresh_recent_files",
        started.elapsed().as_millis(),
        result
            .as_ref()
            .map(|_| ())
            .map_err(|_| "Falha na atualização"),
    );
    result
}

fn validated(path: &str) -> Result<PathBuf, String> {
    let p = fs::canonicalize(path).map_err(|e| format!("invalid recent file: {e}"))?;
    let allowed = roots()
        .into_iter()
        .filter_map(|r| fs::canonicalize(r).ok())
        .any(|r| p.starts_with(r) && !ignored(&p));
    if !allowed || !p.is_file() || category(&p).is_none() {
        return Err("path is outside the recent-file scope".into());
    }
    Ok(p)
}

#[cfg(windows)]
fn explorer_select_argument(path: &Path) -> String {
    format!(r#"/select,"{}""#, path.display())
}

fn launch(path: &Path, reveal: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let mut command = crate::process::hidden_command("explorer.exe");
        if reveal {
            // Explorer parses this switch from the raw command line and expects
            // the selected path immediately after the comma. Supplying two argv
            // entries makes it ignore the path and open the default folder.
            command.raw_arg(explorer_select_argument(path));
        } else {
            command.arg(path);
        }
        command.spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = reveal;
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn open_recent_file(path: String) -> Result<(), String> {
    launch(&validated(&path)?, false)
}

#[tauri::command]
pub fn open_recent_file_location(path: String) -> Result<(), String> {
    launch(&validated(&path)?, true)
}

#[tauri::command]
pub fn get_custom_scan_folders() -> Vec<String> {
    load_custom_folders()
}

#[tauri::command]
pub fn add_custom_scan_folder(path: String) -> Result<Vec<String>, String> {
    let clean = path.trim();
    if clean.is_empty() {
        return Err("O caminho da pasta não pode ser vazio.".into());
    }
    let mut folders = load_custom_folders();
    if folders.iter().any(|f| f.eq_ignore_ascii_case(clean)) {
        return Err("Esta pasta já está na lista de varredura.".into());
    }
    folders.push(clean.to_string());
    save_custom_folders(&folders)?;
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
    Ok(folders)
}

#[tauri::command]
pub fn remove_custom_scan_folder(path: String) -> Result<Vec<String>, String> {
    let clean = path.trim();
    let mut folders = load_custom_folders();
    let initial_len = folders.len();
    folders.retain(|f| !f.eq_ignore_ascii_case(clean));
    if folders.len() == initial_len {
        return Err("Pasta não encontrada na lista.".into());
    }
    save_custom_folders(&folders)?;
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn categories_match_legacy_extensions() {
        assert_eq!(category(Path::new("x.TSX")), Some("code"));
        assert_eq!(category(Path::new("x.pdf")), Some("document"));
        assert_eq!(category(Path::new("x.bin")), None);
    }
    #[test]
    fn ignored_directories_match_legacy() {
        assert!(ignored(Path::new("C:/Users/me/AppData/file.txt")));
        assert!(ignored(Path::new("x/node_modules/a.js")));
        assert!(!ignored(Path::new("x/Documents/a.txt")));
    }
    #[test]
    fn query_limit_is_bounded() {
        assert_eq!(
            RecentFilesQuery {
                limit: Some(999),
                ..Default::default()
            }
            .limit
            .unwrap()
            .clamp(1, MAX_RESULTS),
            MAX_RESULTS
        );
    }
    #[test]
    fn custom_folders_deduplicate() {
        let path = "D:\\Images\\Screenshots";
        let mut folders = vec!["C:\\Users\\lucas\\Custom".to_string()];
        if !folders.iter().any(|f| f.eq_ignore_ascii_case(path)) {
            folders.push(path.to_string());
        }
        assert_eq!(folders.len(), 2);
        assert!(folders
            .iter()
            .any(|f| f.eq_ignore_ascii_case("d:\\images\\screenshots")));
    }

    #[cfg(windows)]
    #[test]
    fn default_roots_use_the_configured_windows_known_folders() {
        let pictures = dirs::picture_dir().expect("Windows Pictures folder should resolve");
        assert!(default_roots().contains(&pictures));
    }

    #[cfg(windows)]
    #[test]
    fn explorer_select_quotes_the_exact_path_after_the_comma() {
        let path = Path::new(r"D:\Images\Screenshots\Captura de tela 2026-09-14 164716.png");
        assert_eq!(
            explorer_select_argument(path),
            r#"/select,"D:\Images\Screenshots\Captura de tela 2026-09-14 164716.png""#
        );
    }
}
