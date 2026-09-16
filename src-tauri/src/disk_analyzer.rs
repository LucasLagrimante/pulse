use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Window};

static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);
static IS_SCANNING: AtomicBool = AtomicBool::new(false);
// A profundidade não é podada: o usuário precisa conseguir descer até o
// último nível encontrado. O teto alto apenas evita um valor arbitrário vindo
// do frontend provocar recursão ilimitada em uma árvore malformada.
const MAX_TREE_DEPTH: usize = 64;
const PROGRESS_INTERVAL_MS: u128 = 120;
const SNAPSHOT_INTERVAL_MS: u128 = 350;
const STREAM_SNAPSHOT_MAX_DEPTH: usize = 5;
const STREAM_SNAPSHOT_MAX_CHILDREN: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub path: String,
    pub label: String,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiskItemNode {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub is_dir: bool,
    pub modified_at_unix_ms: Option<i64>,
    pub children: Vec<DiskItemNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskScanProgress {
    pub items_scanned: u64,
    pub total_bytes_scanned: u64,
    pub current_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_result: Option<DiskItemNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskScanState {
    pub is_scanning: bool,
    pub progress: Option<DiskScanProgress>,
    pub result: Option<DiskItemNode>,
    pub error: Option<String>,
}

/// Resultado persistido da última varredura concluída. Nunca representa uma
/// leitura ao vivo: a interface sempre o sinaliza como cache com seu horário.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedDiskScan {
    pub result: DiskItemNode,
    pub completed_at_unix_ms: i64,
    pub duration_ms: u64,
    #[serde(default)]
    pub is_partial: bool,
    #[serde(default)]
    pub items_scanned: u64,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

static SCAN_STATE: Mutex<DiskScanState> = Mutex::new(DiskScanState {
    is_scanning: false,
    progress: None,
    result: None,
    error: None,
});

fn set_scan_state(next: DiskScanState) {
    if let Ok(mut state) = SCAN_STATE.lock() {
        *state = next;
    }
}

fn set_scan_progress(progress: DiskScanProgress) {
    if let Ok(mut state) = SCAN_STATE.lock() {
        if progress.partial_result.is_none() {
            if let Some(previous) = state.progress.as_mut() {
                previous.items_scanned = progress.items_scanned;
                previous.total_bytes_scanned = progress.total_bytes_scanned;
                previous.current_path = progress.current_path;
                return;
            }
        }
        state.progress = Some(progress);
    }
}

fn data_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
}

fn disk_scan_cache_file() -> PathBuf {
    data_dir().join("last-disk-scan.json")
}

fn persist_disk_scan(cache: &CachedDiskScan) -> Result<(), String> {
    fs::create_dir_all(data_dir())
        .map_err(|error| format!("Não foi possível criar o diretório do cache: {error}"))?;
    let json = serde_json::to_string(cache)
        .map_err(|error| format!("Não foi possível serializar o cache da varredura: {error}"))?;
    fs::write(disk_scan_cache_file(), json)
        .map_err(|error| format!("Não foi possível salvar o cache da varredura: {error}"))
}

#[tauri::command]
pub fn get_cached_disk_scan() -> Option<CachedDiskScan> {
    let content = fs::read_to_string(disk_scan_cache_file()).ok()?;
    let cache = serde_json::from_str::<CachedDiskScan>(&content).ok()?;
    let ended_by_legacy_limit = matches!(
        cache.stop_reason.as_deref(),
        Some("itemLimit" | "timeout" | "batchLimit")
    );
    if ended_by_legacy_limit {
        None
    } else {
        Some(cache)
    }
}

#[derive(Debug, Deserialize)]
struct RawDriveOutput {
    #[serde(rename = "deviceId")]
    device_id: Option<String>,
    #[serde(rename = "volumeName")]
    volume_name: Option<String>,
    size: Option<u64>,
    #[serde(rename = "freeSpace")]
    free_space: Option<u64>,
}

pub fn parse_drives_json(json_str: &str) -> Vec<DriveInfo> {
    let raw: Result<Vec<RawDriveOutput>, _> = serde_json::from_str(json_str);
    if let Ok(list) = raw {
        return list
            .into_iter()
            .filter_map(|d| {
                let id = d.device_id?;
                let path = if id.ends_with('\\') {
                    id.clone()
                } else {
                    format!("{id}\\")
                };
                let vol = d.volume_name.filter(|v| !v.trim().is_empty());
                let label = match vol {
                    Some(v) => format!("{v} ({id})"),
                    None => format!("Disco Local ({id})"),
                };
                Some(DriveInfo {
                    path,
                    label,
                    total_bytes: d.size,
                    free_bytes: d.free_space,
                })
            })
            .collect();
    }

    if let Ok(single) = serde_json::from_str::<RawDriveOutput>(json_str) {
        if let Some(id) = single.device_id {
            let path = if id.ends_with('\\') {
                id.clone()
            } else {
                format!("{id}\\")
            };
            let label = single
                .volume_name
                .filter(|v| !v.trim().is_empty())
                .map(|v| format!("{v} ({id})"))
                .unwrap_or_else(|| format!("Disco Local ({id})"));
            return vec![DriveInfo {
                path,
                label,
                total_bytes: single.size,
                free_bytes: single.free_space,
            }];
        }
    }

    Vec::new()
}

#[tauri::command]
pub fn get_system_drives() -> Vec<DriveInfo> {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        extern "system" {
            fn GetLogicalDrives() -> u32;
            fn GetDriveTypeW(lpRootPathName: *const u16) -> u32;
            fn GetVolumeInformationW(
                lpRootPathName: *const u16,
                lpVolumeNameBuffer: *mut u16,
                nVolumeNameSize: u32,
                lpVolumeSerialNumber: *mut u32,
                lpMaximumComponentLength: *mut u32,
                lpFileSystemFlags: *mut u32,
                lpFileSystemNameBuffer: *mut u16,
                nFileSystemNameSize: u32,
            ) -> i32;
            fn GetDiskFreeSpaceExW(
                lpDirectoryName: *const u16,
                lpFreeBytesAvailableToCaller: *mut u64,
                lpTotalNumberOfBytes: *mut u64,
                lpTotalNumberOfFreeBytes: *mut u64,
            ) -> i32;
        }

        let bitmask = unsafe { GetLogicalDrives() };
        let mut drives = Vec::new();

        if bitmask != 0 {
            for i in 0..26 {
                if (bitmask & (1 << i)) != 0 {
                    let letter = (b'A' + i as u8) as char;
                    let path = format!("{letter}:\\");
                    let wide_path: Vec<u16> = OsStr::new(&path)
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect();

                    let drive_type = unsafe { GetDriveTypeW(wide_path.as_ptr()) };
                    // DRIVE_REMOVABLE = 2, DRIVE_FIXED = 3
                    if drive_type != 2 && drive_type != 3 {
                        continue;
                    }

                    let mut vol_buf = [0u16; 260];
                    let vol_ok = unsafe {
                        GetVolumeInformationW(
                            wide_path.as_ptr(),
                            vol_buf.as_mut_ptr(),
                            vol_buf.len() as u32,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            0,
                        )
                    };

                    let vol_name = if vol_ok != 0 {
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
                        format!("Disco Local ({letter}:)")
                    };

                    let mut free_avail: u64 = 0;
                    let mut total_bytes: u64 = 0;
                    let mut total_free: u64 = 0;
                    let space_ok = unsafe {
                        GetDiskFreeSpaceExW(
                            wide_path.as_ptr(),
                            &mut free_avail,
                            &mut total_bytes,
                            &mut total_free,
                        )
                    };

                    let (total, free) = if space_ok != 0 && total_bytes > 0 {
                        (Some(total_bytes), Some(total_free))
                    } else {
                        (None, None)
                    };

                    drives.push(DriveInfo {
                        path,
                        label,
                        total_bytes: total,
                        free_bytes: free,
                    });
                }
            }
        }

        if !drives.is_empty() {
            return drives;
        }

        // Fallback rápido no Windows: verificar unidades de C: a Z:
        let mut fallback = Vec::new();
        for letter in b'C'..=b'Z' {
            let p = format!("{}:\\", letter as char);
            if Path::new(&p).exists() {
                fallback.push(DriveInfo {
                    path: p.clone(),
                    label: format!("Disco Local ({}:)", letter as char),
                    total_bytes: None,
                    free_bytes: None,
                });
            }
        }
        fallback
    }

    #[cfg(not(windows))]
    {
        let mut drives = Vec::new();
        for &mnt in &["/mnt/c", "/mnt/d", "/mnt/e"] {
            if Path::new(mnt).exists() {
                drives.push(DriveInfo {
                    path: mnt.to_owned(),
                    label: format!("Disco ({mnt})"),
                    total_bytes: None,
                    free_bytes: None,
                });
            }
        }
        if drives.is_empty() && Path::new("/").exists() {
            drives.push(DriveInfo {
                path: "/".to_owned(),
                label: "Raiz do Sistema (/)".to_owned(),
                total_bytes: None,
                free_bytes: None,
            });
        }
        drives
    }
}

pub struct ScanContext<'a> {
    pub window: Option<&'a Window>,
    pub items_scanned: &'a AtomicU64,
    pub bytes_scanned: &'a AtomicU64,
    pub last_progress_emit: &'a Mutex<Instant>,
    pub last_snapshot_emit: &'a Mutex<Instant>,
    pub max_depth: usize,
    pub max_children_per_dir: usize,
}

impl<'a> ScanContext<'a> {
    pub fn maybe_emit_progress(&self, current_path: &str) {
        let count = self.items_scanned.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(15) {
            std::thread::yield_now();
        }
        if count.is_multiple_of(250) {
            std::thread::sleep(Duration::from_millis(1));
        }
        if !count.is_multiple_of(15) && !count.is_multiple_of(200) {
            return;
        }
        let progress = DiskScanProgress {
            items_scanned: count,
            total_bytes_scanned: self.bytes_scanned.load(Ordering::Relaxed),
            current_path: current_path.to_owned(),
            partial_result: None,
        };
        set_scan_progress(progress.clone());
        if count.is_multiple_of(200) {
            if let Ok(mut guard) = self.last_progress_emit.lock() {
                if guard.elapsed().as_millis() >= PROGRESS_INTERVAL_MS {
                    *guard = Instant::now();
                    if let Some(win) = self.window {
                        let _ = win.emit("pulse://disk-scan-progress", progress);
                    }
                }
            }
        }
    }

    fn should_stop(&self) -> bool {
        CANCEL_FLAG.load(Ordering::Relaxed)
    }

    fn snapshot_is_due(&self) -> bool {
        self.last_snapshot_emit
            .lock()
            .map(|guard| guard.elapsed().as_millis() >= SNAPSHOT_INTERVAL_MS)
            .unwrap_or(false)
    }

    fn emit_partial_snapshot(&self, snapshot: DiskItemNode) {
        let Ok(mut guard) = self.last_snapshot_emit.lock() else {
            return;
        };
        if guard.elapsed().as_millis() < SNAPSHOT_INTERVAL_MS {
            return;
        }
        *guard = Instant::now();

        let snapshot = compact_stream_snapshot(
            &snapshot,
            STREAM_SNAPSHOT_MAX_DEPTH,
            STREAM_SNAPSHOT_MAX_CHILDREN,
            0,
        );

        let progress = DiskScanProgress {
            items_scanned: self.items_scanned.load(Ordering::Relaxed),
            total_bytes_scanned: self.bytes_scanned.load(Ordering::Relaxed),
            current_path: snapshot.path.clone(),
            partial_result: Some(snapshot),
        };
        set_scan_progress(progress.clone());
        if let Some(win) = self.window {
            let _ = win.emit("pulse://disk-scan-progress", progress);
        }
    }
}

/// Snapshots durante a busca precisam ser leves. O resultado final continua
/// completo, mas os pacotes intermediários mostram o ramo ativo e resumem o
/// restante para não bloquear o IPC nem o React.
fn compact_stream_snapshot(
    node: &DiskItemNode,
    max_depth: usize,
    max_children: usize,
    depth: usize,
) -> DiskItemNode {
    let children = if node.children.is_empty() {
        Vec::new()
    } else if depth >= max_depth {
        vec![DiskItemNode {
            id: format!("{}::[stream-depth]", node.path),
            name: "[Níveis abaixo ainda em análise]".to_owned(),
            path: node.path.clone(),
            size_bytes: node.children.iter().map(|child| child.size_bytes).sum(),
            file_count: node.children.iter().map(|child| child.file_count).sum(),
            dir_count: node.children.iter().map(|child| child.dir_count).sum(),
            is_dir: false,
            modified_at_unix_ms: None,
            children: Vec::new(),
        }]
    } else {
        let mut indexes: Vec<usize> = (0..node.children.len()).collect();
        indexes.sort_by_key(|index| std::cmp::Reverse(node.children[*index].size_bytes));
        let keep_count = max_children.saturating_sub(1);
        let mut compacted = indexes
            .iter()
            .take(if indexes.len() > max_children {
                keep_count
            } else {
                indexes.len()
            })
            .map(|index| {
                compact_stream_snapshot(&node.children[*index], max_depth, max_children, depth + 1)
            })
            .collect::<Vec<_>>();
        if indexes.len() > max_children {
            let rest = indexes
                .iter()
                .skip(keep_count)
                .map(|index| &node.children[*index]);
            compacted.push(DiskItemNode {
                id: format!("{}::[stream-rest]", node.path),
                name: format!("[Outros {} itens em análise]", indexes.len() - keep_count),
                path: node.path.clone(),
                size_bytes: rest.clone().map(|child| child.size_bytes).sum(),
                file_count: rest.clone().map(|child| child.file_count).sum(),
                dir_count: rest.map(|child| child.dir_count).sum(),
                is_dir: false,
                modified_at_unix_ms: None,
                children: Vec::new(),
            });
        }
        compacted
    };

    DiskItemNode {
        id: node.id.clone(),
        name: node.name.clone(),
        path: node.path.clone(),
        size_bytes: node.size_bytes,
        file_count: node.file_count,
        dir_count: node.dir_count,
        is_dir: node.is_dir,
        modified_at_unix_ms: node.modified_at_unix_ms,
        children,
    }
}

fn partial_children(
    children: &[DiskItemNode],
    parent_path: &str,
    max_children: usize,
) -> Vec<DiskItemNode> {
    let mut sorted = children.to_vec();
    sorted.sort_by_key(|node| std::cmp::Reverse(node.size_bytes));
    if sorted.len() <= max_children {
        return sorted;
    }

    let keep_count = max_children.saturating_sub(1);
    let mut kept = sorted[..keep_count].to_vec();
    let rest = &sorted[keep_count..];
    let rest_size = rest.iter().map(|node| node.size_bytes).sum();
    let rest_files = rest.iter().map(|node| node.file_count).sum();
    let rest_dirs = rest.iter().map(|node| node.dir_count).sum();

    kept.push(DiskItemNode {
        id: format!("{parent_path}::[stream-rest]"),
        name: format!("[Outros {} itens em análise]", children.len() - keep_count),
        path: parent_path.to_owned(),
        size_bytes: rest_size,
        file_count: rest_files,
        dir_count: rest_dirs,
        is_dir: false,
        modified_at_unix_ms: None,
        children: Vec::new(),
    });
    kept.sort_by_key(|node| std::cmp::Reverse(node.size_bytes));
    kept
}

#[allow(clippy::too_many_arguments)]
fn directory_snapshot(
    id: &str,
    name: &str,
    path: &str,
    modified_at_unix_ms: Option<i64>,
    size_bytes: u64,
    file_count: u64,
    dir_count: u64,
    children: &[DiskItemNode],
    max_children: usize,
) -> DiskItemNode {
    DiskItemNode {
        id: id.to_owned(),
        name: name.to_owned(),
        path: path.to_owned(),
        size_bytes,
        file_count,
        dir_count,
        is_dir: true,
        modified_at_unix_ms,
        children: partial_children(children, path, max_children),
    }
}

pub fn scan_path_recursive(path: &Path, current_depth: usize, ctx: &ScanContext) -> DiskItemNode {
    let mut ignore_partial = |_snapshot: DiskItemNode| {};
    scan_path_recursive_stream(path, current_depth, ctx, &mut ignore_partial)
}

fn scan_path_recursive_stream(
    path: &Path,
    current_depth: usize,
    ctx: &ScanContext,
    on_partial: &mut dyn FnMut(DiskItemNode),
) -> DiskItemNode {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let path_str = path.to_string_lossy().into_owned();

    if ctx.should_stop() {
        return DiskItemNode {
            id: path_str.clone(),
            name,
            path: path_str,
            size_bytes: 0,
            file_count: 0,
            dir_count: 0,
            is_dir: true,
            modified_at_unix_ms: None,
            children: Vec::new(),
        };
    }

    ctx.maybe_emit_progress(&path_str);

    let Ok(meta) = fs::symlink_metadata(path) else {
        return DiskItemNode {
            id: path_str.clone(),
            name,
            path: path_str,
            size_bytes: 0,
            file_count: 0,
            dir_count: 0,
            is_dir: false,
            modified_at_unix_ms: None,
            children: Vec::new(),
        };
    };

    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64);

    // Se for link simbólico ou junção, não seguimos para evitar ciclos infinitos
    if meta.file_type().is_symlink() {
        return DiskItemNode {
            id: path_str.clone(),
            name,
            path: path_str,
            size_bytes: meta.len(),
            file_count: 1,
            dir_count: 0,
            is_dir: false,
            modified_at_unix_ms: modified_ms,
            children: Vec::new(),
        };
    }

    if !meta.is_dir() {
        let size = meta.len();
        ctx.bytes_scanned.fetch_add(size, Ordering::Relaxed);
        return DiskItemNode {
            id: path_str.clone(),
            name,
            path: path_str,
            size_bytes: size,
            file_count: 1,
            dir_count: 0,
            is_dir: false,
            modified_at_unix_ms: modified_ms,
            children: Vec::new(),
        };
    }

    let Ok(entries) = fs::read_dir(path) else {
        return DiskItemNode {
            id: path_str.clone(),
            name,
            path: path_str,
            size_bytes: 0,
            file_count: 0,
            dir_count: 1,
            is_dir: true,
            modified_at_unix_ms: modified_ms,
            children: Vec::new(),
        };
    };

    // Ordenar a raiz torna o resultado determinístico sem impor limite de
    // quantidade ou tempo à varredura completa.
    let mut entries = entries.flatten().collect::<Vec<_>>();
    if current_depth == 0 {
        entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    }
    let mut direct_children = Vec::new();
    let mut stream_children = Vec::new();
    let mut total_size = 0u64;
    let mut total_files = 0u64;
    let mut total_dirs = 1u64;

    if ctx.snapshot_is_due() {
        on_partial(directory_snapshot(
            &path_str,
            &name,
            &path_str,
            modified_ms,
            0,
            0,
            1,
            &[],
            STREAM_SNAPSHOT_MAX_CHILDREN,
        ));
    }

    for entry in entries {
        if ctx.should_stop() {
            break;
        }
        let child_path = entry.path();
        if current_depth < ctx.max_depth {
            let child_node = {
                let mut bubble_partial = |partial_child: DiskItemNode| {
                    let mut children = stream_children.clone();
                    let partial_size = partial_child.size_bytes;
                    let partial_files = partial_child.file_count;
                    let partial_dirs = partial_child.dir_count;
                    children.push(partial_child);
                    on_partial(directory_snapshot(
                        &path_str,
                        &name,
                        &path_str,
                        modified_ms,
                        total_size.saturating_add(partial_size),
                        total_files.saturating_add(partial_files),
                        total_dirs.saturating_add(partial_dirs),
                        &children,
                        STREAM_SNAPSHOT_MAX_CHILDREN,
                    ));
                };
                scan_path_recursive_stream(&child_path, current_depth + 1, ctx, &mut bubble_partial)
            };
            total_size = total_size.saturating_add(child_node.size_bytes);
            total_files = total_files.saturating_add(child_node.file_count);
            total_dirs = total_dirs.saturating_add(child_node.dir_count);
            stream_children.push(compact_stream_snapshot(
                &child_node,
                STREAM_SNAPSHOT_MAX_DEPTH.saturating_sub(1),
                STREAM_SNAPSHOT_MAX_CHILDREN,
                0,
            ));
            direct_children.push(child_node);
            if ctx.snapshot_is_due() {
                on_partial(directory_snapshot(
                    &path_str,
                    &name,
                    &path_str,
                    modified_ms,
                    total_size,
                    total_files,
                    total_dirs,
                    &stream_children,
                    STREAM_SNAPSHOT_MAX_CHILDREN,
                ));
            }
        } else {
            // A profundidade limita apenas a árvore enviada ao React. Abaixo
            // dela ainda percorremos tudo para somar tamanho e contagens reais.
            let mut aggregate = AggregateTotals::default();
            let mut bubble_aggregate = |partial: AggregateTotals| {
                on_partial(directory_snapshot(
                    &path_str,
                    &name,
                    &path_str,
                    modified_ms,
                    total_size.saturating_add(partial.size_bytes),
                    total_files.saturating_add(partial.file_count),
                    total_dirs.saturating_add(partial.dir_count),
                    &stream_children,
                    STREAM_SNAPSHOT_MAX_CHILDREN,
                ));
            };
            scan_path_aggregate_stream(&child_path, ctx, &mut aggregate, &mut bubble_aggregate);
            total_size = total_size.saturating_add(aggregate.size_bytes);
            total_files = total_files.saturating_add(aggregate.file_count);
            total_dirs = total_dirs.saturating_add(aggregate.dir_count);
        }
    }

    // Ordena do maior para o menor tamanho (padrão TreeSize)
    direct_children.sort_by_key(|node| std::cmp::Reverse(node.size_bytes));

    // Otimização: se houver mais filhos que o limite máximo de renderização na pasta,
    // mantém os maiores e agrega os menores em um nó sintético amigável
    let final_children = if direct_children.len() > ctx.max_children_per_dir {
        let keep_count = ctx.max_children_per_dir - 1;
        let mut kept = direct_children[..keep_count].to_vec();
        let rest = &direct_children[keep_count..];

        let rest_size: u64 = rest.iter().map(|n| n.size_bytes).sum();
        let rest_files: u64 = rest.iter().map(|n| n.file_count).sum();
        let rest_dirs: u64 = rest.iter().map(|n| n.dir_count).sum();

        kept.push(DiskItemNode {
            id: format!("{path_str}::[rest]"),
            name: format!("[Outros {} itens menores]", rest.len()),
            path: path_str.clone(),
            size_bytes: rest_size,
            file_count: rest_files,
            dir_count: rest_dirs,
            is_dir: false,
            modified_at_unix_ms: None,
            children: Vec::new(),
        });
        kept
    } else {
        direct_children
    };

    DiskItemNode {
        id: path_str.clone(),
        name,
        path: path_str,
        size_bytes: total_size,
        file_count: total_files,
        dir_count: total_dirs,
        is_dir: true,
        modified_at_unix_ms: modified_ms,
        children: final_children,
    }
}

#[derive(Clone, Copy, Default)]
struct AggregateTotals {
    size_bytes: u64,
    file_count: u64,
    dir_count: u64,
}

fn scan_path_aggregate_stream(
    path: &Path,
    ctx: &ScanContext,
    totals: &mut AggregateTotals,
    on_partial: &mut dyn FnMut(AggregateTotals),
) {
    if ctx.should_stop() {
        return;
    }

    let path_str = path.to_string_lossy();
    ctx.maybe_emit_progress(&path_str);
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    if !meta.is_dir() {
        let size = meta.len();
        ctx.bytes_scanned.fetch_add(size, Ordering::Relaxed);
        totals.size_bytes = totals.size_bytes.saturating_add(size);
        totals.file_count = totals.file_count.saturating_add(1);
        if ctx.snapshot_is_due() {
            on_partial(*totals);
        }
        return;
    }

    totals.dir_count = totals.dir_count.saturating_add(1);
    let Ok(entries) = fs::read_dir(path) else {
        if ctx.snapshot_is_due() {
            on_partial(*totals);
        }
        return;
    };
    for entry in entries.flatten() {
        if ctx.should_stop() {
            break;
        }
        scan_path_aggregate_stream(&entry.path(), ctx, totals, on_partial);
    }
}

#[tauri::command]
pub async fn start_disk_scan(
    window: Window,
    path: String,
    max_depth: Option<usize>,
) -> Result<DiskItemNode, String> {
    if IS_SCANNING.swap(true, Ordering::SeqCst) {
        return Err("Uma varredura já está em andamento.".to_owned());
    }

    CANCEL_FLAG.store(false, Ordering::SeqCst);

    set_scan_state(DiskScanState {
        is_scanning: true,
        progress: Some(DiskScanProgress {
            items_scanned: 0,
            total_bytes_scanned: 0,
            current_path: path.clone(),
            partial_result: None,
        }),
        result: None,
        error: None,
    });

    let target_path = PathBuf::from(&path);
    if !target_path.exists() {
        IS_SCANNING.store(false, Ordering::SeqCst);
        set_scan_state(DiskScanState {
            is_scanning: false,
            progress: None,
            result: None,
            error: Some(format!("O caminho informado não existe: {path}")),
        });
        return Err(format!("O caminho informado não existe: {path}"));
    }

    let depth_limit = max_depth.unwrap_or(MAX_TREE_DEPTH).min(MAX_TREE_DEPTH);
    let scan_started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let items_scanned = AtomicU64::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let last_progress_emit = Mutex::new(Instant::now() - Duration::from_millis(250));
        let last_snapshot_emit =
            Mutex::new(Instant::now() - Duration::from_millis(SNAPSHOT_INTERVAL_MS as u64));

        let ctx = ScanContext {
            window: Some(&window),
            items_scanned: &items_scanned,
            bytes_scanned: &bytes_scanned,
            last_progress_emit: &last_progress_emit,
            last_snapshot_emit: &last_snapshot_emit,
            max_depth: depth_limit,
            max_children_per_dir: 40,
        };

        let mut publish_partial = |snapshot: DiskItemNode| {
            ctx.emit_partial_snapshot(snapshot);
        };
        let result = scan_path_recursive_stream(&target_path, 0, &ctx, &mut publish_partial);
        let cancelled = CANCEL_FLAG.load(Ordering::Relaxed);
        let is_partial = cancelled;
        let stop_reason = if cancelled {
            Some("cancelled".to_owned())
        } else {
            None
        };
        let final_progress = DiskScanProgress {
            items_scanned: items_scanned.load(Ordering::Relaxed),
            total_bytes_scanned: result.size_bytes,
            current_path: if cancelled {
                "Varredura cancelada".to_owned()
            } else {
                "Varredura concluída".to_owned()
            },
            partial_result: Some(result.clone()),
        };
        set_scan_progress(final_progress.clone());
        set_scan_state(DiskScanState {
            is_scanning: false,
            progress: Some(final_progress.clone()),
            result: Some(result.clone()),
            error: None,
        });
        IS_SCANNING.store(false, Ordering::SeqCst);

        // Emite o progresso final para qualquer tela que esteja montada.
        let _ = window.emit("pulse://disk-scan-progress", final_progress);
        (
            result,
            is_partial,
            items_scanned.load(Ordering::Relaxed),
            stop_reason,
        )
    })
    .await
    .map_err(|error| {
        IS_SCANNING.store(false, Ordering::SeqCst);
        let message = format!("Falha ao executar a varredura: {error}");
        set_scan_state(DiskScanState {
            is_scanning: false,
            progress: None,
            result: None,
            error: Some(message.clone()),
        });
        message
    })?;

    let (result, is_partial, items_scanned, stop_reason) = result;
    let completed_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    let cache = CachedDiskScan {
        result: result.clone(),
        completed_at_unix_ms,
        duration_ms: scan_started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        is_partial,
        items_scanned,
        stop_reason,
    };
    // Uma falha ao salvar não invalida a análise recém-concluída.
    let _ = persist_disk_scan(&cache);

    Ok(result)
}

#[tauri::command]
pub fn cancel_disk_scan() -> Result<bool, String> {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
    Ok(true)
}

#[tauri::command]
pub fn is_disk_scanning() -> bool {
    IS_SCANNING.load(Ordering::Relaxed)
}

#[tauri::command]
pub fn get_disk_scan_state() -> Result<DiskScanState, String> {
    SCAN_STATE
        .lock()
        .map(|state| state.clone())
        .map_err(|_| "Não foi possível consultar o estado da varredura.".to_owned())
}

#[tauri::command]
pub fn open_disk_item(path: String, reveal: bool) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("O caminho não existe ou não pôde ser acessado.".to_owned());
    }

    #[cfg(windows)]
    {
        let mut command = crate::process::hidden_command("explorer.exe");
        if reveal {
            command.arg(format!("/select,{}", p.display()));
        } else {
            command.arg(&p);
        }
        command.spawn().map(|_| ()).map_err(|e| e.to_string())
    }

    #[cfg(not(windows))]
    {
        let target = if reveal && p.is_file() {
            p.parent().unwrap_or(&p).to_path_buf()
        } else {
            p
        };
        std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_drives_json_multiple() {
        let json = r#"[
            {"deviceId":"C:","volumeName":"Windows","size":1000000,"freeSpace":400000},
            {"deviceId":"D:","volumeName":"Dados","size":2000000,"freeSpace":800000}
        ]"#;
        let drives = parse_drives_json(json);
        assert_eq!(drives.len(), 2);
        assert_eq!(drives[0].path, "C:\\");
        assert_eq!(drives[0].label, "Windows (C:)");
        assert_eq!(drives[1].path, "D:\\");
        assert_eq!(drives[1].label, "Dados (D:)");
    }

    #[test]
    fn parses_drives_json_single() {
        let json = r#"{"deviceId":"C:","volumeName":"","size":500000,"freeSpace":250000}"#;
        let drives = parse_drives_json(json);
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].path, "C:\\");
        assert_eq!(drives[0].label, "Disco Local (C:)");
    }

    #[test]
    fn scans_directory_tree_and_sorts_by_size() {
        let temp = std::env::temp_dir().join(format!("pulse_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(temp.join("folder_small")).unwrap();
        fs::create_dir_all(temp.join("folder_large")).unwrap();

        fs::write(temp.join("folder_small").join("a.txt"), b"123").unwrap();
        fs::write(temp.join("folder_large").join("b.bin"), vec![0u8; 1000]).unwrap();
        fs::write(temp.join("root_file.txt"), vec![1u8; 50]).unwrap();

        let items_scanned = AtomicU64::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let last_progress_emit = Mutex::new(Instant::now());
        let last_snapshot_emit = Mutex::new(Instant::now());

        let ctx = ScanContext {
            window: None,
            items_scanned: &items_scanned,
            bytes_scanned: &bytes_scanned,
            last_progress_emit: &last_progress_emit,
            last_snapshot_emit: &last_snapshot_emit,
            max_depth: 3,
            max_children_per_dir: 10,
        };

        let node = scan_path_recursive(&temp, 0, &ctx);

        assert!(node.is_dir);
        assert!(node.size_bytes >= 1053);
        assert_eq!(node.file_count, 3);
        assert_eq!(node.children.len(), 3);

        // O primeiro deve ser folder_large pois tem 1000 bytes
        assert_eq!(node.children[0].name, "folder_large");
        assert_eq!(node.children[0].size_bytes, 1000);

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn scans_all_items_across_root_directories_without_a_global_quota() {
        let temp = std::env::temp_dir().join(format!("pulse_full_scan_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        for folder in ["a_large", "z_later"] {
            fs::create_dir_all(temp.join(folder)).unwrap();
            for index in 0..8 {
                fs::write(temp.join(folder).join(format!("{index}.bin")), [1u8; 8]).unwrap();
            }
        }

        let items_scanned = AtomicU64::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let last_progress_emit = Mutex::new(Instant::now());
        let last_snapshot_emit = Mutex::new(Instant::now());
        let ctx = ScanContext {
            window: None,
            items_scanned: &items_scanned,
            bytes_scanned: &bytes_scanned,
            last_progress_emit: &last_progress_emit,
            last_snapshot_emit: &last_snapshot_emit,
            max_depth: 3,
            max_children_per_dir: 10,
        };

        let node = scan_path_recursive(&temp, 0, &ctx);
        let first = node
            .children
            .iter()
            .find(|child| child.name == "a_large")
            .unwrap();
        let later = node
            .children
            .iter()
            .find(|child| child.name == "z_later")
            .unwrap();
        assert_eq!(first.file_count, 8);
        assert_eq!(later.file_count, 8);
        assert_eq!(node.file_count, 16);

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn streams_the_active_branch_before_its_root_child_finishes() {
        let temp = std::env::temp_dir().join(format!("pulse_stream_scan_{}", std::process::id()));
        let active_branch = temp.join("large-folder").join("nested");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&active_branch).unwrap();
        fs::write(active_branch.join("first.bin"), [1u8; 3]).unwrap();
        fs::write(active_branch.join("second.bin"), [2u8; 7]).unwrap();

        let items_scanned = AtomicU64::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let last_progress_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
        let last_snapshot_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
        let ctx = ScanContext {
            window: None,
            items_scanned: &items_scanned,
            bytes_scanned: &bytes_scanned,
            last_progress_emit: &last_progress_emit,
            last_snapshot_emit: &last_snapshot_emit,
            max_depth: MAX_TREE_DEPTH,
            max_children_per_dir: 10,
        };
        let mut snapshots = Vec::new();
        let mut collect = |snapshot| snapshots.push(snapshot);

        let result = scan_path_recursive_stream(&temp, 0, &ctx, &mut collect);

        assert_eq!(result.size_bytes, 10);
        assert!(snapshots.iter().any(|snapshot| {
            snapshot.size_bytes > 0
                && snapshot.size_bytes < result.size_bytes
                && snapshot
                    .children
                    .iter()
                    .any(|child| child.name == "large-folder")
        }));

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn progress_events_do_not_postpone_partial_tree_snapshots() {
        let items_scanned = AtomicU64::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let last_progress_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
        let last_snapshot_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
        let ctx = ScanContext {
            window: None,
            items_scanned: &items_scanned,
            bytes_scanned: &bytes_scanned,
            last_progress_emit: &last_progress_emit,
            last_snapshot_emit: &last_snapshot_emit,
            max_depth: MAX_TREE_DEPTH,
            max_children_per_dir: 10,
        };

        for index in 0..200 {
            ctx.maybe_emit_progress(&format!("item-{index}"));
        }

        assert!(last_progress_emit.lock().unwrap().elapsed().as_millis() < PROGRESS_INTERVAL_MS);
        assert!(ctx.snapshot_is_due());
    }

    #[test]
    fn aggregates_every_level_below_the_visible_tree_depth() {
        let temp = std::env::temp_dir().join(format!("pulse_deep_scan_{}", std::process::id()));
        let deep = temp
            .join("one")
            .join("two")
            .join("three")
            .join("four")
            .join("five");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.bin"), vec![7u8; 4096]).unwrap();

        let items_scanned = AtomicU64::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let last_progress_emit = Mutex::new(Instant::now());
        let last_snapshot_emit = Mutex::new(Instant::now());
        let ctx = ScanContext {
            window: None,
            items_scanned: &items_scanned,
            bytes_scanned: &bytes_scanned,
            last_progress_emit: &last_progress_emit,
            last_snapshot_emit: &last_snapshot_emit,
            max_depth: MAX_TREE_DEPTH,
            max_children_per_dir: 10,
        };

        let node = scan_path_recursive(&temp, 0, &ctx);
        assert_eq!(node.size_bytes, 4096);
        assert_eq!(node.file_count, 1);
        assert!(node.dir_count >= 6);

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn scans_past_the_previous_2500_item_limit() {
        let temp =
            std::env::temp_dir().join(format!("pulse_over_2500_scan_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        for index in 0..2_601 {
            fs::write(temp.join(format!("item-{index}.bin")), [1u8]).unwrap();
        }

        let items_scanned = AtomicU64::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let last_progress_emit = Mutex::new(Instant::now());
        let last_snapshot_emit = Mutex::new(Instant::now());
        let ctx = ScanContext {
            window: None,
            items_scanned: &items_scanned,
            bytes_scanned: &bytes_scanned,
            last_progress_emit: &last_progress_emit,
            last_snapshot_emit: &last_snapshot_emit,
            max_depth: MAX_TREE_DEPTH,
            max_children_per_dir: 10,
        };

        let node = scan_path_recursive(&temp, 0, &ctx);
        assert_eq!(node.file_count, 2_601);
        assert!(items_scanned.load(Ordering::Relaxed) > 2_500);

        let _ = fs::remove_dir_all(&temp);
    }
}
