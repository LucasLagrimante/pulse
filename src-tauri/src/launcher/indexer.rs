//! Indexador em memória, histórico de recentes (frecency), busca segura de
//! arquivos locais e ranqueamento por relevância para o Pulse Runner.

use super::calc;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_APP_RESULTS: usize = 16;
const MAX_FILE_RESULTS: usize = 10;
const MAX_RECENT_RESULTS: usize = 6;
const FILE_SEARCH_MAX_DEPTH: usize = 3;
const FILE_SEARCH_MAX_FILES: usize = 2500;
const FILE_SEARCH_DEADLINE: Duration = Duration::from_millis(180);
const YIELD_EVERY_FILES: usize = 15;

const USER_SCAN_FOLDERS: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Pictures",
    "Videos",
    "Music",
];

const IGNORED_DIRS: &[&str] = &[
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LauncherItem {
    pub id: String,
    /// `"calc" | "app" | "module" | "system" | "file" | "folder"`
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub target: String,
    /// `"apps" | "files" | "pulse" | "system" | "calc"`
    pub category: String,
    pub badge: String,
    pub icon_hint: String,
    /// Data URI PNG do ícone real do app. `None` = sem ícone confiável; o
    /// frontend cai para o glifo semântico de `icon_hint`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_label: Option<String>,
    pub is_recent: bool,
    pub score: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LauncherCalcResult {
    pub expression: String,
    pub formatted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherResponse {
    pub query: String,
    pub category: String,
    pub calc: Option<LauncherCalcResult>,
    pub recent: Vec<LauncherItem>,
    pub items: Vec<LauncherItem>,
    pub indexed_app_count: usize,
    pub is_indexing: bool,
    pub search_elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherActionResult {
    pub ok: bool,
    /// `"launched" | "navigated" | "copied" | "executed"`
    pub action: String,
    pub message: String,
    pub close_launcher: bool,
}

#[derive(Debug, Clone)]
pub struct IndexedApp {
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub target: String,
    pub aliases: Vec<String>,
    pub name_norm: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntry {
    id: String,
    kind: String,
    title: String,
    subtitle: String,
    target: String,
    category: String,
    badge: String,
    icon_hint: String,
    count: u32,
    last_used_unix_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LauncherHistory {
    #[serde(default)]
    entries: HashMap<String, HistoryEntry>,
}

struct BuiltinDef {
    id: &'static str,
    kind: &'static str,
    title: &'static str,
    subtitle: &'static str,
    target: &'static str,
    category: &'static str,
    badge: &'static str,
    icon_hint: &'static str,
    action_label: &'static str,
    aliases: &'static [&'static str],
}

static APP_INDEX: OnceLock<RwLock<Vec<IndexedApp>>> = OnceLock::new();
static INDEXING_ACTIVE: AtomicBool = AtomicBool::new(false);
static HISTORY_STORE: OnceLock<Mutex<LauncherHistory>> = OnceLock::new();

fn app_index() -> &'static RwLock<Vec<IndexedApp>> {
    APP_INDEX.get_or_init(|| RwLock::new(Vec::new()))
}

fn history_file_path() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join("launcher-history.json")
}

fn history_store() -> &'static Mutex<LauncherHistory> {
    HISTORY_STORE.get_or_init(|| Mutex::new(load_history_from_disk(&history_file_path())))
}

fn load_history_from_disk(path: &Path) -> LauncherHistory {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<LauncherHistory>(&raw).ok())
        .unwrap_or_default()
}

fn save_history_to_disk(path: &Path, history: &LauncherHistory) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, path).is_err() {
            let _ = fs::remove_file(path);
            let _ = fs::rename(&tmp, path);
        }
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Calcula o reforço de ranqueamento combinando frequência e recência (frecency).
fn frecency_boost(count: u32, last_used_unix_secs: u64, now_secs: u64) -> i32 {
    if count == 0 {
        return 0;
    }
    let age = now_secs.saturating_sub(last_used_unix_secs);
    let recency = if age < 60 * 60 {
        4.0
    } else if age < 24 * 60 * 60 {
        2.0
    } else if age < 7 * 24 * 60 * 60 {
        1.0
    } else {
        0.5
    };
    ((1.0 + f64::from(count)).ln() * 28.0 * recency).round() as i32
}

/// Registra o uso de um item no histórico persistente de recentes/frecency.
pub fn record_usage(id: &str, kind: &str, title: &str, target: &str) {
    // Nunca coloca ações destrutivas de energia na seção Recentes para evitar
    // disparos acidentais ao abrir o launcher e pressionar Enter.
    if target == "system:restart" || target == "system:shutdown" || kind == "calc" {
        return;
    }

    let (category, badge, icon_hint, subtitle) = metadata_for_history(id, kind, target);
    let now = now_unix_secs();

    let snapshot = {
        let mut store = history_store().lock().unwrap_or_else(|e| e.into_inner());
        let entry = store
            .entries
            .entry(id.to_owned())
            .or_insert_with(|| HistoryEntry {
                id: id.to_owned(),
                kind: kind.to_owned(),
                title: title.to_owned(),
                subtitle: subtitle.clone(),
                target: target.to_owned(),
                category: category.to_owned(),
                badge: badge.to_owned(),
                icon_hint: icon_hint.to_owned(),
                count: 0,
                last_used_unix_secs: now,
            });
        entry.title = title.to_owned();
        entry.target = target.to_owned();
        if !subtitle.is_empty() {
            entry.subtitle = subtitle;
        }
        entry.count = entry.count.saturating_add(1);
        entry.last_used_unix_secs = now;

        // Mantém o arquivo enxuto com os 120 itens mais recentes/usados.
        if store.entries.len() > 120 {
            let mut ranked: Vec<(String, i32)> = store
                .entries
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        frecency_boost(v.count, v.last_used_unix_secs, now),
                    )
                })
                .collect();
            ranked.sort_by_key(|a| std::cmp::Reverse(a.1));
            let keep: HashSet<String> = ranked.into_iter().take(100).map(|(k, _)| k).collect();
            store.entries.retain(|k, _| keep.contains(k));
        }

        store.clone()
    };

    let path = history_file_path();
    save_history_to_disk(&path, &snapshot);
}

fn metadata_for_history(
    id: &str,
    kind: &str,
    target: &str,
) -> (&'static str, &'static str, String, String) {
    if let Some(def) = pulse_modules()
        .iter()
        .chain(system_actions().iter())
        .find(|d| d.id == id || d.target == target)
    {
        return (
            def.category,
            def.badge,
            def.icon_hint.to_owned(),
            def.subtitle.to_owned(),
        );
    }

    match kind {
        "module" => (
            "pulse",
            "Pulse",
            "pulse:overview".to_owned(),
            "Módulo do Pulse".to_owned(),
        ),
        "system" => (
            "system",
            "Sistema",
            "system:settings".to_owned(),
            "Ação rápida do sistema".to_owned(),
        ),
        "folder" => (
            "files",
            "Pasta",
            "folder".to_owned(),
            shorten_path_hint(target),
        ),
        "file" => (
            "files",
            "Arquivo",
            "file".to_owned(),
            shorten_path_hint(target),
        ),
        _ => ("apps", "App", "app".to_owned(), shorten_path_hint(target)),
    }
}

fn pulse_modules() -> &'static [BuiltinDef] {
    &[
        BuiltinDef {
            id: "pulse:overview",
            kind: "module",
            title: "Visão geral",
            subtitle: "Painel principal de CPU, memória, disco e saúde do sistema",
            target: "Visão geral",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:overview",
            action_label: "Abrir módulo",
            aliases: &[
                "overview",
                "dashboard",
                "inicio",
                "home",
                "resumo",
                "painel",
            ],
        },
        BuiltinDef {
            id: "pulse:activity",
            kind: "module",
            title: "Atividade",
            subtitle: "Processos em execução, uso de CPU/RAM e encerramento seguro",
            target: "Atividade",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:activity",
            action_label: "Abrir módulo",
            aliases: &[
                "processos",
                "processes",
                "tarefas",
                "atividade",
                "ram",
                "cpu",
            ],
        },
        BuiltinDef {
            id: "pulse:performance",
            kind: "module",
            title: "Desempenho",
            subtitle: "Sensores de hardware, GPU, temperaturas e telemetria ao vivo",
            target: "Desempenho",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:performance",
            action_label: "Abrir módulo",
            aliases: &["performance", "gpu", "temperatura", "sensores", "hardware"],
        },
        BuiltinDef {
            id: "pulse:files",
            kind: "module",
            title: "Arquivos",
            subtitle: "Explorador rápido de pastas e arquivos recentes",
            target: "Arquivos",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:files",
            action_label: "Abrir módulo",
            aliases: &["explorer", "arquivos", "files", "pastas", "recentes"],
        },
        BuiltinDef {
            id: "pulse:storage",
            kind: "module",
            title: "Armazenamento",
            subtitle: "Análise visual de disco (TreeSize) e maiores diretórios",
            target: "Armazenamento",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:storage",
            action_label: "Abrir módulo",
            aliases: &["storage", "disco", "treesize", "espaco", "ssd", "hd"],
        },
        BuiltinDef {
            id: "pulse:cleanup",
            kind: "module",
            title: "Limpeza",
            subtitle: "Limpeza controlada de caches e arquivos temporários em Armazenamento",
            target: "Armazenamento",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:cleanup",
            action_label: "Abrir módulo",
            aliases: &[
                "cleanup",
                "limpeza",
                "limpar",
                "cache",
                "temp",
                "temporarios",
            ],
        },
        BuiltinDef {
            id: "pulse:services",
            kind: "module",
            title: "Serviços",
            subtitle: "Serviços do sistema e atualizações de aplicativos",
            target: "Serviços e atualizações",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:services",
            action_label: "Abrir módulo",
            aliases: &["servicos", "services", "winget", "atualizacoes", "updates"],
        },
        BuiltinDef {
            id: "pulse:uninstall",
            kind: "module",
            title: "Desinstalador",
            subtitle: "Remoção de programas com ponto de restauração e resíduos",
            target: "Desinstalar programas",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:uninstall",
            action_label: "Abrir módulo",
            aliases: &[
                "desinstalador",
                "desinstalar",
                "uninstall",
                "programas",
                "remover",
            ],
        },
        BuiltinDef {
            id: "pulse:quotas",
            kind: "module",
            title: "Cotas de IA",
            subtitle: "Monitor local de cotas e histórico de assinaturas de IA",
            target: "Cotas de IA",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:quotas",
            action_label: "Abrir módulo",
            aliases: &[
                "ia",
                "ai",
                "cotas",
                "assinaturas",
                "claude",
                "codex",
                "openrouter",
            ],
        },
        BuiltinDef {
            id: "pulse:startup",
            kind: "module",
            title: "Inicialização",
            subtitle: "Programas de inicialização, impacto no boot e segurança",
            target: "Inicialização e segurança",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:startup",
            action_label: "Abrir módulo",
            aliases: &["inicializacao", "startup", "boot", "autorun", "seguranca"],
        },
        BuiltinDef {
            id: "pulse:security",
            kind: "module",
            title: "Segurança",
            subtitle: "Auditoria determinística de acesso remoto, RDP, sessões e agentes",
            target: "Segurança",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:security",
            action_label: "Abrir módulo",
            aliases: &[
                "seguranca",
                "rdp",
                "acesso remoto",
                "remote desktop",
                "anydesk",
                "teamviewer",
                "rustdesk",
                "wslg",
                "qwinsta",
            ],
        },
        BuiltinDef {
            id: "pulse:network",
            kind: "module",
            title: "Rede e segurança",
            subtitle: "Conexões ativas, portas locais e postura de segurança",
            target: "Rede e segurança",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:network",
            action_label: "Abrir módulo",
            aliases: &["rede", "network", "conexoes", "portas", "firewall"],
        },
        BuiltinDef {
            id: "pulse:beta",
            kind: "module",
            title: "Recursos Beta",
            subtitle: "Pulse Shell, Runner, bateria de periféricos e laboratórios",
            target: "Recursos Beta",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:beta",
            action_label: "Abrir módulo",
            aliases: &[
                "beta",
                "recursos",
                "dock",
                "shell",
                "runner",
                "launcher",
                "perifericos",
            ],
        },
        BuiltinDef {
            id: "pulse:debug",
            kind: "module",
            title: "Debug",
            subtitle: "Trilha local de auditoria, eventos e diagnóstico técnico",
            target: "Debug",
            category: "pulse",
            badge: "Pulse",
            icon_hint: "pulse:debug",
            action_label: "Abrir módulo",
            aliases: &["debug", "logs", "auditoria", "diagnostico", "erros"],
        },
    ]
}

fn system_actions() -> &'static [BuiltinDef] {
    #[cfg(windows)]
    const SETTINGS_TITLE: &str = "Configurações do Windows";
    #[cfg(not(windows))]
    const SETTINGS_TITLE: &str = "Configurações do sistema";

    #[cfg(windows)]
    const TASKMGR_TITLE: &str = "Gerenciador de Tarefas";
    #[cfg(not(windows))]
    const TASKMGR_TITLE: &str = "Monitor do sistema";

    &[
        BuiltinDef {
            id: "sys:lock",
            kind: "system",
            title: "Bloquear tela",
            subtitle: "Bloqueia a sessão atual imediatamente",
            target: "system:lock",
            category: "system",
            badge: "Sistema",
            icon_hint: "system:lock",
            action_label: "Executar",
            aliases: &["lock", "bloquear", "trancar", "tela", "sessao"],
        },
        BuiltinDef {
            id: "sys:sleep",
            kind: "system",
            title: "Suspender",
            subtitle: "Coloca o computador em modo de suspensão",
            target: "system:sleep",
            category: "system",
            badge: "Sistema",
            icon_hint: "system:sleep",
            action_label: "Executar",
            aliases: &["sleep", "suspender", "dormir", "standby", "repouso"],
        },
        BuiltinDef {
            id: "sys:restart",
            kind: "system",
            title: "Reiniciar",
            subtitle: "Reinicia o computador com confirmação prévia",
            target: "system:restart",
            category: "system",
            badge: "Sistema",
            icon_hint: "system:restart",
            action_label: "Executar",
            aliases: &["restart", "reboot", "reiniciar"],
        },
        BuiltinDef {
            id: "sys:shutdown",
            kind: "system",
            title: "Desligar",
            subtitle: "Desliga o computador com confirmação prévia",
            target: "system:shutdown",
            category: "system",
            badge: "Sistema",
            icon_hint: "system:shutdown",
            action_label: "Executar",
            aliases: &["shutdown", "poweroff", "desligar", "encerrar"],
        },
        BuiltinDef {
            id: "sys:settings",
            kind: "system",
            title: SETTINGS_TITLE,
            subtitle: "Abre as configurações nativas do sistema operacional",
            target: "system:settings",
            category: "system",
            badge: "Sistema",
            icon_hint: "system:settings",
            action_label: "Abrir",
            aliases: &[
                "settings",
                "configuracoes",
                "windows",
                "painel",
                "preferencias",
            ],
        },
        BuiltinDef {
            id: "sys:terminal",
            kind: "system",
            title: "Terminal",
            subtitle: "Abre uma nova janela de terminal do sistema",
            target: "system:terminal",
            category: "system",
            badge: "Sistema",
            icon_hint: "system:terminal",
            action_label: "Abrir",
            aliases: &[
                "terminal",
                "wt",
                "cmd",
                "powershell",
                "prompt",
                "console",
                "shell",
            ],
        },
        BuiltinDef {
            id: "sys:taskmgr",
            kind: "system",
            title: TASKMGR_TITLE,
            subtitle: "Abre o monitor nativo de processos do sistema",
            target: "system:taskmgr",
            category: "system",
            badge: "Sistema",
            icon_hint: "system:taskmgr",
            action_label: "Abrir",
            aliases: &["taskmgr", "gerenciador", "tarefas", "processos", "monitor"],
        },
    ]
}

fn builtin_to_item(def: &BuiltinDef, score: i32) -> LauncherItem {
    LauncherItem {
        id: def.id.to_owned(),
        kind: def.kind.to_owned(),
        title: def.title.to_owned(),
        subtitle: def.subtitle.to_owned(),
        target: def.target.to_owned(),
        category: def.category.to_owned(),
        badge: def.badge.to_owned(),
        icon_hint: def.icon_hint.to_owned(),
        icon: None,
        action_label: Some(def.action_label.to_owned()),
        is_recent: false,
        score,
    }
}

/// Ícone nativo do alvo de um item do runner. Extração e cache ficam em
/// `app_icons`; aqui só decidimos a estratégia por formato de alvo.
fn icon_for_target(target: &str) -> Option<String> {
    // Apps modernos (AUMID) não têm arquivo que o SHGetFileInfo resolva;
    // nesses casos o frontend mantém o glifo semântico.
    if target.starts_with("shell:AppsFolder\\") {
        return None;
    }
    crate::app_icons::icon_for_path(Path::new(target))
        .or_else(|| crate::app_icons::icon_for_command(target))
}

/// Reconstrói o índice de aplicativos instalados em memória. Deve ser executado
/// dentro de `spawn_blocking` para não travar a interface.
pub fn rebuild_app_index() -> usize {
    if INDEXING_ACTIVE.swap(true, Ordering::AcqRel) {
        return app_index().read().map(|g| g.len()).unwrap_or(0);
    }

    let apps = scan_installed_apps();
    let count = apps.len();
    if let Ok(mut guard) = app_index().write() {
        *guard = apps;
    }
    INDEXING_ACTIVE.store(false, Ordering::Release);
    count
}

pub fn indexed_app_count() -> usize {
    app_index().read().map(|g| g.len()).unwrap_or(0)
}

pub fn is_indexing() -> bool {
    INDEXING_ACTIVE.load(Ordering::Acquire)
}

/// Executa a consulta completa no índice em memória + calculadora + busca rápida de arquivos.
pub fn query_sync(raw_query: &str, category_filter: Option<&str>) -> LauncherResponse {
    let started = Instant::now();
    let category = normalize_category(category_filter);
    let trimmed = raw_query.trim();
    let now = now_unix_secs();

    let history_snapshot = history_store()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();

    let apps_snapshot = app_index().read().map(|g| g.clone()).unwrap_or_default();
    let indexed_count = apps_snapshot.len();
    let indexing_now = is_indexing();

    // Consulta vazia: retorna Recentes + Acesso rápido do Pulse (ou itens da categoria ativa).
    if trimmed.is_empty() {
        let recent = build_recent_items(&history_snapshot, category, now);
        let items = build_empty_state_items(category, &apps_snapshot, &history_snapshot, now);
        return LauncherResponse {
            query: raw_query.to_owned(),
            category: category.to_owned(),
            calc: None,
            recent,
            items,
            indexed_app_count: indexed_count,
            is_indexing: indexing_now,
            search_elapsed_ms: started.elapsed().as_millis() as u64,
        };
    }

    // Prefixo explícito de busca em arquivos: `f:` ou `/`.
    let (file_only_mode, effective_query) = if let Some(rest) = trimmed
        .strip_prefix("f:")
        .or_else(|| trimmed.strip_prefix("F:"))
        .or_else(|| trimmed.strip_prefix('/'))
    {
        (true, rest.trim())
    } else {
        (false, trimmed)
    };

    // Calculadora instantânea (ativa em "all" e quando não está em modo exclusivo de arquivos).
    let calc_result = if !file_only_mode && category == "all" {
        calc::evaluate(trimmed).map(|value| LauncherCalcResult {
            expression: trimmed.to_owned(),
            formatted: calc::format(value),
        })
    } else {
        None
    };

    let query_norm = normalize_text(effective_query);
    let mut ranked: Vec<LauncherItem> = Vec::new();

    if !file_only_mode && !query_norm.is_empty() {
        // 1. Aplicativos instalados
        if category == "all" || category == "apps" {
            let mut app_hits: Vec<LauncherItem> = apps_snapshot
                .iter()
                .filter_map(|app| {
                    let base_score = score_match(&query_norm, &app.name_norm, &app.aliases)?;
                    let boost = history_snapshot
                        .entries
                        .get(&app.id)
                        .map(|h| frecency_boost(h.count, h.last_used_unix_secs, now))
                        .unwrap_or(0);
                    Some(LauncherItem {
                        id: app.id.clone(),
                        kind: "app".to_owned(),
                        title: app.name.clone(),
                        subtitle: app.subtitle.clone(),
                        target: app.target.clone(),
                        category: "apps".to_owned(),
                        badge: "App".to_owned(),
                        icon_hint: "app".to_owned(),
                        icon: icon_for_target(&app.target),
                        action_label: Some("Abrir".to_owned()),
                        is_recent: boost > 0,
                        score: base_score + boost,
                    })
                })
                .collect();

            app_hits.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| a.title.len().cmp(&b.title.len()))
                    .then_with(|| a.title.cmp(&b.title))
            });
            app_hits.truncate(MAX_APP_RESULTS);
            ranked.extend(app_hits);
        }

        // 2. Módulos do Pulse
        if category == "all" || category == "pulse" {
            for def in pulse_modules() {
                let title_norm = normalize_text(def.title);
                let aliases: Vec<String> = def.aliases.iter().map(|s| (*s).to_owned()).collect();
                if let Some(base_score) = score_match(&query_norm, &title_norm, &aliases) {
                    let boost = history_snapshot
                        .entries
                        .get(def.id)
                        .map(|h| frecency_boost(h.count, h.last_used_unix_secs, now))
                        .unwrap_or(0);
                    ranked.push(builtin_to_item(def, base_score + boost));
                }
            }
        }

        // 3. Ações rápidas do sistema
        if category == "all" || category == "system" {
            for def in system_actions() {
                let title_norm = normalize_text(def.title);
                let aliases: Vec<String> = def.aliases.iter().map(|s| (*s).to_owned()).collect();
                if let Some(base_score) = score_match(&query_norm, &title_norm, &aliases) {
                    let boost = history_snapshot
                        .entries
                        .get(def.id)
                        .map(|h| frecency_boost(h.count, h.last_used_unix_secs, now))
                        .unwrap_or(0);
                    ranked.push(builtin_to_item(def, base_score + boost));
                }
            }
        }
    }

    // 4. Busca segura de arquivos locais (MAX_DEPTH=3, deadline=180ms, yield_now a cada 15).
    let should_search_files = !effective_query.is_empty()
        && calc_result.is_none()
        && (file_only_mode
            || category == "files"
            || (category == "all" && effective_query.chars().count() >= 2));

    if should_search_files {
        let file_hits = search_local_files(&query_norm, &user_file_roots());
        ranked.extend(file_hits);
    }

    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.title.len().cmp(&b.title.len()))
            .then_with(|| a.title.cmp(&b.title))
    });
    ranked.truncate(24);

    LauncherResponse {
        query: raw_query.to_owned(),
        category: if file_only_mode {
            "files".to_owned()
        } else {
            category.to_owned()
        },
        calc: calc_result,
        recent: Vec::new(),
        items: ranked,
        indexed_app_count: indexed_count,
        is_indexing: indexing_now,
        search_elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

fn normalize_category(category: Option<&str>) -> &'static str {
    match category.map(|c| c.trim().to_ascii_lowercase()).as_deref() {
        Some("apps") => "apps",
        Some("files") => "files",
        Some("pulse") => "pulse",
        Some("system") => "system",
        _ => "all",
    }
}

fn build_recent_items(
    history: &LauncherHistory,
    category: &str,
    now_secs: u64,
) -> Vec<LauncherItem> {
    let mut recents: Vec<LauncherItem> = history
        .entries
        .values()
        .filter(|entry| category == "all" || entry.category == category)
        .map(|entry| {
            let boost = frecency_boost(entry.count, entry.last_used_unix_secs, now_secs);
            LauncherItem {
                id: entry.id.clone(),
                kind: entry.kind.clone(),
                title: entry.title.clone(),
                subtitle: entry.subtitle.clone(),
                target: entry.target.clone(),
                category: entry.category.clone(),
                badge: entry.badge.clone(),
                icon_hint: entry.icon_hint.clone(),
                icon: icon_for_target(&entry.target),
                action_label: Some(action_label_for_kind(&entry.kind).to_owned()),
                is_recent: true,
                score: boost,
            }
        })
        .collect();

    recents.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
    recents.truncate(MAX_RECENT_RESULTS);
    recents
}

fn build_empty_state_items(
    category: &str,
    apps: &[IndexedApp],
    history: &LauncherHistory,
    now_secs: u64,
) -> Vec<LauncherItem> {
    match category {
        "apps" => {
            let mut list: Vec<LauncherItem> = apps
                .iter()
                .map(|app| {
                    let boost = history
                        .entries
                        .get(&app.id)
                        .map(|h| frecency_boost(h.count, h.last_used_unix_secs, now_secs))
                        .unwrap_or(0);
                    LauncherItem {
                        id: app.id.clone(),
                        kind: "app".to_owned(),
                        title: app.name.clone(),
                        subtitle: app.subtitle.clone(),
                        target: app.target.clone(),
                        category: "apps".to_owned(),
                        badge: "App".to_owned(),
                        icon_hint: "app".to_owned(),
                        icon: icon_for_target(&app.target),
                        action_label: Some("Abrir".to_owned()),
                        is_recent: boost > 0,
                        score: boost,
                    }
                })
                .collect();
            list.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
            list.truncate(16);
            list
        }
        "pulse" => pulse_modules()
            .iter()
            .map(|def| builtin_to_item(def, 100))
            .collect(),
        "system" => system_actions()
            .iter()
            .map(|def| builtin_to_item(def, 100))
            .collect(),
        "files" => Vec::new(),
        _ => {
            // Na aba "Tudo" com busca vazia, exibe os principais módulos do Pulse
            // como acesso rápido logo abaixo da seção Recentes.
            const QUICK_PULSE_IDS: &[&str] = &[
                "pulse:overview",
                "pulse:activity",
                "pulse:performance",
                "pulse:storage",
                "pulse:files",
                "pulse:quotas",
            ];
            pulse_modules()
                .iter()
                .filter(|def| QUICK_PULSE_IDS.contains(&def.id))
                .map(|def| builtin_to_item(def, 100))
                .collect()
        }
    }
}

fn action_label_for_kind(kind: &str) -> &'static str {
    match kind {
        "module" => "Abrir módulo",
        "system" => "Executar",
        "calc" => "Copiar",
        _ => "Abrir",
    }
}

/// Normaliza texto para comparação insensível a maiúsculas e acentos em pt-BR.
pub fn normalize_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        let lower = ch.to_lowercase();
        for lc in lower {
            let mapped = match lc {
                'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'í' | 'ì' | 'î' | 'ï' => 'i',
                'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
                'ú' | 'ù' | 'û' | 'ü' => 'u',
                'ç' => 'c',
                'ñ' => 'n',
                other => other,
            };
            out.push(mapped);
        }
    }
    out
}

/// Pontua a relevância de um candidato (`title_norm` + `aliases`) para `query_norm`.
pub fn score_match(query_norm: &str, title_norm: &str, aliases: &[String]) -> Option<i32> {
    if query_norm.is_empty() {
        return None;
    }

    let mut best: Option<i32> = None;

    if title_norm == query_norm {
        best = Some(1000);
    } else if title_norm.starts_with(query_norm) {
        best = Some(720);
    } else if title_norm
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| !word.is_empty() && word.starts_with(query_norm))
    {
        best = Some(540);
    } else if title_norm.contains(query_norm) {
        best = Some(360);
    }

    // Múltiplas palavras na consulta: todas precisam aparecer no título ou nos aliases.
    let tokens: Vec<&str> = query_norm.split_whitespace().collect();
    if tokens.len() > 1 {
        let all_match = tokens
            .iter()
            .all(|tok| title_norm.contains(tok) || aliases.iter().any(|alias| alias.contains(tok)));
        if all_match {
            let token_score = 420 + (tokens.len() as i32 * 25);
            best = Some(best.map_or(token_score, |cur| cur.max(token_score)));
        }
    }

    // Correspondência por iniciais / subsequência limpa (ex.: "vsc" -> "visual studio code").
    if best.is_none() && query_norm.chars().count() >= 2 {
        if let Some(sub_score) = subsequence_score(query_norm, title_norm) {
            best = Some(sub_score);
        }
    }

    // Correspondência por alias literal (prefixo ou exato).
    for alias in aliases {
        let alias_norm = normalize_text(alias);
        let candidate = if alias_norm == query_norm {
            Some(880)
        } else if alias_norm.starts_with(query_norm) {
            Some(560)
        } else if query_norm.chars().count() >= 3 && alias_norm.contains(query_norm) {
            Some(300)
        } else {
            None
        };
        if let Some(score) = candidate {
            best = Some(best.map_or(score, |cur| cur.max(score)));
        }
    }

    best
}

fn subsequence_score(query: &str, text: &str) -> Option<i32> {
    let q_chars: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let t_chars: Vec<char> = text.chars().collect();
    if q_chars.is_empty() || q_chars.len() > t_chars.len() {
        return None;
    }

    let mut qi = 0usize;
    let mut word_start_hits = 0i32;
    let mut consecutive_hits = 0i32;
    let mut mid_word_jumps = 0usize;
    let mut prev_match: Option<usize> = None;

    for (ti, &tc) in t_chars.iter().enumerate() {
        if qi < q_chars.len() && tc == q_chars[qi] {
            let is_word_start = ti == 0 || !t_chars[ti - 1].is_alphanumeric();
            let is_consecutive = prev_match.is_some_and(|p| p + 1 == ti);
            if is_word_start {
                word_start_hits += 1;
            } else if !is_consecutive {
                mid_word_jumps += 1;
            }
            if is_consecutive {
                consecutive_hits += 1;
            }
            prev_match = Some(ti);
            qi += 1;
        }
    }

    if qi == q_chars.len() && mid_word_jumps <= 1 {
        Some(180 + word_start_hits * 35 + consecutive_hits * 18)
    } else {
        None
    }
}

fn user_file_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    if let Some(home) = crate::storage::paths::user_home() {
        for folder in USER_SCAN_FOLDERS {
            let candidate = home.join(folder);
            if candidate.is_dir() {
                let key = candidate.to_string_lossy().to_lowercase();
                if seen.insert(key) {
                    roots.push(candidate);
                }
            }
        }
    }

    for custom in crate::recent_files::load_custom_folders() {
        let candidate = PathBuf::from(custom);
        if candidate.is_dir() {
            let key = candidate.to_string_lossy().to_lowercase();
            if seen.insert(key) {
                roots.push(candidate);
            }
        }
    }

    roots
}

/// Busca segura em pastas do usuário respeitando os limites do `AGENTS.md`:
/// `MAX_DEPTH = 3`, limite de 2500 entradas, deadline de 180 ms, sem symlinks
/// e `std::thread::yield_now()` a cada 15 arquivos.
pub fn search_local_files(query_norm: &str, roots: &[PathBuf]) -> Vec<LauncherItem> {
    if query_norm.is_empty() || roots.is_empty() {
        return Vec::new();
    }

    let deadline = Instant::now() + FILE_SEARCH_DEADLINE;
    let mut visited_entries = 0usize;
    let mut hits: Vec<LauncherItem> = Vec::new();

    for root in roots {
        if Instant::now() >= deadline || visited_entries >= FILE_SEARCH_MAX_FILES {
            break;
        }
        walk_files(
            root,
            0,
            query_norm,
            deadline,
            &mut visited_entries,
            &mut hits,
        );
    }

    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.title.len().cmp(&b.title.len()))
            .then_with(|| a.title.cmp(&b.title))
    });
    hits.truncate(MAX_FILE_RESULTS);
    hits
}

fn walk_files(
    dir: &Path,
    depth: usize,
    query_norm: &str,
    deadline: Instant,
    visited_entries: &mut usize,
    hits: &mut Vec<LauncherItem>,
) {
    if depth > FILE_SEARCH_MAX_DEPTH
        || Instant::now() >= deadline
        || *visited_entries >= FILE_SEARCH_MAX_FILES
    {
        return;
    }

    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };

    for entry in read_dir.flatten() {
        if Instant::now() >= deadline || *visited_entries >= FILE_SEARCH_MAX_FILES {
            break;
        }

        *visited_entries += 1;
        if (*visited_entries).is_multiple_of(YIELD_EVERY_FILES) {
            std::thread::yield_now();
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }

        let is_dir = file_type.is_dir();
        if is_dir
            && IGNORED_DIRS
                .iter()
                .any(|ignored| ignored.eq_ignore_ascii_case(&name))
        {
            continue;
        }

        let name_norm = normalize_text(&name);
        if let Some(score) = score_match(query_norm, &name_norm, &[]) {
            let full_path = entry.path().to_string_lossy().into_owned();
            // Arquivos e pastas ficam ligeiramente abaixo de apps exatos no ranking misto.
            let adjusted = (score - 120).max(40);
            hits.push(LauncherItem {
                id: format!("file:{full_path}"),
                kind: if is_dir {
                    "folder".to_owned()
                } else {
                    "file".to_owned()
                },
                title: name,
                subtitle: shorten_path_hint(&full_path),
                target: full_path,
                category: "files".to_owned(),
                badge: if is_dir {
                    "Pasta".to_owned()
                } else {
                    "Arquivo".to_owned()
                },
                icon_hint: if is_dir {
                    "folder".to_owned()
                } else {
                    "file".to_owned()
                },
                icon: None,
                action_label: Some("Abrir".to_owned()),
                is_recent: false,
                score: adjusted,
            });
        }

        if is_dir && depth < FILE_SEARCH_MAX_DEPTH {
            walk_files(
                &entry.path(),
                depth + 1,
                query_norm,
                deadline,
                visited_entries,
                hits,
            );
        }
    }
}

fn shorten_path_hint(path: &str) -> String {
    let trimmed = path.trim();
    if let Some(rest) = trimmed.strip_prefix("shell:AppsFolder\\") {
        return format!("Aplicativo do Windows · {rest}");
    }
    if let Some(home) = crate::storage::paths::user_home() {
        let home_str = home.to_string_lossy();
        if !home_str.is_empty() && trimmed.starts_with(home_str.as_ref()) {
            let suffix = &trimmed[home_str.len()..];
            return format!("~{suffix}");
        }
    }
    trimmed.to_owned()
}

fn is_ignored_app_name(name: &str) -> bool {
    let lower = normalize_text(name);
    if lower.is_empty() {
        return true;
    }
    const BLOCKED_PREFIXES: &[&str] = &[
        "uninstall",
        "desinstalar",
        "remove ",
        "readme",
        "leia-me",
        "license",
        "licenca",
        "help",
        "manual",
        "website",
        "documentation",
        "release notes",
    ];
    BLOCKED_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn scan_installed_apps() -> Vec<IndexedApp> {
    #[cfg(windows)]
    {
        windows_scan::scan_apps()
    }
    #[cfg(target_os = "linux")]
    {
        linux_scan::scan_apps()
    }
    #[cfg(target_os = "macos")]
    {
        macos_scan::scan_apps()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
mod windows_scan {
    use super::*;
    use std::env;
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, SHGetKnownFolderItem,
        KF_FLAG_DEFAULT, SIGDN, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
        SIGDN_PARENTRELATIVEPARSING,
    };

    const START_MENU_MAX_DEPTH: usize = 3;

    pub fn scan_apps() -> Vec<IndexedApp> {
        let mut apps = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();
        let mut seen_targets: HashSet<String> = HashSet::new();
        let mut processed = 0usize;

        // 1. Menu Iniciar (sistema + usuário)
        for root in start_menu_roots() {
            walk_start_menu(
                &root,
                0,
                &mut apps,
                &mut seen_names,
                &mut seen_targets,
                &mut processed,
            );
        }

        // 2. App Paths do Registro (HKLM e HKCU)
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            scan_app_paths_registry(
                hive,
                &mut apps,
                &mut seen_names,
                &mut seen_targets,
                &mut processed,
            );
        }

        // 3. Apps modernos e UWP via shell:AppsFolder
        scan_apps_folder(
            &mut apps,
            &mut seen_names,
            &mut seen_targets,
            &mut processed,
        );

        apps.sort_by(|a, b| a.name_norm.cmp(&b.name_norm));
        apps
    }

    fn start_menu_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Some(program_data) = env::var_os("ProgramData") {
            let p = PathBuf::from(program_data)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            if p.is_dir() {
                roots.push(p);
            }
        }
        if let Some(appdata) = env::var_os("APPDATA") {
            let p = PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            if p.is_dir() {
                roots.push(p);
            }
        }
        roots
    }

    fn walk_start_menu(
        dir: &Path,
        depth: usize,
        apps: &mut Vec<IndexedApp>,
        seen_names: &mut HashSet<String>,
        seen_targets: &mut HashSet<String>,
        processed: &mut usize,
    ) {
        if depth > START_MENU_MAX_DEPTH {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            *processed += 1;
            if (*processed).is_multiple_of(YIELD_EVERY_FILES) {
                std::thread::yield_now();
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_dir() {
                walk_start_menu(&path, depth + 1, apps, seen_names, seen_targets, processed);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("lnk") | Some("url")) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let name = stem.trim().to_owned();
            if is_ignored_app_name(&name) {
                continue;
            }
            let name_norm = normalize_text(&name);
            let target = path.to_string_lossy().into_owned();
            let target_key = target.to_lowercase();
            if !seen_names.insert(name_norm.clone()) || !seen_targets.insert(target_key) {
                continue;
            }
            let aliases = derive_aliases_from_name(&name, None);
            apps.push(IndexedApp {
                id: format!("app:{target}"),
                name,
                subtitle: shorten_path_hint(&target),
                target,
                aliases,
                name_norm,
            });
        }
    }

    fn scan_app_paths_registry(
        hive: HKEY,
        apps: &mut Vec<IndexedApp>,
        seen_names: &mut HashSet<String>,
        seen_targets: &mut HashSet<String>,
        processed: &mut usize,
    ) {
        let subkey = HSTRING::from("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths");
        let mut root_key = HKEY::default();
        if unsafe { RegOpenKeyExW(hive, &subkey, None, KEY_READ, &mut root_key) }.0 != 0 {
            return;
        }

        let mut index = 0u32;
        let mut name_buf = [0u16; 260];
        loop {
            *processed += 1;
            if (*processed).is_multiple_of(YIELD_EVERY_FILES) {
                std::thread::yield_now();
            }

            let mut name_len = name_buf.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(
                    root_key,
                    index,
                    Some(PWSTR(name_buf.as_mut_ptr())),
                    &mut name_len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status.0 != 0 {
                break;
            }
            index += 1;

            let key_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let Some(exe_path) = read_default_reg_string(root_key, &key_name) else {
                continue;
            };
            let cleaned = exe_path.trim().trim_matches('"').to_owned();
            if cleaned.is_empty() || !Path::new(&cleaned).is_file() {
                continue;
            }

            let stem = key_name
                .strip_suffix(".exe")
                .or_else(|| key_name.strip_suffix(".EXE"))
                .unwrap_or(&key_name)
                .trim();
            if stem.is_empty() || is_ignored_app_name(stem) {
                continue;
            }

            let title = crate::process_identity::choose_display_name(
                stem,
                crate::process_identity::official_name_for_path(&cleaned).as_deref(),
                Some(&humanize_exe_stem(stem)),
            );
            let name_norm = normalize_text(&title);
            let target_key = cleaned.to_lowercase();
            if !seen_targets.insert(target_key) || !seen_names.insert(name_norm.clone()) {
                continue;
            }

            let aliases = derive_aliases_from_name(&title, Some(stem));
            apps.push(IndexedApp {
                id: format!("app:{cleaned}"),
                name: title,
                subtitle: shorten_path_hint(&cleaned),
                target: cleaned,
                aliases,
                name_norm,
            });
        }

        let _ = unsafe { RegCloseKey(root_key) };
    }

    fn read_default_reg_string(parent: HKEY, subkey: &str) -> Option<String> {
        let sub = HSTRING::from(subkey);
        let mut bytes: u32 = 0;
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        if unsafe { RegGetValueW(parent, &sub, None, flags, None, None, Some(&mut bytes)) }.0 != 0
            || bytes < 2
        {
            return None;
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
        if unsafe {
            RegGetValueW(
                parent,
                &sub,
                None,
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        }
        .0 != 0
        {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let text = String::from_utf16_lossy(&buf[..len]);
        (!text.trim().is_empty()).then_some(text)
    }

    fn scan_apps_folder(
        apps: &mut Vec<IndexedApp>,
        seen_names: &mut HashSet<String>,
        seen_targets: &mut HashSet<String>,
        processed: &mut usize,
    ) {
        let com_init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let _ = (|| -> windows::core::Result<()> {
            let folder: IShellItem =
                unsafe { SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)? };
            let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems)? };
            loop {
                *processed += 1;
                if (*processed).is_multiple_of(YIELD_EVERY_FILES) {
                    std::thread::yield_now();
                }

                let mut slot = [None];
                let mut fetched = 0u32;
                if unsafe { items.Next(&mut slot, Some(&mut fetched)) }.is_err() || fetched == 0 {
                    break;
                }
                let Some(item) = slot[0].take() else {
                    break;
                };
                let Some(name) = display_name(&item, SIGDN_NORMALDISPLAY) else {
                    continue;
                };
                let Some(parsing_id) = display_name(&item, SIGDN_PARENTRELATIVEPARSING) else {
                    continue;
                };
                if is_ignored_app_name(&name) || !is_launchable_shell_id(&parsing_id) {
                    continue;
                }

                let name_norm = normalize_text(&name);
                let fs_path = display_name(&item, SIGDN_FILESYSPATH);
                let target = fs_path
                    .clone()
                    .unwrap_or_else(|| format!("shell:AppsFolder\\{parsing_id}"));
                let target_key = target.to_lowercase();

                if !seen_names.insert(name_norm.clone()) || !seen_targets.insert(target_key) {
                    continue;
                }

                let mut aliases = derive_aliases_from_name(&name, None);
                add_inbox_aliases(&parsing_id, &mut aliases);

                apps.push(IndexedApp {
                    id: format!("app:{parsing_id}"),
                    name,
                    subtitle: shorten_path_hint(&target),
                    target,
                    aliases,
                    name_norm,
                });
            }
            Ok(())
        })();

        if com_init {
            unsafe { CoUninitialize() };
        }
    }

    fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
        unsafe {
            let ptr = item.GetDisplayName(kind).ok()?;
            let text = ptr.to_string().ok();
            CoTaskMemFree(Some(ptr.0 as _));
            text.filter(|s| !s.trim().is_empty())
        }
    }

    fn is_launchable_shell_id(id: &str) -> bool {
        const SKIP_EXTENSIONS: &[&str] = &[
            ".txt", ".chm", ".pdf", ".htm", ".html", ".rtf", ".ini", ".log", ".md", ".xml",
        ];
        let lower = id.to_ascii_lowercase();
        !lower.starts_with("http:")
            && !lower.starts_with("https:")
            && !SKIP_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
    }

    fn add_inbox_aliases(parsing_id: &str, aliases: &mut Vec<String>) {
        const INBOX: &[(&str, &str)] = &[
            ("Microsoft.WindowsCalculator_8wekyb3d8bbwe", "calc"),
            ("Microsoft.WindowsTerminal_8wekyb3d8bbwe", "wt"),
            ("Microsoft.Paint_8wekyb3d8bbwe", "mspaint"),
            ("Microsoft.WindowsNotepad_8wekyb3d8bbwe", "notepad"),
        ];
        if let Some((family, _)) = parsing_id.split_once('!') {
            for &(pkg, alias) in INBOX {
                if family.eq_ignore_ascii_case(pkg) && !aliases.iter().any(|a| a == alias) {
                    aliases.push(alias.to_owned());
                }
            }
        }
    }

    fn humanize_exe_stem(stem: &str) -> String {
        let mut chars = stem.chars();
        match chars.next() {
            Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
            None => String::new(),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_scan {
    use super::*;

    pub fn scan_apps() -> Vec<IndexedApp> {
        let mut dirs = vec![
            PathBuf::from("/usr/share/applications"),
            PathBuf::from("/usr/local/share/applications"),
            PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        ];
        if let Some(home) = crate::storage::paths::user_home() {
            dirs.push(home.join(".local/share/applications"));
            dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
        }

        let mut apps = Vec::new();
        let mut seen_names = HashSet::new();
        let mut processed = 0usize;

        for dir in dirs {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                processed += 1;
                if processed % YIELD_EVERY_FILES == 0 {
                    std::thread::yield_now();
                }
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&path) else {
                    continue;
                };
                if let Some(app) = parse_desktop_entry(&path, &content) {
                    if seen_names.insert(app.name_norm.clone()) {
                        apps.push(app);
                    }
                }
            }
        }

        apps.sort_by(|a, b| a.name_norm.cmp(&b.name_norm));
        apps
    }

    pub(super) fn parse_desktop_entry(path: &Path, content: &str) -> Option<IndexedApp> {
        let mut in_main = false;
        let mut name: Option<String> = None;
        let mut exec: Option<String> = None;
        let mut comment: Option<String> = None;
        let mut keywords: Vec<String> = Vec::new();
        let mut entry_type: Option<String> = None;
        let mut hidden = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_main = trimmed == "[Desktop Entry]";
                continue;
            }
            if !in_main {
                continue;
            }
            let Some((key, val)) = trimmed.split_once('=') else {
                continue;
            };
            let val = val.trim();
            match key.trim() {
                "Type" => entry_type = Some(val.to_owned()),
                "Name" if name.is_none() => name = Some(val.to_owned()),
                "Exec" if exec.is_none() => exec = Some(val.to_owned()),
                "Comment" if comment.is_none() => comment = Some(val.to_owned()),
                "Keywords" => {
                    keywords.extend(
                        val.split(';')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(normalize_text),
                    );
                }
                "NoDisplay" | "Hidden" if val.eq_ignore_ascii_case("true") => hidden = true,
                _ => {}
            }
        }

        if hidden || entry_type.as_deref() != Some("Application") {
            return None;
        }
        let name = name.filter(|n| !is_ignored_app_name(n))?;
        let exec_raw = exec?;
        let cleaned_exec = clean_desktop_exec(&exec_raw);
        if cleaned_exec.is_empty() {
            return None;
        }

        let name_norm = normalize_text(&name);
        let mut aliases = derive_aliases_from_name(&name, cleaned_exec.split_whitespace().next());
        aliases.extend(keywords);

        Some(IndexedApp {
            id: format!("app:{}", path.to_string_lossy()),
            name,
            subtitle: comment.unwrap_or_else(|| cleaned_exec.clone()),
            target: cleaned_exec,
            aliases,
            name_norm,
        })
    }

    pub(super) fn clean_desktop_exec(raw: &str) -> String {
        raw.split_whitespace()
            .filter(|tok| !tok.starts_with('%'))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(target_os = "macos")]
mod macos_scan {
    use super::*;

    pub fn scan_apps() -> Vec<IndexedApp> {
        let mut roots = vec![
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
        ];
        if let Some(home) = crate::storage::paths::user_home() {
            roots.push(home.join("Applications"));
        }

        let mut apps = Vec::new();
        let mut seen = HashSet::new();
        let mut processed = 0usize;

        for root in roots {
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                processed += 1;
                if processed % YIELD_EVERY_FILES == 0 {
                    std::thread::yield_now();
                }
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("app") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if is_ignored_app_name(stem) {
                    continue;
                }
                let name = stem.to_owned();
                let name_norm = normalize_text(&name);
                if !seen.insert(name_norm.clone()) {
                    continue;
                }
                let target = path.to_string_lossy().into_owned();
                let aliases = derive_aliases_from_name(&name, None);
                apps.push(IndexedApp {
                    id: format!("app:{target}"),
                    name,
                    subtitle: shorten_path_hint(&target),
                    target,
                    aliases,
                    name_norm,
                });
            }
        }

        apps.sort_by(|a, b| a.name_norm.cmp(&b.name_norm));
        apps
    }
}

fn derive_aliases_from_name(name: &str, exe_hint: Option<&str>) -> Vec<String> {
    let mut aliases = Vec::new();
    if let Some(exe) = exe_hint {
        let trimmed = exe.trim().to_ascii_lowercase();
        if !trimmed.is_empty() {
            aliases.push(trimmed);
        }
    }
    let words: Vec<String> = normalize_text(name)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    if words.len() >= 2 {
        let acronym: String = words.iter().filter_map(|w| w.chars().next()).collect();
        if acronym.len() >= 2 && !aliases.contains(&acronym) {
            aliases.push(acronym);
        }
    }
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_portuguese_accents_and_ranks_matches() {
        assert_eq!(
            normalize_text("Configurações do Windows"),
            "configuracoes do windows"
        );
        assert_eq!(normalize_text("Visão geral"), "visao geral");

        let exact = score_match("visao geral", "visao geral", &[]).unwrap();
        let prefix = score_match("visao", "visao geral", &[]).unwrap();
        let acronym = score_match("vsc", "visual studio code", &[]).unwrap();
        assert!(exact > prefix);
        assert!(prefix > acronym);
        assert!(score_match("xyz", "visual studio code", &[]).is_none());
    }

    #[test]
    fn frecency_boost_prefers_recent_and_frequent_usage() {
        let now = 1_000_000;
        let recent_hot = frecency_boost(5, now - 120, now);
        let old_hot = frecency_boost(5, now - 10 * 86_400, now);
        let recent_once = frecency_boost(1, now - 120, now);
        assert!(recent_hot > old_hot);
        assert!(recent_hot > recent_once);
        assert_eq!(frecency_boost(0, now, now), 0);
    }

    #[test]
    fn query_sync_returns_calculator_and_pulse_modules() {
        let calc_resp = query_sync("1280 * 0.85", Some("all"));
        let calc = calc_resp.calc.expect("esperava resultado de calculadora");
        assert_eq!(calc.formatted, "1088");

        let pulse_resp = query_sync("cotas", Some("pulse"));
        assert!(pulse_resp
            .items
            .iter()
            .any(|item| item.target == "Cotas de IA"));

        let sys_resp = query_sync("bloquear", Some("system"));
        assert!(sys_resp
            .items
            .iter()
            .any(|item| item.target == "system:lock"));
    }

    #[test]
    fn local_file_search_respects_depth_and_finds_matching_files() {
        let base = std::env::temp_dir().join(format!("pulse-runner-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let nested = base.join("projetos").join("relatorios");
        let deep = nested.join("nivel3").join("nivel4").join("muito-fundo");
        let ignored = base.join("node_modules");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir_all(&ignored).unwrap();

        fs::write(nested.join("orcamento-2026.pdf"), b"ok").unwrap();
        fs::write(deep.join("orcamento-oculto.pdf"), b"ok").unwrap();
        fs::write(ignored.join("orcamento-ignored.pdf"), b"ok").unwrap();

        let hits = search_local_files("orcamento", std::slice::from_ref(&base));
        assert!(hits.iter().any(|h| h.title == "orcamento-2026.pdf"));
        assert!(!hits.iter().any(|h| h.title == "orcamento-oculto.pdf"));
        assert!(!hits.iter().any(|h| h.title == "orcamento-ignored.pdf"));

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_linux_desktop_entries_safely() {
        let content = r#"
[Desktop Entry]
Type=Application
Name=Visual Studio Code
Comment=Code Editing. Redefined.
Exec=/usr/share/code/code %F
Keywords=vscode;editor;
"#;
        let parsed = linux_scan::parse_desktop_entry(
            Path::new("/usr/share/applications/code.desktop"),
            content,
        )
        .expect("deve parsear entrada .desktop válida");
        assert_eq!(parsed.name, "Visual Studio Code");
        assert_eq!(parsed.target, "/usr/share/code/code");
        assert!(parsed.aliases.iter().any(|a| a == "vscode"));

        let hidden = "[Desktop Entry]\nType=Application\nName=Hidden\nExec=foo\nNoDisplay=true\n";
        assert!(linux_scan::parse_desktop_entry(Path::new("/tmp/h.desktop"), hidden).is_none());
    }
}
