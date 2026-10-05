//! Inventário de programas instalados.
//!
//! A fonte da verdade é a chave `Uninstall` de cada app:
//!
//! - `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall` (64 e 32 bits)
//! - `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
//! - `HKU\<S-1-5-21-*\SOFTWARE\...\Uninstall` (perfis já carregados; o painel de
//!   controle lista o mesmo conjunto)
//!
//! `DisplayName` sozinho não identifica ninguém: duas entradas podem ter o
//! mesmo nome (versões side-by-side) e `UninstallString` pode estar ausente em
//! componentes do sistema. Por isso o `id` é derivado do caminho real da chave,
//! nunca do nome.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::registry::{Hive, RegHandle, RegistryView};

const INVENTORY_CACHE_TTL: Duration = Duration::from_secs(30);
const UNINSTALL_SUBKEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    /// `HKLM|SOFTWARE\...\Uninstall\Chave|64` — identidade estável do registro.
    /// É o que a sessão de desinstalação carrega; o nome pode ser renomeado
    /// pelo uninstaller no meio do processo.
    pub id: String,
    pub hive: String,
    pub view: String,
    pub registry_path: String,
    pub display_name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_date: Option<String>,
    pub install_location: Option<String>,
    pub estimated_size_kb: Option<u64>,
    pub uninstall_string: Option<String>,
    /// `MsiExec.exe` quando o produto é MSI: o desinstalador padrão é o
    /// próprio Windows Installer e a remoção de registro é determinística.
    pub is_msi: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledAppsSnapshot {
    pub apps: Vec<InstalledApp>,
    pub total_count: usize,
    /// Preenchido quando nenhuma raiz pôde ser lida. Uma lista parcial
    /// continua sendo lista — mas a origem incompleta fica declarada.
    pub partial_reason: Option<String>,
    pub scanned_roots: usize,
    pub collected_at_unix_ms: u64,
}

struct InventoryCache {
    snapshot: InstalledAppsSnapshot,
    fetched_at: Instant,
}

static INVENTORY_CACHE: std::sync::Mutex<Option<InventoryCache>> = std::sync::Mutex::new(None);

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Entradas de componente do sistema e subentradas de outros produtos não são
/// "programas instalados" para o usuário — o Painel de Controle também as
/// esconde. `SystemComponent=1` e a presença de `ParentKeyName` são exatamente
/// os marcadores usados pelo shell.
pub fn is_hidden_by_shell(system_component: Option<u32>, parent_key_name: Option<&str>) -> bool {
    if system_component.unwrap_or(0) == 1 {
        return true;
    }
    matches!(parent_key_name, Some(parent) if !parent.trim().is_empty())
}

/// `UninstallString` do Windows Installer: o produto é MSI quando o comando é o
/// `MsiExec` com `/I` ou `/X`. Detectar isso importa porque a limpeza de MSI
/// segue regras diferentes de registro das de apps com chave própria.
pub fn detect_msi(uninstall_string: Option<&str>) -> bool {
    let Some(command) = uninstall_string else {
        return false;
    };
    let program = command.split('"').nth(1).unwrap_or(command);
    if !program.to_ascii_lowercase().contains("msiexec") {
        return false;
    }
    // Token exato: `contains("/i")` casaria com qualquer caminho que tivesse
    // "/i" no meio (`C:\msiexec\install\x`).
    // O switch real vem colado do código do produto — `MsiExec.exe /X{GUID}`
    // tem "/x" como prefixo do token, nunca como token inteiro. Exigir
    // igualdade exata faria o detector errar em 100% dos comandos do Windows
    // Installer; aceitar "/xqualquer" reabriria o `contains` que o teste
    // anterior-existia para fechar.
    command.split_whitespace().any(|token| {
        let switch = token.trim_matches('"').to_ascii_lowercase();
        ["i", "x", "uninstall"].iter().any(|name| {
            switch == format!("/{name}")
                || switch.starts_with(&format!("/{name}="))
                || switch.starts_with(&format!("/{name}{{"))
        })
    })
}

pub fn build_app_id(hive: Hive, view: RegistryView, subkey: &str) -> String {
    format!("{}|{}|{}", hive.label(), subkey, view.label())
}

pub fn parse_app_id(id: &str) -> Option<(Hive, RegistryView, String)> {
    let mut parts = id.split('|');
    let hive = Hive::parse_label(parts.next()?)?;
    let subkey = parts.next()?.to_string();
    let view = RegistryView::parse_label(parts.next()?)?;
    if subkey.is_empty() {
        return None;
    }
    Some((hive, view, subkey))
}

/// Percorre as raízes de desinstalação e monta a lista. Raias ilegíveis não
/// derrubam o resultado: cada uma vira uma linha em `partial_reason`.
pub fn collect() -> InstalledAppsSnapshot {
    let mut apps: Vec<InstalledApp> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut scanned_roots = 0usize;

    for hive in [Hive::LocalMachine, Hive::CurrentUser] {
        for view in [RegistryView::Native64, RegistryView::Native32] {
            match RegHandle::open(hive, UNINSTALL_SUBKEY, view) {
                Ok(handle) => match handle.enum_keys() {
                    Ok(names) => {
                        scanned_roots += 1;
                        for name in names {
                            if let Some(app) = read_app(hive, view, &name) {
                                apps.push(app);
                            }
                        }
                    }
                    Err(code) => problems.push(format!(
                        "{} (visão {}) não pôde ser enumerada (código {})",
                        hive.label(),
                        view.label(),
                        code
                    )),
                },
                // A visão de 32 bits não existe em installs só de 64 bits:
                // chave raiz ausente é normal, não é problema. Qualquer outro
                // código é permissão ou bloqueio e precisa aparecer.
                Err(code) => {
                    if code != super::registry::ERROR_FILE_NOT_FOUND_CODE {
                        problems.push(format!(
                            "{} (visão {}) não pôde ser aberta (código {})",
                            hive.label(),
                            view.label(),
                            code
                        ));
                    }
                }
            }
        }
    }

    scanned_roots += collect_user_hives(&mut apps, &mut problems);

    dedupe(&mut apps);
    apps.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
    });

    let total_count = apps.len();
    InstalledAppsSnapshot {
        apps,
        total_count,
        partial_reason: if problems.is_empty() {
            None
        } else {
            Some(problems.join("; "))
        },
        scanned_roots,
        collected_at_unix_ms: now_unix_ms(),
    }
}

/// Perfis montados em `HKU`. Só `S-1-5-21-*` interessa: as demais chaves são
/// contas de serviço (`S-1-5-18/19/20`) e `.Default`, que não têm programas.
fn collect_user_hives(apps: &mut Vec<InstalledApp>, problems: &mut Vec<String>) -> usize {
    let mut scanned = 0usize;
    // `HKU` não tem subchave: abrir `""` depende do comportamento do Win32 e
    // falha em algumas builds. Usar o handle pré-definido é o caminho certo —
    // mas se algo der errado, isso precisa aparecer como leitura parcial.
    let users = match RegHandle::open(Hive::Users, "", RegistryView::Native64) {
        Ok(handle) => handle,
        Err(code) => {
            problems.push(format!(
                "HKU não pôde ser aberta (código {}); perfis podem faltar",
                code
            ));
            return scanned;
        }
    };
    let sids = match users.enum_keys() {
        Ok(sids) => sids,
        Err(_) => return scanned,
    };
    for sid in sids {
        if !sid.starts_with("S-1-5-21-") {
            continue;
        }
        let path = format!("{}\\{}", sid, UNINSTALL_SUBKEY);
        for view in [RegistryView::Native64, RegistryView::Native32] {
            let handle = match RegHandle::open(Hive::Users, &path, view) {
                Ok(handle) => handle,
                Err(_) => continue,
            };
            scanned += 1;
            let names = match handle.enum_keys() {
                Ok(names) => names,
                Err(code) => {
                    problems.push(format!("Perfil {} não pôde ser enumerado (código {})", sid, code));
                    continue;
                }
            };
            for name in names {
                if let Some(app) = read_app(Hive::Users, view, &format!("{}\\{}", path, name)) {
                    apps.push(app);
                }
            }
        }
    }
    scanned
}

fn read_app(hive: Hive, view: RegistryView, subkey: &str) -> Option<InstalledApp> {
    let handle = RegHandle::open(hive, subkey, view).ok()?;
    let display_name = handle.query_string("DisplayName")?;
    if display_name.trim().is_empty() {
        return None;
    }
    if is_hidden_by_shell(
        handle.query_u32("SystemComponent"),
        handle.query_string("ParentKeyName").as_deref(),
    ) {
        return None;
    }
    let uninstall_string = handle.query_string("UninstallString");
    Some(InstalledApp {
        id: build_app_id(hive, view, subkey),
        hive: hive.label().to_string(),
        view: view.label().to_string(),
        registry_path: format!("{}\\{}", hive.label(), subkey),
        display_name,
        publisher: handle.query_string("Publisher"),
        version: handle.query_string("DisplayVersion"),
        install_date: handle.query_string("InstallDate"),
        install_location: handle.query_string("InstallLocation"),
        estimated_size_kb: handle.query_u32("EstimatedSize").map(|kb| kb as u64),
        is_msi: detect_msi(uninstall_string.as_deref()),
        uninstall_string,
    })
}

/// A mesma chave aparece em `HKLM` e, quando o app é per-user em máquina com
/// UAC, também em `HKU\<perfil>`. Manter as duas invites o usuário a
/// desinstalar duas vezes o mesmo programa.
fn dedupe(apps: &mut Vec<InstalledApp>) {
    // Mesma chave visível em HKLM e no perfil do usuário é o mesmo programa;
    // versões x86/x64 legítimas têm chaves distintas e são preservadas porque a
    // visão (32/64) entra na chave de comparação.
    let mut seen: Vec<(String, String, String)> = Vec::new();
    apps.retain(|app| {
        if app.display_name.trim().is_empty() {
            return false;
        }
        let key = (
            app.display_name.to_lowercase(),
            app.view.to_string(),
            app.uninstall_string.clone().unwrap_or_default().to_lowercase(),
        );
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });
}

fn cached() -> Option<InstalledAppsSnapshot> {
    let guard = INVENTORY_CACHE.lock().ok()?;
    let entry = guard.as_ref()?;
    if entry.fetched_at.elapsed() < INVENTORY_CACHE_TTL {
        return Some(entry.snapshot.clone());
    }
    None
}

/// Inventário preferindo o cache. `other_install_dirs_excluding` roda várias
/// vezes por desinstalação; sem isso, seriam 3-4 varreduras completas do
/// registro.
pub fn collect_cached() -> InstalledAppsSnapshot {
    cached().unwrap_or_else(|| {
        let snapshot = collect();
        if let Ok(mut guard) = INVENTORY_CACHE.lock() {
            *guard = Some(InventoryCache {
                snapshot: snapshot.clone(),
                fetched_at: Instant::now(),
            });
        }
        snapshot
    })
}

pub fn invalidate_cache() {
    if let Ok(mut guard) = INVENTORY_CACHE.lock() {
        *guard = None;
    }
}

#[tauri::command]
pub async fn list_installed_apps(force_refresh: Option<bool>) -> InstalledAppsSnapshot {
    if !force_refresh.unwrap_or(false) {
        if let Some(snapshot) = cached() {
            return snapshot;
        }
    }
    let snapshot = match tauri::async_runtime::spawn_blocking(collect).await {
        Ok(snapshot) => snapshot,
        Err(_) => InstalledAppsSnapshot {
            apps: Vec::new(),
            total_count: 0,
            partial_reason: Some("A leitura dos instalados foi interrompida".to_string()),
            scanned_roots: 0,
            collected_at_unix_ms: now_unix_ms(),
        },
    };
    if let Ok(mut guard) = INVENTORY_CACHE.lock() {
        *guard = Some(InventoryCache {
            snapshot: snapshot.clone(),
            fetched_at: Instant::now(),
        });
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_round_trip() {
        let id = build_app_id(
            Hive::LocalMachine,
            RegistryView::Native32,
            "SOFTWARE\\X\\Uninstall\\Foto",
        );
        let (hive, view, path) = parse_app_id(&id).expect("id gerado precisa ser legível");
        assert_eq!(hive, Hive::LocalMachine);
        assert_eq!(view, RegistryView::Native32);
        assert_eq!(path, "SOFTWARE\\X\\Uninstall\\Foto");
    }

    #[test]
    fn app_id_invalida_nao_e_aceita() {
        assert!(parse_app_id("lixo").is_none());
        assert!(parse_app_id("HKLM|SOFTWARE\\X|16").is_none());
        assert!(parse_app_id("HKXX|SOFTWARE\\X|64").is_none());
        assert!(parse_app_id("HKLM||64").is_none());
    }

    #[test]
    fn componentes_do_sistema_e_subentradas_sao_ocultos() {
        assert!(is_hidden_by_shell(Some(1), None));
        assert!(is_hidden_by_shell(None, Some("{GUID}")));
        assert!(!is_hidden_by_shell(Some(0), None));
        assert!(!is_hidden_by_shell(None, None));
        assert!(!is_hidden_by_shell(None, Some("   ")));
    }

    #[test]
    fn msi_detectado_pelo_comando_do_instalador() {
        assert!(detect_msi(Some(
            "MsiExec.exe /X{1234-5678} /quiet"
        )));
        assert!(detect_msi(Some("C:\\Windows\\System32\\msiexec.exe /I{GUID}")));
        assert!(!detect_msi(Some("C:\\Program Files\\App\\unins000.exe")));
        assert!(!detect_msi(None));
    }

    #[test]
    fn inventario_real_respeita_invariantes() {
        let snapshot = collect();
        assert_eq!(snapshot.total_count, snapshot.apps.len());
        assert!(snapshot.scanned_roots >= 2, "as raízes principais precisam ser lidas");
        assert!(
            snapshot
                .apps
                .iter()
                .all(|app| !app.display_name.trim().is_empty()),
            "nenhum app pode ter nome vazio"
        );
        assert!(
            snapshot.apps.iter().all(|app| parse_app_id(&app.id).is_some()),
            "todo app precisa ter id de registro utilizável"
        );
    }

    #[test]
    fn inventario_real_nao_traz_componente_de_sistema() {
        let snapshot = collect();
        // Componentes como "Windows SDK" ou Atualizações do Windows têm
        // SystemComponent=1 e não devem aparecer.
        assert!(!snapshot
            .apps
            .iter()
            .any(|app| app.display_name.eq_ignore_ascii_case("Windows SDK")));
    }
}