use serde::Serialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::UI::Shell::{
    SHEmptyRecycleBinW, SHQueryRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
    SHERB_NOSOUND, SHQUERYRBINFO,
};

const MAX_DEPTH: usize = 3;
const MAX_FILES_TOTAL: usize = 2_500;
const ROOT_TIMEOUT: Duration = Duration::from_millis(1_500);
const MAX_LARGEST_ITEMS: usize = 12;

#[derive(Clone)]
struct CleanupDefinition {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    recommended: bool,
    roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupItem {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub is_dir: bool,
    pub modified_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupCategory {
    pub id: String,
    pub name: String,
    pub description: String,
    pub recommended: bool,
    pub available_bytes: u64,
    pub item_count: usize,
    pub is_partial: bool,
    pub unavailable_reason: Option<String>,
    pub largest_items: Vec<CleanupItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupAnalysis {
    pub categories: Vec<CleanupCategory>,
    pub scanned_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub reclaimed_bytes: u64,
    pub deleted_files: usize,
    pub deleted_directories: usize,
    pub skipped_items: usize,
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn browser_cache_roots(local: &Path, browser: &str) -> Vec<PathBuf> {
    let user_data = local.join(browser).join("User Data");
    let mut profiles = vec![user_data.join("Default")];
    if let Ok(entries) = fs::read_dir(&user_data) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("Profile ") {
                profiles.push(entry.path());
            }
        }
    }
    profiles
        .into_iter()
        .flat_map(|profile| {
            [
                profile.join("Cache").join("Cache_Data"),
                profile.join("Code Cache"),
                profile.join("GPUCache"),
            ]
        })
        .collect()
}

fn firefox_cache_roots(local: &Path) -> Vec<PathBuf> {
    let profiles = local.join("Mozilla").join("Firefox").join("Profiles");
    let Ok(entries) = fs::read_dir(profiles) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path().join("cache2"))
        .collect()
}

fn query_recycle_bin() -> Result<(u64, usize), String> {
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: caminho nulo consulta todas as unidades do usuário atual; `info` é
    // um buffer válido e tem `cbSize` inicializado conforme o contrato da API.
    let status = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
    if status < 0 {
        return Err(format!(
            "A Lixeira do Windows não pôde ser consultada (HRESULT 0x{status:08X})"
        ));
    }
    Ok((
        u64::try_from(info.i64Size).unwrap_or_default(),
        usize::try_from(info.i64NumItems).unwrap_or_default(),
    ))
}

fn definitions() -> Vec<CleanupDefinition> {
    let local = env_path("LOCALAPPDATA").unwrap_or_else(env::temp_dir);
    let windows = env_path("WINDIR").unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let program_data = env_path("PROGRAMDATA").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let browser_roots = browser_cache_roots(&local, r"Microsoft\Edge")
        .into_iter()
        .chain(browser_cache_roots(&local, "Google\\Chrome"))
        .chain(firefox_cache_roots(&local))
        .collect();
    vec![
        CleanupDefinition {
            id: "user-temp",
            name: "Arquivos temporários do usuário",
            description: "Sobras temporárias de aplicativos na pasta TEMP. Arquivos em uso são preservados.",
            recommended: true,
            roots: vec![env::temp_dir()],
        },
        CleanupDefinition {
            id: "windows-temp",
            name: "Temporários do Windows",
            description: "Arquivos temporários do sistema que não estão bloqueados pelo Windows.",
            recommended: true,
            roots: vec![windows.join("Temp")],
        },
        CleanupDefinition {
            id: "browser-cache",
            name: "Cache de navegadores",
            description: "Cache local do Edge, Chrome e Firefox. Sites podem carregar mais devagar na primeira visita.",
            recommended: true,
            roots: browser_roots,
        },
        CleanupDefinition {
            id: "shader-cache",
            name: "Cache de shaders DirectX",
            description: "Shaders compilados que jogos e aplicativos recriam quando necessário.",
            recommended: true,
            roots: vec![local.join("D3DSCache")],
        },
        CleanupDefinition {
            id: "crash-dumps",
            name: "Relatórios e dumps de falha",
            description: "Diagnósticos antigos de aplicativos. Desmarque se pretende investigar uma falha recente.",
            recommended: false,
            roots: vec![
                local.join("CrashDumps"),
                local.join("Microsoft").join("Windows").join("WER"),
                windows.join("Minidump"),
            ],
        },
        CleanupDefinition {
            id: "delivery-optimization",
            name: "Cache de Otimização de Entrega",
            description: "Cópias locais usadas para acelerar downloads de atualizações. O Windows baixa novamente se precisar.",
            recommended: true,
            roots: vec![program_data
                .join("Microsoft")
                .join("Windows")
                .join("DeliveryOptimization")
                .join("Cache")],
        },
        CleanupDefinition {
            id: "internet-cache",
            name: "Cache de internet do Windows",
            description: "Arquivos temporários mantidos por componentes web do Windows e aplicativos antigos.",
            recommended: true,
            roots: vec![local.join("Microsoft").join("Windows").join("INetCache")],
        },
        CleanupDefinition {
            id: "recycle-bin",
            name: "Lixeira do Windows",
            description: "Arquivos excluídos que ainda podem ser restaurados. Ao limpar, a exclusão se torna permanente.",
            recommended: false,
            roots: Vec::new(),
        },
    ]
}

fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

struct ScanBudget {
    started: Instant,
    files: usize,
    partial: bool,
}

fn measured_size(path: &Path, depth: usize, budget: &mut ScanBudget) -> u64 {
    if depth > MAX_DEPTH
        || budget.files >= MAX_FILES_TOTAL
        || budget.started.elapsed() >= ROOT_TIMEOUT
    {
        budget.partial = true;
        return 0;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() {
        return 0;
    }
    if metadata.is_file() {
        budget.files += 1;
        if budget.files.is_multiple_of(15) {
            std::thread::yield_now();
        }
        return metadata.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| measured_size(&entry.path(), depth + 1, budget))
        .sum()
}

fn analyze_definition(definition: CleanupDefinition) -> CleanupCategory {
    if definition.id == "recycle-bin" {
        return match query_recycle_bin() {
            Ok((available_bytes, item_count)) => CleanupCategory {
                id: definition.id.into(),
                name: definition.name.into(),
                description: definition.description.into(),
                recommended: definition.recommended,
                available_bytes,
                item_count,
                is_partial: false,
                unavailable_reason: None,
                largest_items: Vec::new(),
            },
            Err(reason) => CleanupCategory {
                id: definition.id.into(),
                name: definition.name.into(),
                description: definition.description.into(),
                recommended: definition.recommended,
                available_bytes: 0,
                item_count: 0,
                is_partial: false,
                unavailable_reason: Some(reason),
                largest_items: Vec::new(),
            },
        };
    }
    let mut largest_items = Vec::new();
    let mut available_bytes: u64 = 0;
    let mut item_count = 0;
    let mut partial = false;
    let mut found_root = false;

    for root in &definition.roots {
        if !root.is_dir() {
            continue;
        }
        found_root = true;
        let mut budget = ScanBudget {
            started: Instant::now(),
            files: 0,
            partial: false,
        };
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if metadata.file_type().is_symlink() {
                    continue;
                }
                let size = measured_size(&path, 0, &mut budget);
                available_bytes = available_bytes.saturating_add(size);
                item_count += 1;
                largest_items.push(CleanupItem {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                    size_bytes: size,
                    is_dir: metadata.is_dir(),
                    modified_at_unix_ms: modified_ms(&metadata),
                });
                if budget.partial {
                    break;
                }
            }
        }
        partial |= budget.partial;
    }
    largest_items.sort_by_key(|item| std::cmp::Reverse(item.size_bytes));
    largest_items.truncate(MAX_LARGEST_ITEMS);

    CleanupCategory {
        id: definition.id.into(),
        name: definition.name.into(),
        description: definition.description.into(),
        recommended: definition.recommended,
        available_bytes,
        item_count,
        is_partial: partial,
        unavailable_reason: (!found_root).then(|| "Pasta não encontrada neste computador".into()),
        largest_items,
    }
}

#[tauri::command]
pub async fn analyze_disk_cleanup() -> CleanupAnalysis {
    let categories = tauri::async_runtime::spawn_blocking(|| {
        definitions().into_iter().map(analyze_definition).collect()
    })
    .await
    .unwrap_or_default();
    crate::debug::record("disk_cleanup_analysis", 0, Ok(()));
    CleanupAnalysis {
        categories,
        scanned_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    }
}

fn remove_entry(path: &Path, result: &mut CleanupResult, visited: &mut usize) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        result.skipped_items += 1;
        return;
    };
    if metadata.file_type().is_symlink() {
        result.skipped_items += 1;
        return;
    }
    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            result.skipped_items += 1;
            return;
        };
        for entry in entries.flatten() {
            remove_entry(&entry.path(), result, visited);
        }
        match fs::remove_dir(path) {
            Ok(()) => result.deleted_directories += 1,
            Err(_) => result.skipped_items += 1,
        }
    } else {
        let size = metadata.len();
        match fs::remove_file(path) {
            Ok(()) => {
                result.deleted_files += 1;
                result.reclaimed_bytes = result.reclaimed_bytes.saturating_add(size);
            }
            Err(_) => result.skipped_items += 1,
        }
    }
    *visited += 1;
    if (*visited).is_multiple_of(15) {
        std::thread::yield_now();
    }
}

fn clean_selected(category_ids: &[String]) -> CleanupResult {
    let selected: std::collections::HashSet<&str> =
        category_ids.iter().map(String::as_str).collect();
    let mut result = CleanupResult {
        reclaimed_bytes: 0,
        deleted_files: 0,
        deleted_directories: 0,
        skipped_items: 0,
    };
    let mut visited = 0;
    for definition in definitions()
        .into_iter()
        .filter(|definition| selected.contains(definition.id))
    {
        if definition.id == "recycle-bin" {
            let before = query_recycle_bin().unwrap_or_default();
            // SAFETY: HWND e caminho nulos são aceitos pela API e significam todas as
            // Lixeiras do usuário. A confirmação já ocorreu na interface do Pulse.
            let status = unsafe {
                SHEmptyRecycleBinW(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
                )
            };
            if status >= 0 {
                let after = query_recycle_bin().unwrap_or_default();
                result.reclaimed_bytes = result
                    .reclaimed_bytes
                    .saturating_add(before.0.saturating_sub(after.0));
                result.deleted_files = result
                    .deleted_files
                    .saturating_add(before.1.saturating_sub(after.1));
            } else {
                result.skipped_items = result.skipped_items.saturating_add(before.1.max(1));
            }
            continue;
        }
        for root in definition.roots {
            if let Ok(entries) = fs::read_dir(&root) {
                for entry in entries.flatten() {
                    remove_entry(&entry.path(), &mut result, &mut visited);
                }
            }
        }
    }
    result
}

#[tauri::command]
pub async fn clean_disk_categories(category_ids: Vec<String>) -> Result<CleanupResult, String> {
    let allowed: std::collections::HashSet<String> = definitions()
        .iter()
        .map(|definition| definition.id.to_owned())
        .collect();
    if category_ids.is_empty() {
        return Err("Selecione ao menos uma categoria para limpar.".into());
    }
    if category_ids.iter().any(|id| !allowed.contains(id.as_str())) {
        return Err("A seleção contém uma categoria de limpeza inválida.".into());
    }
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || clean_selected(&category_ids))
        .await
        .map_err(|error| format!("A limpeza não pôde ser concluída: {error}"))?;
    crate::debug::record("disk_cleanup", started.elapsed().as_millis(), Ok(()));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::definitions;

    #[test]
    fn cleanup_categories_use_unique_stable_ids() {
        let definitions = definitions();
        let mut ids: Vec<_> = definitions.iter().map(|definition| definition.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), definitions.len());
        assert!(ids.contains(&"user-temp"));
        assert!(ids.contains(&"recycle-bin"));
        assert!(ids.contains(&"delivery-optimization"));
        assert!(definitions.iter().any(|definition| !definition.recommended));
    }
}
