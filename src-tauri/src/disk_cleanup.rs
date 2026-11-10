use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::Emitter;
use windows_sys::Win32::{
    System::{
        Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
        SystemInformation::GetTickCount,
    },
    UI::{
        Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
        Shell::{
            SHEmptyRecycleBinW, SHQueryRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
            SHERB_NOSOUND, SHQUERYRBINFO,
        },
    },
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
    /// Pasta do sistema que só administrador lista e apaga por completo.
    pub requires_admin: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    let profile = env_path("USERPROFILE").unwrap_or_else(env::temp_dir);
    let browser_roots = browser_cache_roots(&local, r"Microsoft\Edge")
        .into_iter()
        .chain(browser_cache_roots(&local, "Google\\Chrome"))
        .chain(firefox_cache_roots(&local))
        .collect();
    vec![
        CleanupDefinition {
            id: "user-temp",
            name: "Arquivos temporários do usuário",
            description: "Sobras temporárias de aplicativos na pasta TEMP. Arquivos em uso, das últimas 24 horas e os do próprio Pulse (build e relatórios) são preservados.",
            recommended: true,
            roots: vec![env::temp_dir()],
        },
        CleanupDefinition {
            id: "windows-temp",
            name: "Temporários do Windows",
            description: "Arquivos temporários do sistema que não estão bloqueados pelo Windows. Os das últimas 24 horas são preservados.",
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
            id: "nvidia-shader-cache",
            name: "Cache de shaders NVIDIA",
            description: "Shaders compilados pelo driver NVIDIA. Jogos recompilam na primeira abertura, com leve travada inicial.",
            recommended: true,
            roots: vec![
                local.join("NVIDIA").join("DXCache"),
                local.join("NVIDIA").join("GLCache"),
            ],
        },
        CleanupDefinition {
            id: "dev-package-caches",
            name: "Caches de ferramentas de desenvolvimento",
            // O Chrome do Puppeteer fica de fora: não é baixado de novo sozinho e
            // os testes quebram até alguém reinstalá-lo.
            description: "Pacotes baixados por npm, uv e Gradle. São baixados de novo no próximo build, que fica mais lento uma vez. Feche builds em andamento antes; só roda na limpeza manual.",
            recommended: false,
            roots: vec![
                local.join("npm-cache"),
                local.join("uv").join("cache"),
                profile.join(".gradle").join("caches"),
            ],
        },
        CleanupDefinition {
            id: "nvidia-installers",
            name: "Instaladores de driver NVIDIA",
            description: "Pacotes de driver já aplicados. Pode exigir o Pulse em modo administrador; itens protegidos são preservados.",
            recommended: false,
            roots: vec![program_data.join("NVIDIA Corporation").join("Downloader")],
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

/// Entradas do próprio Pulse no topo da TEMP: staging e cache de build
/// (`pulse-src`, `pulse-target*`), relatórios para a IA (`pulse-disk-*.md`) e o
/// roteiro de compactação (`pulse-compact.txt`). Apagar parte de um cache de
/// compilação corrompe o build, então nunca são medidas nem removidas.
fn is_pulse_owned_name(name: &str) -> bool {
    name.get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("pulse-"))
}

fn is_pulse_owned(entry: &fs::DirEntry) -> bool {
    is_pulse_owned_name(&entry.file_name().to_string_lossy())
}

/// Categorias em pastas do sistema (Windows, ProgramData) que o Windows só deixa
/// listar e apagar por completo com o Pulse em modo administrador.
fn requires_admin(id: &str) -> bool {
    matches!(
        id,
        "windows-temp" | "delivery-optimization" | "nvidia-installers"
    )
}

fn analyze_definition(definition: CleanupDefinition) -> CleanupCategory {
    if definition.id == "recycle-bin" {
        return match query_recycle_bin() {
            Ok((available_bytes, item_count)) => CleanupCategory {
                id: definition.id.into(),
                name: definition.name.into(),
                description: definition.description.into(),
                recommended: definition.recommended,
                requires_admin: false,
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
                requires_admin: false,
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
            for entry in entries.flatten().filter(|entry| !is_pulse_owned(entry)) {
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
        requires_admin: requires_admin(definition.id),
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

/// `true` quando o item foi modificado depois do corte (ou a data é desconhecida)
/// e por isso deve ser preservado pela limpeza automática.
fn is_recent(metadata: &fs::Metadata, cutoff: Option<SystemTime>) -> bool {
    let Some(cutoff) = cutoff else {
        return false;
    };
    !metadata.modified().is_ok_and(|modified| modified < cutoff)
}

fn remove_entry(
    path: &Path,
    result: &mut CleanupResult,
    visited: &mut usize,
    cutoff: Option<SystemTime>,
) {
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
            remove_entry(&entry.path(), result, visited, cutoff);
        }
        // Com corte de idade, pasta recente ou que ainda guarda arquivos recentes
        // é preservada em silêncio: não é falha, é a regra da limpeza automática.
        if is_recent(&metadata, cutoff) {
            return;
        }
        match fs::remove_dir(path) {
            Ok(()) => result.deleted_directories += 1,
            Err(_) if cutoff.is_some() => {}
            Err(_) => result.skipped_items += 1,
        }
    } else if is_recent(&metadata, cutoff) {
        return;
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

/// Na limpeza manual, temporários das últimas 24 h ficam: um instalador ou
/// atualização em andamento pode ter extraído arquivos que ainda vai usar.
const TEMP_MIN_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn is_temp_category(id: &str) -> bool {
    matches!(id, "user-temp" | "windows-temp")
}

/// Corte de idade de uma categoria: o da limpeza automática (mais rígido)
/// prevalece; sem ele, as pastas TEMP ainda usam o mínimo de 24 h.
fn effective_cutoff(
    id: &str,
    cutoff: Option<SystemTime>,
    temp_cutoff: Option<SystemTime>,
) -> Option<SystemTime> {
    match (cutoff, is_temp_category(id)) {
        (Some(cutoff), _) => Some(cutoff),
        (None, true) => temp_cutoff,
        (None, false) => None,
    }
}

fn clean_selected(category_ids: &[String], cutoff: Option<SystemTime>) -> CleanupResult {
    let selected: std::collections::HashSet<&str> =
        category_ids.iter().map(String::as_str).collect();
    let mut result = CleanupResult {
        reclaimed_bytes: 0,
        deleted_files: 0,
        deleted_directories: 0,
        skipped_items: 0,
    };
    let mut visited = 0;
    let temp_cutoff = SystemTime::now().checked_sub(TEMP_MIN_AGE);
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
        let cutoff = effective_cutoff(definition.id, cutoff, temp_cutoff);
        for root in definition.roots {
            if let Ok(entries) = fs::read_dir(&root) {
                for entry in entries.flatten().filter(|entry| !is_pulse_owned(entry)) {
                    remove_entry(&entry.path(), &mut result, &mut visited, cutoff);
                }
            }
        }
    }
    result
}

/// Impede que a limpeza manual e a automática removam as mesmas pastas ao mesmo tempo.
static CLEANING: AtomicBool = AtomicBool::new(false);

struct CleaningGuard;

impl CleaningGuard {
    fn acquire() -> Option<Self> {
        (!CLEANING.swap(true, Ordering::SeqCst)).then_some(Self)
    }
}

impl Drop for CleaningGuard {
    fn drop(&mut self) {
        CLEANING.store(false, Ordering::SeqCst);
    }
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
    let Some(guard) = CleaningGuard::acquire() else {
        return Err("Uma limpeza já está em andamento. Aguarde a conclusão.".into());
    };
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        clean_selected(&category_ids, None)
    })
    .await
    .map_err(|error| format!("A limpeza não pôde ser concluída: {error}"))?;
    crate::debug::record("disk_cleanup", started.elapsed().as_millis(), Ok(()));
    Ok(result)
}

// ---------------------------------------------------------------------------
// Limpeza automática (opcional, desativada por padrão)
// ---------------------------------------------------------------------------

const AUTO_CLEANUP_FILE: &str = "auto-cleanup.json";
/// Arquivos modificados nos últimos 7 dias nunca entram na limpeza automática.
const AUTO_MIN_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Só roda depois de 5 min sem teclado/mouse.
const AUTO_MIN_IDLE: Duration = Duration::from_secs(5 * 60);
/// Espera inicial para não competir com a inicialização do Windows e do Pulse.
const AUTO_STARTUP_DELAY: Duration = Duration::from_secs(5 * 60);
const AUTO_CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
const AUTO_ALLOWED_INTERVALS: [u32; 3] = [1, 7, 30];
/// A Lixeira guarda arquivos restauráveis do usuário: nunca é esvaziada sozinha.
/// Caches de pacotes também não: o corte por idade apaga arquivo a arquivo e
/// pode deixar uma entrada do Gradle/uv pela metade, quebrando o próximo build.
const AUTO_EXCLUDED: [&str; 2] = ["recycle-bin", "dev-package-caches"];
pub const AUTO_CLEANUP_EVENT: &str = "pulse://auto-cleanup-finished";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoCleanupConfig {
    pub enabled: bool,
    pub interval_days: u32,
    pub category_ids: Vec<String>,
    pub last_run_at_unix_ms: Option<u64>,
    pub last_result: Option<CleanupResult>,
}

impl Default for AutoCleanupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_days: 7,
            category_ids: definitions()
                .into_iter()
                .filter(|definition| definition.recommended && is_auto_eligible(definition.id))
                .map(|definition| definition.id.to_owned())
                .collect(),
            last_run_at_unix_ms: None,
            last_result: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoCleanupCategoryOption {
    pub id: String,
    pub name: String,
    pub recommended: bool,
    pub requires_admin: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoCleanupStatus {
    #[serde(flatten)]
    pub config: AutoCleanupConfig,
    pub next_run_at_unix_ms: Option<u64>,
    pub min_age_days: u64,
    pub available_categories: Vec<AutoCleanupCategoryOption>,
}

fn is_auto_eligible(id: &str) -> bool {
    !AUTO_EXCLUDED.contains(&id)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn auto_cleanup_file() -> PathBuf {
    env_path("LOCALAPPDATA")
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
        .join(AUTO_CLEANUP_FILE)
}

/// Remove ids desconhecidos/inelegíveis e normaliza o intervalo de um arquivo
/// antigo ou editado à mão.
fn sanitize_config(mut config: AutoCleanupConfig) -> AutoCleanupConfig {
    let valid: std::collections::HashSet<&str> = definitions()
        .iter()
        .map(|definition| definition.id)
        .collect();
    let mut seen = std::collections::HashSet::new();
    config.category_ids.retain(|id| {
        valid.contains(id.as_str()) && is_auto_eligible(id) && seen.insert(id.clone())
    });
    if !AUTO_ALLOWED_INTERVALS.contains(&config.interval_days) {
        config.interval_days = 7;
    }
    config
}

fn load_auto_config() -> AutoCleanupConfig {
    fs::read_to_string(auto_cleanup_file())
        .ok()
        .and_then(|content| serde_json::from_str::<AutoCleanupConfig>(&content).ok())
        .map(sanitize_config)
        .unwrap_or_default()
}

fn save_auto_config(config: &AutoCleanupConfig) -> Result<(), String> {
    let file = auto_cleanup_file();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)
            .map_err(|error| format!("Não foi possível criar a pasta de configuração: {error}"))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| format!("Erro ao serializar a limpeza automática: {error}"))?;
    // Escrita atômica: um desligamento no meio não deixa o arquivo corrompido.
    let temp = file.with_extension("json.tmp");
    fs::write(&temp, content)
        .and_then(|()| fs::rename(&temp, &file))
        .map_err(|error| format!("Não foi possível salvar a limpeza automática: {error}"))
}

fn next_run_ms(config: &AutoCleanupConfig) -> Option<u64> {
    if !config.enabled || config.category_ids.is_empty() {
        return None;
    }
    let interval_ms = u64::from(config.interval_days) * 24 * 60 * 60 * 1000;
    // Nunca rodou: fica elegível já na próxima checagem do agendador.
    Some(
        config
            .last_run_at_unix_ms
            .map_or_else(now_ms, |last| last.saturating_add(interval_ms)),
    )
}

fn auto_status(config: AutoCleanupConfig) -> AutoCleanupStatus {
    AutoCleanupStatus {
        next_run_at_unix_ms: next_run_ms(&config),
        min_age_days: AUTO_MIN_AGE.as_secs() / 86_400,
        available_categories: definitions()
            .into_iter()
            .filter(|definition| is_auto_eligible(definition.id))
            .map(|definition| AutoCleanupCategoryOption {
                id: definition.id.into(),
                name: definition.name.into(),
                recommended: definition.recommended,
                requires_admin: requires_admin(definition.id),
            })
            .collect(),
        config,
    }
}

#[tauri::command]
pub async fn get_auto_cleanup() -> AutoCleanupStatus {
    tauri::async_runtime::spawn_blocking(|| auto_status(load_auto_config()))
        .await
        .unwrap_or_else(|_| auto_status(AutoCleanupConfig::default()))
}

#[tauri::command]
pub async fn set_auto_cleanup(
    enabled: bool,
    interval_days: u32,
    category_ids: Vec<String>,
) -> Result<AutoCleanupStatus, String> {
    if !AUTO_ALLOWED_INTERVALS.contains(&interval_days) {
        return Err("Intervalo de limpeza automática inválido.".into());
    }
    let eligible: std::collections::HashSet<&str> = definitions()
        .iter()
        .map(|definition| definition.id)
        .filter(|id| is_auto_eligible(id))
        .collect();
    if category_ids
        .iter()
        .any(|id| !eligible.contains(id.as_str()))
    {
        return Err(
            "A seleção contém uma categoria que não pode ser limpa automaticamente.".into(),
        );
    }
    if enabled && category_ids.is_empty() {
        return Err("Escolha ao menos uma categoria para a limpeza automática.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let previous = load_auto_config();
        let config = sanitize_config(AutoCleanupConfig {
            enabled,
            interval_days,
            category_ids,
            ..previous
        });
        save_auto_config(&config)?;
        Ok(auto_status(config))
    })
    .await
    .map_err(|error| format!("A configuração não pôde ser salva: {error}"))?
}

/// Tempo desde a última entrada de teclado/mouse da sessão interativa.
fn user_idle_time() -> Option<Duration> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` é um buffer válido com `cbSize` inicializado.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // SAFETY: sem parâmetros; ambos os contadores usam a mesma base de 32 bits,
    // então `wrapping_sub` trata o estouro a cada ~49,7 dias.
    let now = unsafe { GetTickCount() };
    Some(Duration::from_millis(u64::from(
        now.wrapping_sub(info.dwTime),
    )))
}

/// `false` só quando o Windows confirma que o notebook está na bateria.
fn on_external_power() -> bool {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` é um buffer válido do tamanho esperado pela API.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return true;
    }
    // 0 = bateria, 1 = tomada, 255 = desconhecido (desktops sem bateria).
    status.ACLineStatus != 0
}

fn is_due(config: &AutoCleanupConfig, now: u64) -> bool {
    next_run_ms(config).is_some_and(|next| now >= next)
}

fn run_auto_cleanup_if_due(app: &tauri::AppHandle) {
    let mut config = load_auto_config();
    if !is_due(&config, now_ms()) {
        return;
    }
    if user_idle_time().is_none_or(|idle| idle < AUTO_MIN_IDLE) || !on_external_power() {
        return;
    }
    let Some(_guard) = CleaningGuard::acquire() else {
        return;
    };
    let started = Instant::now();
    let cutoff = SystemTime::now().checked_sub(AUTO_MIN_AGE);
    let result = {
        #[cfg(windows)]
        let _priority = crate::disk_analyzer::ScopedBackgroundPriority::enter();
        clean_selected(&config.category_ids, cutoff)
    };
    crate::debug::record("disk_cleanup_auto", started.elapsed().as_millis(), Ok(()));
    config.last_run_at_unix_ms = Some(now_ms());
    config.last_result = Some(result);
    if let Err(error) = save_auto_config(&config) {
        eprintln!("Pulse: limpeza automática concluída, mas não foi salva: {error}");
    }
    let _ = app.emit(AUTO_CLEANUP_EVENT, auto_status(config));
}

/// Agendador em thread própria: dorme a maior parte do tempo e só lê um JSON
/// pequeno a cada 15 min. Não faz nada enquanto a opção estiver desligada.
pub fn init(app: tauri::AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("pulse-auto-cleanup".into())
        .spawn(move || {
            std::thread::sleep(AUTO_STARTUP_DELAY);
            loop {
                run_auto_cleanup_if_due(&app);
                std::thread::sleep(AUTO_CHECK_INTERVAL);
            }
        });
    if let Err(error) = spawned {
        eprintln!("Pulse: agendador de limpeza automática indisponível: {error}");
    }
}

// ---------------------------------------------------------------------------
// Recuperação guiada: o Pulse só detecta e explica; o usuário executa os
// comandos como administrador. Nada aqui apaga ou altera arquivos.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuidedInsight {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub path: Option<String>,
    pub size_bytes: Option<u64>,
    pub unavailable_reason: Option<String>,
    pub warning: String,
    pub command: String,
}

/// Discos virtuais conhecidos do WSL e do Docker Desktop (nome, caminho).
fn virtual_disks(local: &Path) -> Vec<(String, PathBuf)> {
    let mut disks = Vec::new();
    // WSL 2.x moderno: %LOCALAPPDATA%\wsl\{guid}\ext4.vhdx
    if let Ok(entries) = fs::read_dir(local.join("wsl")) {
        for entry in entries.flatten() {
            let vhdx = entry.path().join("ext4.vhdx");
            if vhdx.is_file() {
                disks.push(("Distribuição WSL".to_owned(), vhdx));
            }
        }
    }
    // Distribuições da Microsoft Store: Packages\<pacote>\LocalState\ext4.vhdx
    if let Ok(entries) = fs::read_dir(local.join("Packages")) {
        for entry in entries.flatten() {
            let vhdx = entry.path().join("LocalState").join("ext4.vhdx");
            if vhdx.is_file() {
                let package = entry.file_name().to_string_lossy().into_owned();
                let name = package.split('_').next().unwrap_or(&package).to_owned();
                disks.push((format!("WSL ({name})"), vhdx));
            }
        }
    }
    for (name, path) in [
        (
            "Docker Desktop",
            local
                .join("Docker")
                .join("wsl")
                .join("disk")
                .join("docker_data.vhdx"),
        ),
        (
            "Docker Desktop",
            local
                .join("Docker")
                .join("wsl")
                .join("data")
                .join("ext4.vhdx"),
        ),
    ] {
        if path.is_file() {
            disks.push((name.to_owned(), path));
        }
    }
    disks
}

fn compact_command(path: &Path) -> String {
    let file = path.to_string_lossy();
    format!(
        "# PowerShell como Administrador. Feche o Docker Desktop pela bandeja antes.\n\
         wsl --shutdown\n\
         @\"\n\
         select vdisk file=\"{file}\"\n\
         attach vdisk readonly\n\
         compact vdisk\n\
         detach vdisk\n\
         \"@ | Set-Content \"$env:TEMP\\pulse-compact.txt\" -Encoding ascii\n\
         diskpart /s \"$env:TEMP\\pulse-compact.txt\""
    )
}

fn guided_insights() -> Vec<GuidedInsight> {
    let local = env_path("LOCALAPPDATA").unwrap_or_else(env::temp_dir);
    let mut insights: Vec<GuidedInsight> = virtual_disks(&local)
        .into_iter()
        .enumerate()
        .map(|(index, (name, path))| {
            let size = fs::metadata(&path).ok().map(|metadata| metadata.len());
            GuidedInsight {
                id: format!("vhdx-{index}"),
                title: format!("Compactar disco virtual · {name}"),
                detail: "O arquivo .vhdx cresce mas não devolve ao Windows o espaço apagado dentro do Linux/Docker. Compactar mantém todos os dados e só libera o espaço vazio.".into(),
                path: Some(path.to_string_lossy().into_owned()),
                unavailable_reason: size.is_none().then(|| "Tamanho do arquivo não pôde ser lido".into()),
                size_bytes: size,
                warning: "Nunca apague o .vhdx: ele é a distribuição inteira (ou todas as imagens e volumes do Docker). O ganho real depende do espaço livre dentro do disco virtual.".into(),
                command: compact_command(&path),
            }
        })
        .collect();
    insights.push(GuidedInsight {
        id: "windows-component-store".into(),
        title: "Componentes do Windows (WinSxS)".into(),
        detail: "Remove versões substituídas de atualizações com a ferramenta oficial do Windows e esvazia downloads de atualização já aplicadas.".into(),
        path: None,
        size_bytes: None,
        unavailable_reason: Some(
            "Não medido: a WinSxS usa hard links e o tamanho aparente engana".into(),
        ),
        warning: "Nunca apague arquivos da WinSxS à mão; use apenas o DISM.".into(),
        command: "# PowerShell como Administrador\n\
                  DISM /Online /Cleanup-Image /StartComponentCleanup\n\
                  Stop-Service wuauserv -Force\n\
                  Remove-Item \"C:\\Windows\\SoftwareDistribution\\Download\\*\" -Recurse -Force -ErrorAction SilentlyContinue\n\
                  Start-Service wuauserv"
            .into(),
    });
    insights
}

#[tauri::command]
pub async fn get_cleanup_insights() -> Vec<GuidedInsight> {
    tauri::async_runtime::spawn_blocking(guided_insights)
        .await
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        compact_command, definitions, effective_cutoff, is_auto_eligible, is_due,
        is_pulse_owned_name, is_recent, requires_admin, sanitize_config, AutoCleanupConfig,
    };
    use std::time::{Duration, SystemTime};

    #[test]
    fn auto_cleanup_is_disabled_by_default_and_never_empties_recycle_bin() {
        let config = AutoCleanupConfig::default();
        assert!(!config.enabled);
        assert!(!config.category_ids.is_empty());
        assert!(!config.category_ids.iter().any(|id| id == "recycle-bin"));
        assert!(!is_auto_eligible("recycle-bin"));
        assert!(!is_auto_eligible("dev-package-caches"));
        assert!(!is_due(&config, u64::MAX));
    }

    #[test]
    fn manual_cleanup_keeps_last_day_of_temp_files_only() {
        let now = SystemTime::now();
        let day_ago = now - Duration::from_secs(24 * 60 * 60);
        let week_ago = now - Duration::from_secs(7 * 24 * 60 * 60);
        // Manual: TEMP preserva 24 h; as demais categorias não têm corte.
        assert_eq!(
            effective_cutoff("user-temp", None, Some(day_ago)),
            Some(day_ago)
        );
        assert_eq!(
            effective_cutoff("windows-temp", None, Some(day_ago)),
            Some(day_ago)
        );
        assert_eq!(effective_cutoff("browser-cache", None, Some(day_ago)), None);
        // Automática: o corte de 7 dias vale para todas, inclusive TEMP.
        assert_eq!(
            effective_cutoff("user-temp", Some(week_ago), Some(day_ago)),
            Some(week_ago)
        );
    }

    #[test]
    fn dev_caches_never_include_puppeteer_browser() {
        let dev = definitions()
            .into_iter()
            .find(|definition| definition.id == "dev-package-caches")
            .expect("categoria de desenvolvimento");
        assert!(!dev
            .roots
            .iter()
            .any(|root| root.to_string_lossy().contains("puppeteer")));
    }

    #[test]
    fn auto_cleanup_due_only_after_interval() {
        let day = 24 * 60 * 60 * 1000;
        let config = AutoCleanupConfig {
            enabled: true,
            interval_days: 7,
            category_ids: vec!["user-temp".into()],
            last_run_at_unix_ms: Some(10 * day),
            last_result: None,
        };
        assert!(!is_due(&config, 16 * day));
        assert!(is_due(&config, 17 * day));
    }

    #[test]
    fn sanitize_drops_unknown_ids_and_bad_interval() {
        let config = sanitize_config(AutoCleanupConfig {
            enabled: true,
            interval_days: 3,
            category_ids: vec!["user-temp".into(), "recycle-bin".into(), "C:\\".into()],
            last_run_at_unix_ms: None,
            last_result: None,
        });
        assert_eq!(config.category_ids, vec!["user-temp".to_owned()]);
        assert_eq!(config.interval_days, 7);
    }

    #[test]
    fn compact_command_only_attaches_read_only_and_never_deletes() {
        let command = compact_command(std::path::Path::new(r"C:\Users\x\docker_data.vhdx"));
        assert!(command.contains(r#"select vdisk file="C:\Users\x\docker_data.vhdx""#));
        assert!(command.contains("attach vdisk readonly"));
        assert!(command.contains("wsl --shutdown"));
        assert!(!command.to_lowercase().contains("remove-item"));
    }

    #[test]
    fn age_cutoff_preserves_recent_files() {
        let file = std::env::temp_dir().join("pulse-auto-cleanup-age-test.tmp");
        std::fs::write(&file, b"x").unwrap();
        let metadata = std::fs::metadata(&file).unwrap();
        let week_ago = SystemTime::now() - Duration::from_secs(7 * 24 * 60 * 60);
        assert!(is_recent(&metadata, Some(week_ago)));
        assert!(!is_recent(&metadata, None));
        let _ = std::fs::remove_file(&file);
    }

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
        // Toda categoria marcada como "exige administrador" precisa existir.
        for id in ["windows-temp", "delivery-optimization", "nvidia-installers"] {
            assert!(requires_admin(id) && ids.contains(&id), "{id}");
        }
        assert!(!requires_admin("user-temp"));
    }

    #[test]
    fn pulse_own_build_cache_and_reports_are_never_touched() {
        for name in [
            "pulse-src",
            "pulse-target",
            "pulse-target-check",
            "Pulse-Target",
            "pulse-disk-analysis-C.md",
            "pulse-disk-cleanup-plan-C.md",
            "pulse-compact.txt",
        ] {
            assert!(is_pulse_owned_name(name), "{name} deveria ser preservado");
        }
        for name in [
            "pulse",
            "pulsar-cache",
            "npm-cache",
            "tmp1234",
            "pulse.log",
            "é",
        ] {
            assert!(!is_pulse_owned_name(name), "{name} não é do Pulse");
        }
    }
}
