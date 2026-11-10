//! Pegada real em disco dos programas instalados.
//!
//! `EstimatedSize` do registro é escrito pelo instalador no dia da instalação
//! e nunca mais atualizado — navegadores, jogos e apps com auto-update crescem
//! muito além dele. Aqui medimos a pasta de instalação de verdade, com custo
//! controlado:
//!
//! - roda em `spawn_blocking`, em lotes pequenos pedidos pela UI;
//! - orçamento de entradas por app e prazo por lote; estourou, o resultado
//!   vira **limite inferior** (`complete = false`), nunca um número inventado;
//! - `yield_now` a cada 15 entradas para não enfileirar I/O/DPC do kernel;
//! - não segue junction/symlink e recusa pastas vetadas (raiz de unidade,
//!   Windows, pastas do shell, pasta que contém outra instalação);
//! - cache de 10 minutos por pasta normalizada.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::installed::collect_cached;
use super::residue::{folder_deletion_veto, is_reparse_point, normalize, shell_folder_patterns};

/// Entradas lidas por app antes de desistir (cobre apps grandes comuns; jogos
/// com centenas de milhares de arquivos viram limite inferior).
const PER_APP_MAX_ENTRIES: usize = 60_000;
/// Prazo de um app dentro do lote.
const PER_APP_DEADLINE: Duration = Duration::from_millis(1_500);
/// Prazo total de um lote: a UI pede o próximo lote só depois deste.
const BATCH_DEADLINE: Duration = Duration::from_millis(4_000);
/// Máximo de apps aceitos por chamada.
const BATCH_MAX_APPS: usize = 24;
const YIELD_EVERY: usize = 15;
const CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppFootprint {
    pub app_id: String,
    /// Bytes somados na pasta de instalação; `None` quando não houve medição.
    pub measured_bytes: Option<u64>,
    /// `false` quando o orçamento/prazo acabou: `measured_bytes` é mínimo.
    pub complete: bool,
    pub file_count: u64,
    pub install_location: Option<String>,
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

fn cache_get(key: &str) -> Option<CachedMeasure> {
    let guard = CACHE.lock().ok()?;
    let hit = guard.as_ref()?.get(key)?.clone();
    (hit.at.elapsed() < CACHE_TTL).then_some(hit)
}

fn cache_put(key: String, value: CachedMeasure) {
    if let Ok(mut guard) = CACHE.lock() {
        guard.get_or_insert_with(HashMap::new).insert(key, value);
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

fn unavailable(app_id: &str, location: Option<String>, reason: &str) -> AppFootprint {
    AppFootprint {
        app_id: app_id.to_string(),
        measured_bytes: None,
        complete: false,
        file_count: 0,
        install_location: location,
        unavailable_reason: Some(reason.to_string()),
    }
}

/// Vários instaladores gravam `InstallLocation` entre aspas (`"C:\\Program Files\\X"`);
/// sem tirá-las o caminho parece relativo e o veto recusa a medição.
pub fn clean_install_location(raw: &str) -> &str {
    raw.trim().trim_matches('"').trim()
}

pub fn measure_apps(app_ids: &[String]) -> Vec<AppFootprint> {
    let snapshot = collect_cached();
    let shell_patterns = shell_folder_patterns();
    let install_dirs: Vec<(String, String)> = snapshot
        .apps
        .iter()
        .filter_map(|app| {
            let location = clean_install_location(app.install_location.as_deref()?);
            (!location.is_empty()).then(|| (app.id.clone(), normalize(Path::new(location))))
        })
        .collect();

    let batch_deadline = Instant::now() + BATCH_DEADLINE;
    let mut results = Vec::with_capacity(app_ids.len().min(BATCH_MAX_APPS));

    for app_id in app_ids.iter().take(BATCH_MAX_APPS) {
        if !snapshot.apps.iter().any(|app| &app.id == app_id) {
            results.push(unavailable(app_id, None, "Programa não está mais no inventário"));
            continue;
        }
        let Some((_, normalized)) = install_dirs.iter().find(|(id, _)| id == app_id) else {
            results.push(unavailable(app_id, None, "O instalador não informou a pasta de instalação"));
            continue;
        };
        let location = Some(normalized.clone());

        // Pastas compartilhadas por mais de um programa não são de nenhum deles.
        let shared = install_dirs
            .iter()
            .any(|(id, other)| id != app_id && other.eq_ignore_ascii_case(normalized));
        if shared {
            results.push(unavailable(app_id, location, "Pasta compartilhada com outro programa"));
            continue;
        }
        let others: Vec<String> = install_dirs
            .iter()
            .filter(|(id, _)| id != app_id)
            .map(|(_, dir)| dir.clone())
            .collect();
        if let Some(reason) = folder_deletion_veto(normalized, &shell_patterns, &others) {
            results.push(unavailable(app_id, location, &format!("Pasta não medida: {reason}")));
            continue;
        }

        let path = Path::new(normalized);
        if !path.is_dir() {
            results.push(unavailable(app_id, location, "A pasta de instalação não existe mais"));
            continue;
        }
        if is_reparse_point(path) {
            results.push(unavailable(app_id, location, "A pasta de instalação é um link"));
            continue;
        }

        let key = normalized.to_lowercase();
        let measure = match cache_get(&key) {
            Some(hit) => hit,
            None => {
                let now = Instant::now();
                if now >= batch_deadline {
                    // Sem tempo neste lote: a UI pede de novo no próximo.
                    results.push(unavailable(app_id, location, "Medição adiada"));
                    continue;
                }
                let deadline = (now + PER_APP_DEADLINE).min(batch_deadline);
                let (bytes, files, complete) = measure_bounded(path, PER_APP_MAX_ENTRIES, deadline);
                let value = CachedMeasure { bytes, files, complete, at: Instant::now() };
                cache_put(key, value.clone());
                value
            }
        };

        results.push(AppFootprint {
            app_id: app_id.clone(),
            measured_bytes: Some(measure.bytes),
            complete: measure.complete,
            file_count: measure.files,
            install_location: location,
            unavailable_reason: None,
        });
    }
    results
}

/// Mede a pasta de instalação de até `BATCH_MAX_APPS` programas.
#[tauri::command]
pub async fn measure_installed_app_sizes(app_ids: Vec<String>) -> Vec<AppFootprint> {
    tauri::async_runtime::spawn_blocking(move || measure_apps(&app_ids))
        .await
        .unwrap_or_default()
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
        assert_eq!(clean_install_location(" \"C:\\Program Files\\AnyDesk\" "), "C:\\Program Files\\AnyDesk");
        assert_eq!(clean_install_location("C:\\Apps\\X"), "C:\\Apps\\X");
        assert_eq!(clean_install_location("\"\""), "");
    }

    #[test]
    fn orcamento_estourado_vira_limite_inferior() {
        let root = std::env::temp_dir().join(format!("pulse-footprint-cap-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for index in 0..10 {
            fs::write(root.join(format!("{index}.bin")), vec![0u8; 10]).unwrap();
        }

        let (bytes, _, complete) = measure_bounded(&root, 3, Instant::now() + Duration::from_secs(5));
        fs::remove_dir_all(&root).ok();

        assert!(!complete);
        assert!(bytes <= 30);
    }
}
