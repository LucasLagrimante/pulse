//! Sessão de desinstalação: da seleção do app à limpeza de resíduos.
//!
//! ## Whitelist como barreira de segurança
//!
//! O front-end envia **identificadores de itens que o próprio backend ofereceu**
//! e nunca caminhos livres. `cleanup` recusa qualquer id fora da sessão. Um
//! `invoke` forjado a partir do console do WebView2 não consegue transformar o
//! Pulse em um `reg delete` ou `rm -rf` genérico.
//!
//! ## Verificação de snapshot
//!
//! Entre a varredura e a limpeza o uninstaller pode ter criado arquivos (log de
//! desinstalação, cache). `matches_snapshot` compara o conjunto de filhos com o
//! visto na varredura e recusa a remoção quando algo **novo** apareceu: é
//! exatamente o cenário em que apagar por engano destruiria dado novo.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::installed::{self, parse_app_id, InstalledApp};
use super::platform::{self, RestorePointOutcome};
use super::registry::RegHandle;
use super::residue::{self, SnapshotEntry, UninstallTargets};

/// Teto de espera do desinstalador. Passado isso, o Pulse **não** mata o
/// processo — matar um instalador no meio deixa o sistema inconsistente.
const UNINSTALLER_TIMEOUT: Duration = Duration::from_secs(900);
/// Espera extra pela removal da chave de desinstalação: muitos instaladores
/// terminam o processo antes de limpar o registro.
const REGISTRY_SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Session {
    token: String,
    app_id: String,
    targets: UninstallTargets,
    registry_snapshot: Vec<SnapshotEntry>,
    folder_snapshot: Vec<SnapshotEntry>,
    created_at: Instant,
    restore_point: Option<RestorePointOutcome>,
}

static SESSIONS: Mutex<Vec<Session>> = Mutex::new(Vec::new());

/// Sessões são de uso único: uma desinstalação não pode vazar para a próxima.
const MAX_SESSIONS: usize = 8;

/// Identificador de sessão único por chamada: dois apps abertos na mesma
/// milissegada não podem colidir.
fn new_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}-{}",
        now_unix_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        std::process::id()
    )
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub fn find_app(app_id: &str) -> Option<InstalledApp> {
    installed::collect_cached()
        .apps
        .into_iter()
        .find(|app| app.id == app_id)
}

fn register_session(session: Session) {
    if let Ok(mut guard) = SESSIONS.lock() {
        // Expiração por tempo: um app reinstalado entre a varredura e a limpeza
        // precisa gerar um snapshot novo, não reaproveitar o antigo.
        guard.retain(|item| item.created_at.elapsed() < Duration::from_secs(1800));
        guard.push(session);
        let overflow = guard.len().saturating_sub(MAX_SESSIONS);
        guard.drain(0..overflow);
    }
}

fn take_session(token: &str) -> Option<Session> {
    let mut guard = SESSIONS.lock().ok()?;
    let position = guard.iter().position(|item| item.token == token)?;
    Some(guard.remove(position))
}

fn touch_session(session: &Session) {
    if let Ok(mut guard) = SESSIONS.lock() {
        guard.retain(|item| item.created_at.elapsed() < Duration::from_secs(1800));
        guard.push(Session {
            token: session.token.clone(),
            app_id: session.app_id.clone(),
            targets: session.targets.clone(),
            registry_snapshot: session.registry_snapshot.clone(),
            folder_snapshot: session.folder_snapshot.clone(),
            created_at: Instant::now(),
            restore_point: session.restore_point.clone(),
        });
        let overflow = guard.len().saturating_sub(MAX_SESSIONS);
        guard.drain(0..overflow);
    }
}

fn take_snapshot(targets: &UninstallTargets) -> (Vec<SnapshotEntry>, Vec<SnapshotEntry>) {
    let registry = residue::snapshot_registry(&targets.registry_keys);
    let folders = targets
        .folders
        .iter()
        .map(|folder| residue::snapshot_folder(Path::new(&folder.path)))
        .collect();
    (registry, folders)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallSession {
    pub token: String,
    pub targets: UninstallTargets,
    pub elevated: bool,
    pub restore_point_supported: bool,
    pub restore_point_unavailable_reason: Option<String>,
    pub started_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallerOutcome {
    pub started: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// `true` quando o próprio Windows deixou de listar o app — prova de que o
    /// desinstalador terminou de verdade.
    pub entry_removed: bool,
    pub detection_note: Option<String>,
    pub messages: Vec<String>,
    pub restore_point: Option<RestorePointOutcome>,
    pub targets: UninstallTargets,
    pub finished_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupItemResult {
    pub id: String,
    pub path: String,
    pub removed: bool,
    pub freed_bytes: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub registry_selected: usize,
    pub registry_removed: usize,
    pub folder_selected: usize,
    pub folder_removed: usize,
    pub freed_bytes: u64,
    pub items: Vec<CleanupItemResult>,
    pub finished_at_unix_ms: u64,
}

/// Divide uma linha de comando do Windows em programa + argumentos.
///
/// Implementado aqui, e não com `CommandLineToArgvW`, por dois motivos: é
/// lógica pura (testável) e não exige gerenciar a memória que a API aloca.
/// Regras: aspas duplas agrupam, `\"` e `\\` são escape dentro de aspas.
pub fn split_command_line(line: &str) -> Option<(String, Vec<String>)> {
    let characters: Vec<char> = line.trim().chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut index = 0usize;
    let mut produced = false;

    while index < characters.len() {
        let character = characters[index];
        if in_quotes {
            match character {
                '\\' => {
                    let mut backslashes = 0usize;
                    while index < characters.len() && characters[index] == '\\' {
                        backslashes += 1;
                        index += 1;
                    }
                    if index < characters.len() && characters[index] == '"' {
                        for _ in 0..(backslashes / 2) {
                            current.push('\\');
                        }
                        if backslashes.is_multiple_of(2) {
                            in_quotes = false;
                            index += 1;
                        } else {
                            current.push('"');
                            index += 1;
                        }
                    } else {
                        for _ in 0..backslashes {
                            current.push('\\');
                        }
                    }
                    continue;
                }
                '"' => {
                    in_quotes = false;
                    index += 1;
                    produced = true;
                    continue;
                }
                _ => {
                    current.push(character);
                    index += 1;
                    continue;
                }
            }
        }
        match character {
            '"' => {
                in_quotes = true;
                produced = true;
                index += 1;
            }
            character if character.is_whitespace() => {
                if produced {
                    parts.push(std::mem::take(&mut current));
                    produced = false;
                }
                index += 1;
            }
            _ => {
                current.push(character);
                produced = true;
                index += 1;
            }
        }
    }
    if in_quotes {
        return None;
    }
    if produced {
        parts.push(current);
    }
    if parts.is_empty() {
        return None;
    }
    let program = parts.remove(0);
    if program.trim().is_empty() {
        return None;
    }
    Some((program, parts))
}

/// Códigos NTSTATUS: são `u32` com o bit alto ligado, então só compilam como
/// `i32` via cast explícito — e `cast` não é válido em padrão de `match`.
const STATUS_DLL_NOT_FOUND: i32 = 0xC0000135u32 as i32;
const STATUS_ENTRYPOINT_NOT_FOUND: i32 = 0xC0000139u32 as i32;
const STATUS_IMAGE_MACHINE_MISMATCH: i32 = 0xC000007Bu32 as i32;

/// Códigos de saída do Windows Installer e do Windows em geral. Traduzir
/// aqui evita que a UI mostre "erro 1603" para o usuário.
pub fn describe_exit_code(exit_code: i32) -> Vec<String> {
    match exit_code {
        0 => vec!["O desinstalador terminou com sucesso".to_string()],
        1603 | 1618 | 1619 => vec![
            format!("O Windows Installer falhou (código {})", exit_code),
            "O fabricante pode exigir a versão anterior do produto instalada ou o cancelamento do reinício pendente.".to_string(),
        ],
        3010 => vec![
            "O produto foi removido, mas o Windows precisa reiniciar para concluir".to_string(),
        ],
        1602 => vec!["O usuário cancelou a desinstalação".to_string()],
        1605 => vec!["O produto não está instalado segundo o Windows Installer".to_string()],
        STATUS_DLL_NOT_FOUND => {
            vec!["Falta um componente do Windows para executar o desinstalador".to_string()]
        }
        STATUS_ENTRYPOINT_NOT_FOUND | STATUS_IMAGE_MACHINE_MISMATCH => vec![
            "O desinstalador é de uma arquitetura diferente da deste computador".to_string(),
        ],
        other => vec![format!("O desinstalador terminou com código {}", other)],
    }
}

#[tauri::command]
pub async fn start_uninstall_session(app_id: String) -> Result<UninstallSession, String> {
    // Uma única enumeração do registro: procurar o app e varrer os resíduos são
    // o mesmo trabalho. Três varreduras completas por sessão travariam a UI.
    let scan_id = app_id.clone();
    let scanned = tauri::async_runtime::spawn_blocking(move || {
        find_app(&scan_id).map(|app| residue::scan(&app))
    })
    .await
    .map_err(|_| "A análise de resíduos foi interrompida".to_string())?;
    let targets = scanned.ok_or_else(|| {
        "Este programa não está mais registrado. Atualize a lista.".to_string()
    })?;

    // O snapshot percorre até 20 mil itens por pasta: em thread de I/O, nunca
    // no runtime do Tauri (AGENTS.md §3.1).
    let snapshot_targets = targets.clone();
    let (registry_snapshot, folder_snapshot) = tauri::async_runtime::spawn_blocking(move || {
        take_snapshot(&snapshot_targets)
    })
    .await
    .unwrap_or((Vec::new(), Vec::new()));
    let capabilities = platform::capabilities();
    let app_id = targets.app_id.clone();
    let token = new_token();

    let session = UninstallSession {
        token: token.clone(),
        targets,
        elevated: capabilities.elevated,
        restore_point_supported: capabilities.restore_point_supported,
        restore_point_unavailable_reason: capabilities.restore_point_unavailable_reason,
        started_at_unix_ms: now_unix_ms(),
    };

    register_session(Session {
        token,
        app_id,
        targets: session.targets.clone(),
        registry_snapshot,
        folder_snapshot,
        created_at: Instant::now(),
        restore_point: None,
    });

    Ok(session)
}

/// Etapas 1 e 2 do modal: grava o ponto de restauração (se pedido) e roda o
/// desinstalador padrão do fabricante, aguardando o fim de verdade.
#[tauri::command]
pub async fn run_app_uninstaller(
    session_token: String,
    create_restore_point: Option<bool>,
) -> Result<UninstallerOutcome, String> {
    let mut session = take_session(&session_token)
        .ok_or_else(|| "Sessão de desinstalação expirou. Abra a lista novamente.".to_string())?;

    let wants_restore_point = create_restore_point.unwrap_or(false);
    let restore_point = if wants_restore_point {
        let description = format!(
            "Pulse antes de desinstalar {}",
            session.targets.display_name
        );
        Some(
            tauri::async_runtime::spawn_blocking(move || platform::create_restore_point(&description))
                .await
                .map_err(|_| "A criação do ponto de restauração foi interrompida".to_string())?,
        )
    } else {
        None
    };

    let Some(command_line) = session.targets.uninstall_string.clone() else {
        let message = "Este programa não declara um desinstalador. Use o Painel de Controle."
            .to_string();
        touch_session(&session);
        return Err(message);
    };
    let Some((program, args)) = split_command_line(&command_line) else {
        let message = "O comando de desinstalação registrado é inválido".to_string();
        touch_session(&session);
        return Err(message);
    };
    let (program, args) = resolve_uninstaller_program(program, args);
    let Some(program) = program else {
        let message =
            "O caminho do desinstalador não existe mais neste computador.".to_string();
        touch_session(&session);
        return Err(message);
    };

    let started_at = Instant::now();
    let running_app_id = session.app_id.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        wait_for_uninstaller(&program, &args, started_at, &running_app_id)
    })
    .await
    .map_err(|_| "A execução do desinstalador foi interrompida".to_string())?;

    if !outcome.started {
        session.restore_point = restore_point;
        touch_session(&session);
        return Err(outcome.detection_note.unwrap_or_else(|| {
            "O desinstalador não pôde ser iniciado.".to_string()
        }));
    }

    let entry_removed = outcome.entry_removed;
    let app_id = session.app_id.clone();
    // Atualiza o inventário e revarre os resíduos: o que sobrou só é conhecido
    // depois que o desinstalador terminou.
    installed::invalidate_cache();
    let fresh = tauri::async_runtime::spawn_blocking(move || {
        find_app(&app_id).map(|app| residue::scan(&app))
    })
    .await
    .unwrap_or(None);

    // Sem varredura nova (o uninstaller removeu a chave), a lista fica vazia em
    // vez de repetir a antiga: oferecer de novo o que já foi removido seria
    //lie.
    let targets = fresh.unwrap_or_else(|| UninstallTargets {
        registry_keys: Vec::new(),
        folders: Vec::new(),
        skipped_reason: Some(
            "O programa não está mais registrado; não há resíduos a analisar.".to_string(),
        ),
        ..session.targets.clone()
    });
    let snapshot_targets = targets.clone();
    let (registry_snapshot, folder_snapshot) = tauri::async_runtime::spawn_blocking(move || {
        take_snapshot(&snapshot_targets)
    })
    .await
    .unwrap_or((Vec::new(), Vec::new()));

    session.targets = targets.clone();
    session.registry_snapshot = registry_snapshot;
    session.folder_snapshot = folder_snapshot;
    session.restore_point = restore_point.clone();
    touch_session(&session);

    Ok(UninstallerOutcome {
        started: true,
        exit_code: outcome.exit_code,
        timed_out: outcome.timed_out,
        entry_removed,
        detection_note: outcome.detection_note,
        messages: outcome.messages,
        restore_point,
        targets,
        finished_at_unix_ms: now_unix_ms(),
    })
}

/// Alguns instaladores registram o `UninstallString` sem aspas
/// (`C:\Program Files\App\unins.exe /q`). O parser, corretamente, devolve
/// `C:\Program` como programa — então remontamos os tokens enquanto o arquivo
/// não existir, em vez de tentar executar um caminho que não existe.
pub fn resolve_uninstaller_program(
    program: String,
    args: Vec<String>,
) -> (Option<String>, Vec<String>) {
    if Path::new(&program).exists() {
        return (Some(program), args);
    }
    let mut tokens = vec![program];
    tokens.extend(args);
    for take in 2..=tokens.len().min(6) {
        let candidate = tokens[..take].join(" ");
        if Path::new(&candidate).exists() {
            return (Some(candidate), tokens[take..].to_vec());
        }
    }
    (None, tokens)
}

struct RunOutcome {
    started: bool,
    exit_code: Option<i32>,
    timed_out: bool,
    entry_removed: bool,
    detection_note: Option<String>,
    messages: Vec<String>,
}

fn wait_for_uninstaller(
    program: &str,
    args: &[String],
    started_at: Instant,
    app_id: &str,
) -> RunOutcome {
    let mut command = std::process::Command::new(program);
    command.args(args);
    // O desinstalador é interativo por definição; nada de `CREATE_NO_WINDOW`.
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return RunOutcome {
                started: false,
                exit_code: None,
                timed_out: false,
                entry_removed: false,
                detection_note: Some(format!(
                    "Não foi possível executar {}: {}",
                    program,
                    describe_spawn_error(&error, program)
                )),
                messages: Vec::new(),
            }
        }
    };

    let mut exit_code = None;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                break;
            }
            Ok(None) => {}
            Err(_) => {
                return RunOutcome {
                    started: true,
                    exit_code: None,
                    timed_out: false,
                    entry_removed: false,
                    detection_note: Some(
                        "O desinstalador foi encerrado de forma inesperada.".to_string(),
                    ),
                    messages: Vec::new(),
                }
            }
        }
        if started_at.elapsed() >= UNINSTALLER_TIMEOUT {
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(700));
    }

    let mut messages = exit_code.map(describe_exit_code).unwrap_or_default();
    if timed_out {
        messages.push(
            "O desinstalador continua em execução e o Pulse parou de aguardá-lo. Confira na tela se a remoção terminou.".to_string(),
        );
    }

    let entry_removed = if timed_out {
        false
    } else {
        wait_for_entry_removal(app_id)
    };
    let detection_note = if entry_removed || timed_out {
        None
    } else {
        Some(
            "A janela do desinstalador fechou, mas o Windows ainda lista este programa. A limpeza de resíduos não foi recomendada.".to_string(),
        )
    };

    RunOutcome {
        started: true,
        exit_code,
        timed_out,
        entry_removed,
        detection_note,
        messages,
    }
}

fn describe_spawn_error(error: &std::io::Error, program: &str) -> String {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => format!(
            "o arquivo '{}' não existe mais (o instalador foi removido)",
            program
        ),
        ErrorKind::PermissionDenied => {
            "o Windows exigiu elevação e o Pulse não está como administrador".to_string()
        }
        _ => format!("{}", error),
    }
}

/// Enquanto o item ainda estiver registrado, afirmar "terminou" seria mentira:
/// a remoção da chave é a única prova observável de conclusão.
fn wait_for_entry_removal(app_id: &str) -> bool {
    let deadline = Instant::now() + REGISTRY_SETTLE_TIMEOUT;
    loop {
        if !app_entry_exists(app_id) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1200));
    }
}

/// Existence check sem custo de inventário: usa a última chave conhecida pelo
/// chamador. Mantido separado para que o poll não reenumere o sistema.
pub fn app_entry_exists(app_id: &str) -> bool {
    match parse_app_id(app_id) {
        Some((hive, view, subkey)) => RegHandle::exists(hive, &subkey, view),
        None => false,
    }
}

#[tauri::command]
pub async fn cleanup_uninstall_residue(
    session_token: String,
    registry_ids: Vec<String>,
    folder_paths: Vec<String>,
) -> Result<CleanupReport, String> {
    let session = take_session(&session_token)
        .ok_or_else(|| "Sessão de desinstalação expirou. Abra a lista novamente.".to_string())?;

    let computation = tauri::async_runtime::spawn_blocking({
        let session = session.clone();
        move || cleanup_with_session(&session, registry_ids, folder_paths)
    })
    .await;

    match computation {
        Ok(Ok((report, refreshed_folders, refreshed_registry))) => {
            let mut updated = session;
            updated.folder_snapshot = refreshed_folders;
            updated.registry_snapshot = refreshed_registry;
            touch_session(&updated);
            Ok(report)
        }
        // Item recusado não pode derrubar a sessão: o usuário corrigiu a marcação
        // e tenta de novo sem reabrir a lista.
        Ok(Err(message)) => {
            touch_session(&session);
            Err(message)
        }
        Err(_) => {
            touch_session(&session);
            Err("A limpeza foi interrompida".to_string())
        }
    }
}

/// Cada id recebido do front precisa existir **na sessão** e no snapshot: é a
/// barreira contra um `invoke` forjado com caminho de sistema.
type CleanupComputation = (CleanupReport, Vec<SnapshotEntry>, Vec<SnapshotEntry>);

fn cleanup_with_session(
    session: &Session,
    registry_ids: Vec<String>,
    folder_paths: Vec<String>,
) -> Result<CleanupComputation, String> {
    let deadline = residue::now_deadline();
    let shell_patterns = residue::shell_folder_patterns();
    let other_dirs = residue::other_install_dirs_excluding(&session.app_id);
    let mut items: Vec<CleanupItemResult> = Vec::new();
    let mut registry_removed = 0usize;
    let mut folder_removed = 0usize;
    let mut freed_bytes = 0u64;
    let mut refreshed_folders = session.folder_snapshot.clone();
    let mut refreshed_registry = session.registry_snapshot.clone();
    let registry_selected = registry_ids.len();
    let folder_selected = folder_paths.len();

    for id in registry_ids {
        let Some(key) = session
            .targets
            .registry_keys
            .iter()
            .find(|key| key.id == id)
        else {
                return Err(format!(
                "A chave {} não faz parte desta sessão de desinstalação",
                id
            ));
        };
        let snapshot = session
            .folder_snapshot
            .iter()
            .chain(session.registry_snapshot.iter())
            .find(|entry| entry.id == id);
        if snapshot.is_none() {
            return Err(format!(
                "A chave {} não foi registrada na varredura desta sessão",
                id
            ));
        }
        let result = residue::remove_registry_key(key);
        let removed = result.is_ok();
        if removed {
            registry_removed += 1;
            // Reenviar o mesmo id não pode contar duas remoções.
            if let Some(entry) = refreshed_registry.iter_mut().find(|entry| entry.id == id) {
                entry.existed = false;
            }
        }
        items.push(CleanupItemResult {
            id: id.clone(),
            path: key.path.clone(),
            removed,
            freed_bytes: None,
            message: match result {
                Ok(()) => "Chave removida".to_string(),
                Err(message) => message,
            },
        });
    }

    for path in folder_paths {
        let normalized = residue::normalize(Path::new(&path));
        let Some(root) = session
            .targets
            .folders
            .iter()
            .find(|folder| is_inside_tree(&folder.path, &normalized))
            .cloned()
        else {
            return Err(format!(
                "{} não pertence a nenhum item analisado nesta sessão",
                path
            ));
        };
        if let Some(reason) =
            residue::folder_deletion_veto(&normalized, &shell_patterns, &other_dirs)
        {
            return Err(format!("{}: {}", normalized, reason));
        }
        // O snapshot é da raiz: remover uma subpasta isolada exigiria um
        // snapshot próprio, e comparar o da raiz com a subpasta nunca casaria.
        // Preferimos recusar com motivo explícito a aceitar uma remoção sem
        // prova.
        if normalized != residue::normalize(Path::new(&root.path)) {
            return Err(format!(
                "{} está dentro de um item já analisado; marque a pasta raiz para remover a subárvore",
                normalized
            ));
        }
        let current = residue::snapshot_folder(Path::new(&normalized));
        let allowed = session
            .folder_snapshot
            .iter()
            .find(|entry| entry.id == root.path)
            .map(|snapshot| residue::matches_snapshot(snapshot, &current))
            .unwrap_or(false);
        if !allowed {
            let incomplete = session
                .folder_snapshot
                .iter()
                .find(|entry| entry.id == root.path)
                .map(|entry| !entry.complete)
                .unwrap_or(false);
            return Err(if incomplete {
                format!(
                    "{} tem conteúdo acima do limite de análise; o Pulse não apaga o que não conseguiu ver por completo",
                    normalized
                )
            } else {
                format!(
                    "{} mudou depois da varredura; revise a lista antes de remover",
                    normalized
                )
            });
        }
        // A raiz consumida some do snapshot: reenviar o mesmo item na próxima
        // chamada precisa falhar em vez de apagar de novo.
        if let Some(entry) = refreshed_folders.iter_mut().find(|entry| entry.id == root.path) {
            entry.existed = false;
        }

        match residue::remove_folder(
            Path::new(&normalized),
            deadline,
            &shell_patterns,
            &other_dirs,
        ) {
            Ok(freed) => {
                folder_removed += 1;
                freed_bytes = freed_bytes.saturating_add(freed);
                items.push(CleanupItemResult {
                    id: normalized.clone(),
                    path: normalized,
                    removed: true,
                    freed_bytes: Some(freed),
                    message: "Pasta removida".to_string(),
                });
            }
            Err(message) => items.push(CleanupItemResult {
                id: path.clone(),
                path: normalized,
                removed: false,
                freed_bytes: None,
                message,
            }),
        }
    }

    let report = CleanupReport {
        registry_selected,
        registry_removed,
        folder_selected,
        folder_removed,
        freed_bytes,
        items,
        finished_at_unix_ms: now_unix_ms(),
    };
    Ok((report, refreshed_folders, refreshed_registry))
}

/// `true` quando `candidate` é a própria raiz ou está dentro dela. É o que
/// impede o front de pedir a remoção de `C:\Windows` a partir de uma raiz
/// Analysisada.
pub fn is_inside_tree(root: &str, candidate: &str) -> bool {
    let root = residue::normalize(Path::new(root)).to_lowercase();
    let candidate = residue::normalize(Path::new(candidate)).to_lowercase();
    if root.is_empty() || candidate.is_empty() {
        return false;
    }
    candidate == root
        || (candidate.len() > root.len()
            && candidate.starts_with(&root)
            && candidate.as_bytes().get(root.len()) == Some(&b'\\'))
}

#[tauri::command]
pub fn cancel_uninstall_session(session_token: String) -> bool {
    if let Ok(mut guard) = SESSIONS.lock() {
        let before = guard.len();
        guard.retain(|session| session.token != session_token);
        return guard.len() != before;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_de_comando_com_caminho_espacado() {
        let (program, args) =
            split_command_line(r#""C:\Program Files\Foto\unins000.exe" /SILENT"#)
                .expect("deve separar");
        assert_eq!(program, r"C:\Program Files\Foto\unins000.exe");
        assert_eq!(args, vec!["/SILENT".to_string()]);
    }

    #[test]
    fn split_de_comando_msi() {
        let (program, args) =
            split_command_line(r#"MsiExec.exe /X{ABCD-1234} /qn"#).expect("deve separar");
        assert_eq!(program, "MsiExec.exe");
        assert_eq!(args, vec!["/X{ABCD-1234}".to_string(), "/qn".to_string()]);
    }

    #[test]
    fn split_de_comando_invalido() {
        assert!(split_command_line("").is_none());
        assert!(split_command_line("   ").is_none());
        assert!(split_command_line(r#""aberto.exe"#).is_none());
    }

    #[test]
    fn split_preserva_argumentos_vazios_entre_aspas() {
        let (program, args) = split_command_line(r#""C:\a.exe" "" /flag"#).expect("deve separar");
        assert_eq!(program, r"C:\a.exe");
        assert_eq!(args, vec!["".to_string(), "/flag".to_string()]);
    }

    #[test]
    fn containment_de_arvore() {
        assert!(is_inside_tree(
            r"C:\Program Files\Foto",
            r"C:\Program Files\Foto\sub\a.txt"
        ));
        assert!(is_inside_tree(
            r"C:\Program Files\Foto",
            r"C:\Program Files\Foto"
        ));
        assert!(!is_inside_tree(
            r"C:\Program Files\Foto",
            r"C:\Program Files\Fotografia\x"
        ));
        assert!(!is_inside_tree(r"C:\Program Files\Foto", r"C:\Windows\x"));
        assert!(!is_inside_tree("", r"C:\Windows"));
    }

    #[test]
    fn resolve_de_programa_com_espaco_sem_aspas() {
        let (program, args) = resolve_uninstaller_program(
            "C:\\Program".to_string(),
            vec!["Files\\App\\unins.exe".to_string(), "/q".to_string()],
        );
        if program.is_some() {
            assert!(Path::new(program.as_deref().unwrap()).exists());
            assert_eq!(args, vec!["/q".to_string()]);
        } else {
            // Sem o arquivo no disco de teste, o fallback é "não encontrado" —
            // nunca um caminho inventado.
            assert!(args.contains(&"/q".to_string()));
        }
    }

    #[test]
    fn resolve_de_programa_inexistente() {
        let (program, args) =
            resolve_uninstaller_program("nao-existe.exe".to_string(), vec!["/x".to_string()]);
        assert!(program.is_none());
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn exit_codes_sao_traduzidos() {
        assert!(describe_exit_code(0)[0].contains("sucesso"));
        assert!(describe_exit_code(3010)[0].contains("reiniciar"));
        assert!(describe_exit_code(1603)[0].contains("Windows Installer"));
        assert!(describe_exit_code(9999)[0].contains("9999"));
    }
}