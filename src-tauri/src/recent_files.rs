use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_RESULTS: usize = 200;
const MAX_AGE_DAYS: u64 = 30;
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
];

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
fn roots() -> Vec<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(|h| FOLDERS.iter().map(|n| PathBuf::from(&h).join(n)).collect())
        .unwrap_or_default()
}
fn scan(root: &Path, out: &mut Vec<RecentFile>, cutoff: SystemTime) {
    if out.len() >= MAX_RESULTS || ignored(root) {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_RESULTS {
            break;
        }
        let path = entry.path();
        if ignored(&path) {
            continue;
        }
        if path.is_dir() {
            scan(&path, out, cutoff);
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
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            category: cat.into(),
            size_bytes: meta.len(),
            modified_at_unix_ms: ms,
        });
    }
}
fn collect(q: RecentFilesQuery) -> Vec<RecentFile> {
    let mut files = Vec::new();
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(MAX_AGE_DAYS * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for root in roots() {
        scan(&root, &mut files, cutoff);
    }
    files.sort_by(|a, b| {
        b.modified_at_unix_ms
            .cmp(&a.modified_at_unix_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    let needle = q.query.map(|x| x.to_ascii_lowercase());
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_RESULTS);
    files
        .into_iter()
        .filter(|f| q.category.as_ref().map_or(true, |c| c == &f.category))
        .filter(|f| {
            needle
                .as_ref()
                .map_or(true, |n| f.path.to_ascii_lowercase().contains(n))
        })
        .take(limit)
        .collect()
}

#[tauri::command]
pub fn list_recent_files(query: Option<RecentFilesQuery>) -> Vec<RecentFile> {
    collect(query.unwrap_or_default())
}
#[tauri::command]
pub fn refresh_recent_files() -> Vec<RecentFile> {
    collect(RecentFilesQuery {
        limit: Some(MAX_RESULTS),
        ..Default::default()
    })
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
fn launch(path: &Path, reveal: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("explorer.exe");
        if reveal {
            command.arg(format!("/select,{}", path.display()));
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
}
