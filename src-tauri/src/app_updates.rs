//! Atualizações pendentes de aplicativos via Gerenciador de Pacotes do Windows.
//!
//! O `winget list --upgrade-available` devolve uma tabela cujas colunas são
//! **localizadas** e cujas larguras variam com o conteúdo, então a leitura é
//! feita pelas posições dos cabeçalhos da própria saída — nunca por índice
//! fixo de coluna nem por posição de caractere presumida.
//!
//! O comando leva ~5 s e consulta a fonte remota, então:
//! - roda em `spawn_blocking` (nunca bloqueia a thread de UI);
//! - usa subprocesso sem janela (`crate::process::hidden_command`);
//! - só é acionado sob demanda, nunca no ciclo de polling da Visão geral;
//! - mantém cache longo (30 min) — atualização de app não muda por minuto.

use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Evento emitido durante a instalação, uma vez por linha de saída do winget.
pub const UPGRADE_PROGRESS_EVENT: &str = "winget-upgrade-progress";

/// Id reservado para a atualização em lote (`winget upgrade --all`): o
/// progresso pertence à operação inteira, não a um pacote específico.
pub const ALL_PACKAGES_ID: &str = "*";

/// Progresso de uma atualização. `phase` é `started`, `log`, `finished`,
/// `failed` ou `timeout`; `line` traz o texto literal do winget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeProgress {
    pub id: String,
    pub phase: String,
    pub line: String,
}

/// TTL generoso: `winget` consome rede e ~5 s de CPU.
const UPDATES_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
/// Tempo máximo para o `winget` responder. Acima disso a lista é declarada
/// indisponível em vez de deixar a UI esperando para sempre.
const WINGET_TIMEOUT: Duration = Duration::from_secs(90);
/// Colunas da tabela do winget: Nome, ID, Versão, Disponível, Origem.
const EXPECTED_COLUMNS: usize = 5;
/// Valores aceitos na coluna `Origem`. Uma linha de rodapé do winget nunca
/// produz um desses tokens na última coluna, então isso filtra o texto de nota.
const KNOWN_SOURCES: [&str; 3] = ["winget", "msstore", "source"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingUpdate {
    pub id: String,
    pub name: String,
    pub current_version: String,
    pub available_version: String,
    pub source: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesSnapshot {
    pub updates: Vec<PendingUpdate>,
    pub total_count: usize,
    /// Preenchido quando o winget não existe, falha ou excede o tempo limite.
    /// Uma lista vazia com `None` significa "nenhuma atualização pendente".
    pub unavailable_reason: Option<String>,
    pub collected_at_unix_ms: u64,
}

struct UpdatesCache {
    snapshot: UpdatesSnapshot,
    fetched_at: Instant,
}

static UPDATES_CACHE: Mutex<Option<UpdatesCache>> = Mutex::new(None);

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn unavailable(reason: impl Into<String>) -> UpdatesSnapshot {
    UpdatesSnapshot {
        updates: Vec::new(),
        total_count: 0,
        unavailable_reason: Some(reason.into()),
        collected_at_unix_ms: now_unix_ms(),
    }
}

/// Localiza o executável do winget.
///
/// A instalação moderna usa o alias em `%LOCALAPPDATA%\Microsoft\WindowsApps`, que
/// é um **App Execution Alias**: existe como link, mas o arquivo real é
/// `winget.exe` dentro do pacote. Verificamos os dois caminhos.
fn winget_path() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            std::path::PathBuf::from(path)
                .join("Microsoft")
                .join("WindowsApps")
                .join("winget.exe"),
        );
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(
            std::path::PathBuf::from(program_files)
                .join("WindowsApps")
                .join("winget.exe"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// Interpreta a saída tabular do winget.
///
/// A tabela tem 5 colunas (Nome, ID, Versão, Disponível, Origem) alinhadas por
/// posição. Descobrimos os limites lendo os **tokens do cabeçalho**, então
/// funciona em pt-BR, en-US ou qualquer outro idioma instalado — o texto muda,
/// as colunas não.
///
/// Detalhe crítico: os limites são contados em **caracteres**, não em bytes.
/// O cabeçalho pt-BR contém `Versão` e `Disponível` (2 bytes por letra
/// acentuada); usar offsets de byte nas linhas de dados — que são ASCII — desloca
/// todas as colunas a partir da terceira e corrompe as versões. Por isso o
/// corte é feito com `char_indices`.
///
/// A linha de notas do rodapé (`57 atualizações disponíveis.`) e avisos de
/// versão indeterminada são ignorados: só linhas com ID no formato
/// `Publicador.Pacote` e `Origem` conhecida viram atualizações.
pub fn parse_winget_upgrades(output: &str) -> Vec<PendingUpdate> {
    let lines: Vec<&str> = output.lines().collect();

    // 1. Localiza a linha de cabeçalho: a única com exatamente 5 tokens, o
    //    que descreve as colunas usadas nas linhas de dados.
    let header_index = lines.iter().position(|line| {
        column_offsets(line)
            .map(|offsets| offsets.len() == EXPECTED_COLUMNS)
            .unwrap_or(false)
    });

    let Some(header_index) = header_index else {
        return Vec::new();
    };
    let Some(offsets) = column_offsets(lines[header_index]) else {
        return Vec::new();
    };

    let mut updates = Vec::new();
    for line in lines.iter().skip(header_index + 1) {
        let trimmed = line.trim();
        // Linha vazia, separador `-----` ou nota de rodapé/aviso.
        if trimmed.is_empty() || trimmed.starts_with('-') {
            continue;
        }

        let fields = slice_columns(line, &offsets);
        let [name, id, version, available, source] = fields[..] else {
            continue;
        };

        // `id` de pacote do winget é sempre Publicador.Pacote, sem espaços.
        // Isso descarta linhas de nota sem depender do idioma instalado.
        if !id.contains('.') || id.contains(char::is_whitespace) {
            continue;
        }
        if name.is_empty() || available.is_empty() {
            continue;
        }
        // A última coluna é a origem do pacote (winget, msstore, ...). Uma
        // linha de rodapé nunca chega até aqui com esse valor preenchido.
        if !KNOWN_SOURCES.contains(&source) {
            continue;
        }

        updates.push(PendingUpdate {
            id: id.to_string(),
            name: name.to_string(),
            // Versões ausentes ficam vazias de propósito: a UI mostra
            // "Indisponível" em vez de um número inventado.
            current_version: version.to_string(),
            available_version: available.to_string(),
            source: if source.is_empty() {
                "Origem desconhecida".to_string()
            } else {
                source.to_string()
            },
            icon: None,
        });
    }

    updates
}

/// Enriquece os pacotes do winget com os ícones dos aplicativos instalados no sistema.
pub fn enrich_updates_with_icons(updates: &mut [PendingUpdate]) {
    let installed = crate::uninstall::installed::collect_cached().apps;
    for update in updates.iter_mut() {
        let name_lower = update.name.trim().to_lowercase();
        let id_lower = update.id.trim().to_lowercase();

        for app in &installed {
            let app_name_lower = app.display_name.trim().to_lowercase();
            if app_name_lower == name_lower
                || (app_name_lower.contains(&name_lower) && !name_lower.is_empty())
                || (name_lower.contains(&app_name_lower) && !app_name_lower.is_empty())
                || app.id.to_lowercase().contains(&id_lower)
            {
                if let Some(icon) = &app.icon {
                    update.icon = Some(icon.clone());
                    break;
                }
            }
        }
    }
}

/// Posições de início de cada coluna, em **caracteres**, derivadas dos tokens
/// do cabeçalho. Retorna `None` quando a linha não tem exatamente 5 colunas.
fn column_offsets(line: &str) -> Option<Vec<usize>> {
    let mut offsets = Vec::new();
    for (index, ch) in line.chars().enumerate() {
        if ch != ' ' {
            // Só conta o início de token (anterior era espaço ou início da linha).
            if index == 0 || line.chars().nth(index - 1) == Some(' ') {
                offsets.push(index);
            }
        }
    }
    if offsets.len() == EXPECTED_COLUMNS {
        Some(offsets)
    } else {
        None
    }
}

/// Recorta as colunas de uma linha de dados pelos offsets do cabeçalho.
///
/// O corte respeita fronteiras de caractere UTF-8 (`char_indices`), então uma
/// linha acentuada nunca é cortada no meio de um caractere — o `str` retornado
/// seria inválido e o `get()` cairia no `unwrap_or("")` silenciosamente.
fn slice_columns<'a>(line: &'a str, offsets: &[usize]) -> [&'a str; EXPECTED_COLUMNS] {
    let mut bounds: Vec<usize> = offsets.to_vec();
    bounds.push(line.chars().count());

    let starts: Vec<usize> = line.char_indices().map(|(index, _)| index).collect();
    let mut fields = [""; EXPECTED_COLUMNS];
    for (index, start) in bounds.iter().enumerate().take(EXPECTED_COLUMNS) {
        let from = starts.get(*start).copied().unwrap_or(line.len());
        let to = bounds
            .get(index + 1)
            .and_then(|next| starts.get(*next).copied())
            .unwrap_or(line.len());
        fields[index] = line.get(from..to).unwrap_or("").trim();
    }
    fields
}

/// Extrai o número de atualizações do rodapé, quando presente.
/// Usado apenas para conferência: a métrica exibida vem da lista real.
pub fn parse_footer_total(output: &str) -> Option<usize> {
    for line in output.lines().rev() {
        let trimmed = line.trim();
        let mut digits = String::new();
        for ch in trimmed.chars() {
            if ch.is_ascii_digit() {
                digits.push(ch);
            } else {
                break;
            }
        }
        if digits.is_empty() {
            continue;
        }
        if trimmed.contains("atualiza")
            || trimmed.contains("upgrade")
            || trimmed.contains("actualiza")
        {
            return digits.parse().ok();
        }
    }
    None
}

/// Tempo máximo de um upgrade real. Um pacote grande (SDK, driver, jogo)
/// baixa e instala em minutos, então o limite é bem acima do de `list`.
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Teto de caracteres devolvidos ao front. A saída do `winget upgrade` é
/// curta; truncar evita inflar o JSON do Tauri sem perder informação útil.
const UPGRADE_OUTPUT_LIMIT: usize = 4000;
/// Comprimento máximo aceitável para o ID de pacote.
const MAX_PACKAGE_ID_LEN: usize = 200;

/// Valida o identificador do pacote antes de virá-lo argumento de subprocesso.
///
/// O ID vem da lista de atualizações (confiável), mas é validado mesmo assim
/// porque é repassado como argumento do `winget`: aspas ou aspas duplas
/// permitiriam reescrever a linha de comando. Números também são aceitos
/// porque pacotes do Microsoft Store são referenciados por ProductId.
fn validate_package_id(id: &str) -> Result<(), String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err("O identificador do pacote está vazio".to_string());
    }
    if trimmed.chars().count() > MAX_PACKAGE_ID_LEN {
        return Err(format!(
            "O identificador do pacote excede {MAX_PACKAGE_ID_LEN} caracteres"
        ));
    }
    if trimmed.contains('"') || trimmed.contains('\'') {
        return Err("O identificador do pacote contém aspas, que não são permitidas".to_string());
    }
    Ok(())
}

/// Falha de uma atualização, já traduzida para o usuário.
///
/// O `winget` devolve um HRESULT como código de saída e uma saída localizada
/// que começa pela tabela de pacotes. Repassar essa saída crua à UI mostrava a
/// tabela inteira como "erro" e escondia a causa real (ex.: `0x80073D28`,
/// pacote MSIX que registra serviço e exige administrador). Por isso a causa é
/// derivada do **código** (independe de idioma) e a saída entra só como
/// `detail`, filtrada.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeFailure {
    /// Frase pronta para a UI, em pt-BR.
    pub message: String,
    /// Código de saída em hexadecimal (`0x8A15002C`), quando houver.
    pub code: Option<String>,
    /// A falha some ao repetir a operação com privilégios de administrador.
    pub needs_admin: bool,
    /// Últimas linhas úteis da saída do winget (sem tabela nem barra de progresso).
    pub detail: Option<String>,
}

impl UpgradeFailure {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            needs_admin: false,
            detail: None,
        }
    }

    fn from_exit_code(code: Option<i32>, output: &str) -> Self {
        let detail = relevant_output_detail(output);
        match code {
            Some(code) => {
                let (message, needs_admin) = describe_exit_code(code);
                Self {
                    message,
                    code: Some(format_exit_code(code)),
                    needs_admin,
                    detail,
                }
            }
            None => Self {
                message: "O winget foi encerrado sem código de saída".to_string(),
                code: None,
                needs_admin: false,
                detail,
            },
        }
    }
}

/// Código de saída no formato em que o winget e a documentação o exibem.
pub fn format_exit_code(code: i32) -> String {
    if !(0..=0xFFFF).contains(&code) {
        format!("0x{:08X}", code as u32)
    } else {
        code.to_string()
    }
}

/// `ERROR_CANCELLED`: devolvido pelo script de elevação quando o usuário
/// recusa o aviso do Controle de Conta de Usuário.
const UAC_CANCELLED_EXIT_CODE: i32 = 1223;

/// Traduz o código de saída do winget (HRESULT) para pt-BR e diz se a falha
/// depende de privilégios de administrador.
///
/// Tabela oficial: `winget error --output` / doc `returnCodes.md` do winget-cli.
pub fn describe_exit_code(code: i32) -> (String, bool) {
    let admin = |text: &str| (text.to_string(), true);
    let plain = |text: &str| (text.to_string(), false);
    match code as u32 {
        0x8007_3D28 => admin(
            "O pacote registra um serviço do Windows e exige privilégios de administrador",
        ),
        0x8A15_0019 => admin("O comando exige privilégios de administrador"),
        0x8007_0005 => admin("Acesso negado pelo Windows; tente como administrador"),
        0x8A15_0006 => plain("O instalador do aplicativo terminou com erro"),
        0x8A15_0008 => plain("Falha ao baixar o instalador"),
        0x8A15_0010 => plain("Nenhum instalador do pacote é compatível com este sistema"),
        0x8A15_0011 => plain("O instalador baixado não confere com o hash do manifesto"),
        0x8A15_002B => plain(
            "Nenhuma atualização aplicável: a versão instalada não pôde ser comparada ou o pacote está fixado",
        ),
        0x8A15_002C => plain("Uma ou mais atualizações falharam"),
        0x8A15_0065 => plain("Um ou mais aplicativos não foram instalados"),
        0x8A15_0101 => plain("O aplicativo está em execução; feche-o e tente de novo"),
        0x8A15_0102 => plain("Outra instalação já está em andamento; tente mais tarde"),
        0x8A15_0103 => plain("Há arquivos do aplicativo em uso; feche-o e tente de novo"),
        0x8A15_0104 => plain("Falta uma dependência do pacote neste sistema"),
        0x8A15_0105 => plain("Sem espaço livre em disco para instalar"),
        0x8A15_0106 => plain("Memória insuficiente para instalar; feche outros aplicativos"),
        0x8A15_0107 => plain("O instalador precisa de conexão com a internet"),
        0x8A15_0109 => plain("Reinicie o computador para concluir a instalação"),
        0x8A15_010A => plain("Reinicie o computador e tente de novo"),
        0x8A15_010C => plain("A instalação foi cancelada"),
        0x8A15_010E => plain("Uma versão mais nova do aplicativo já está instalada"),
        0x8A15_010F => plain("Políticas da organização bloqueiam esta instalação"),
        0x8A15_0110 => plain("Falha ao instalar as dependências do pacote"),
        0x8A15_0111 => plain("O aplicativo está em uso por outro programa; feche-o e tente de novo"),
        0x8A15_0114 => plain(
            "O instalador não aceita atualizar por cima; desinstale e instale de novo",
        ),
        _ if code == UAC_CANCELLED_EXIT_CODE => {
            plain("A elevação foi recusada no aviso do Controle de Conta de Usuário")
        }
        _ => (format!("O winget falhou com o código {}", format_exit_code(code)), false),
    }
}

/// Linhas da saída que explicam a falha.
///
/// A saída do `upgrade` repete a tabela de pacotes e barras de progresso
/// (`██▒▒ 10 MB / 50 MB`); nada disso explica o erro. Descartamos linhas de
/// tabela (ID conhecido + origem conhecida), separadores e progresso, e
/// devolvemos as últimas 3 restantes — é onde o winget escreve o motivo.
pub fn relevant_output_detail(output: &str) -> Option<String> {
    let table_ids: Vec<String> = parse_winget_upgrades(output)
        .into_iter()
        .map(|update| update.id)
        .collect();
    let useful: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.chars().all(|ch| ch == '-' || ch.is_whitespace()))
        .filter(|line| !line.contains('█') && !line.contains('▒'))
        .filter(|line| !matches!(*line, "-" | "\\" | "|" | "/"))
        .filter(|line| {
            let last = line.split_whitespace().last().unwrap_or("");
            !(KNOWN_SOURCES.contains(&last)
                && table_ids.iter().any(|id| line.contains(id.as_str())))
        })
        .filter(|line| column_offsets(line).is_none())
        .collect();
    if useful.is_empty() {
        return None;
    }
    let start = useful.len().saturating_sub(3);
    Some(useful[start..].join("\n"))
}

/// Instala/atualiza um pacote via `winget upgrade`.
///
/// Diferente da listagem, isto é uma ação do usuário: roda sob demanda, sem
/// cache, com timeout longo (upgrade é lento) e propaga a causa da falha —
/// como "exige administrador" — para a UI, em vez de engoli-la.
#[tauri::command]
pub async fn upgrade_winget_package(id: String, app: AppHandle) -> Result<String, UpgradeFailure> {
    validate_package_id(&id).map_err(UpgradeFailure::plain)?;
    let package_id = id.trim().to_string();

    let result = tauri::async_runtime::spawn_blocking(move || {
        run_winget_upgrade(package_upgrade_args(&package_id), (app, package_id))
    })
    .await
    .map_err(|_| UpgradeFailure::plain("A atualização foi interrompida"))?;
    invalidate_updates_cache();
    result
}

fn package_upgrade_args(id: &str) -> Vec<String> {
    vec![
        "upgrade".to_string(),
        "--id".to_string(),
        id.to_string(),
        "--exact".to_string(),
    ]
}

/// Falha de um pacote dentro do lote.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatchFailure {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub failure: UpgradeFailure,
}

/// Resultado de "Atualizar tudo": quem foi atualizado e por que os demais não.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatchUpgradeReport {
    pub total: usize,
    pub succeeded: Vec<String>,
    pub failed: Vec<BatchFailure>,
}

/// Atualiza **todos** os pacotes pendentes, um `winget upgrade --id` por vez.
///
/// Não usamos `winget upgrade --all`: ele devolve um único código
/// (`0x8A15002C`, "concluído com falhas") e uma saída localizada sem dizer
/// qual pacote falhou nem por quê. Pacote a pacote, cada um tem código de
/// saída próprio, a linha correspondente mostra o próprio progresso e o
/// relatório final lista as falhas com a causa traduzida.
#[tauri::command]
pub async fn upgrade_all_winget_packages(
    app: AppHandle,
) -> Result<BatchUpgradeReport, UpgradeFailure> {
    let result = tauri::async_runtime::spawn_blocking(move || run_upgrade_all(&app))
        .await
        .map_err(|_| UpgradeFailure::plain("A atualização foi interrompida"))?;
    invalidate_updates_cache();
    result
}

fn run_upgrade_all(app: &AppHandle) -> Result<BatchUpgradeReport, UpgradeFailure> {
    let snapshot = cached_or_fresh_snapshot(false);
    if let Some(reason) = snapshot.unavailable_reason {
        return Err(UpgradeFailure::plain(reason));
    }

    let total = snapshot.updates.len();
    emit_progress(
        app,
        ALL_PACKAGES_ID,
        "started",
        &format!("{total} aplicativos na fila de atualização"),
    );
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    for (index, update) in snapshot.updates.iter().enumerate() {
        emit_progress(
            app,
            ALL_PACKAGES_ID,
            "log",
            &format!("({}/{total}) Atualizando {}…", index + 1, update.name),
        );
        let outcome = validate_package_id(&update.id)
            .map_err(UpgradeFailure::plain)
            .and_then(|_| {
                run_winget_upgrade(
                    package_upgrade_args(&update.id),
                    (app.clone(), update.id.clone()),
                )
            });
        match outcome {
            Ok(_) => succeeded.push(update.id.clone()),
            Err(failure) => failed.push(BatchFailure {
                id: update.id.clone(),
                name: update.name.clone(),
                failure,
            }),
        }
    }
    emit_progress(
        app,
        ALL_PACKAGES_ID,
        "finished",
        &format!("{} de {total} aplicativos atualizados", succeeded.len()),
    );
    Ok(BatchUpgradeReport {
        total,
        succeeded,
        failed,
    })
}

/// Repete a atualização de um pacote **como administrador**.
///
/// O Pulse roda sem elevação; pacotes MSIX que registram serviço (Claude,
/// WSL, Codex) falham com `0x80073D28`. Aqui o winget é relançado via
/// `Start-Process -Verb RunAs`, o que mostra o aviso do UAC — o usuário decide.
/// Um processo elevado não pode ter a saída redirecionada para um processo
/// comum, então o progresso é só o código de saída final.
#[tauri::command]
pub async fn upgrade_winget_package_elevated(
    id: String,
    app: AppHandle,
) -> Result<String, UpgradeFailure> {
    validate_package_id(&id).map_err(UpgradeFailure::plain)?;
    let package_id = id.trim().to_string();

    let result =
        tauri::async_runtime::spawn_blocking(move || run_elevated_upgrade(package_id, app))
            .await
            .map_err(|_| UpgradeFailure::plain("A atualização foi interrompida"))?;
    invalidate_updates_cache();
    result
}

/// Script PowerShell de uma linha que relança o winget elevado e devolve o
/// código de saída dele. Recusa no UAC vira `1223` (`ERROR_CANCELLED`).
pub fn elevated_upgrade_script(winget: &str, id: &str) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let mut args = package_upgrade_args(id);
    args.extend(
        [
            "--accept-source-agreements",
            "--accept-package-agreements",
            "--disable-interactivity",
        ]
        .map(String::from),
    );
    let arg_list = args
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "$ErrorActionPreference='Stop'; try {{ $p = Start-Process -FilePath {} -ArgumentList @({arg_list}) -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode }} catch {{ $e = $_.Exception; while ($e) {{ if ($e.NativeErrorCode -eq {UAC_CANCELLED_EXIT_CODE} -or $e.HResult -eq -2147023673) {{ exit {UAC_CANCELLED_EXIT_CODE} }}; $e = $e.InnerException }}; [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}",
        quote(winget)
    )
}

fn run_elevated_upgrade(id: String, app: AppHandle) -> Result<String, UpgradeFailure> {
    use base64::Engine;

    let Some(path) = winget_path() else {
        return Err(UpgradeFailure::plain(
            "Gerenciador de Pacotes do Windows (winget) não encontrado neste sistema",
        ));
    };
    let script = elevated_upgrade_script(&path.to_string_lossy(), &id);
    // `-EncodedCommand` (UTF-16LE em base64) elimina qualquer problema de
    // aspas ao repassar o script como argumento.
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);

    let mut child = crate::process::hidden_command("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            UpgradeFailure::plain(format!("Não foi possível iniciar o PowerShell: {error}"))
        })?;

    emit_progress(
        &app,
        &id,
        "started",
        "Aguardando confirmação no aviso de administrador do Windows…",
    );
    let mut readers = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        readers.push(spawn_output_reader(pipe, app.clone(), id.clone()));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(spawn_output_reader(pipe, app.clone(), id.clone()));
    }

    let status = wait_for_upgrade(&mut child, &app, &id)?;
    let output = join_readers(readers);
    if !status.success() {
        let failure = UpgradeFailure::from_exit_code(status.code(), &output);
        emit_progress(&app, &id, "failed", &failure.message);
        return Err(failure);
    }
    emit_progress(&app, &id, "finished", "Concluído como administrador");
    Ok(truncate_chars(&output, UPGRADE_OUTPUT_LIMIT))
}

/// Executa `winget upgrade` com os argumentos informados e devolve a saída.
///
/// Com `progress` o comando emite `UPGRADE_PROGRESS_EVENT` a cada linha de
/// saída do winget: o front precisa mostrar o que está rodando, porque vários
/// upgrades podem ficar em paralelo. O `winget` **não expõe porcentagem** de
/// progresso, então a barra é indeterminada e o texto da saída é o feedback
/// honesto disponível — nada de porcentagem fictícia.
fn run_winget_upgrade(
    mut args: Vec<String>,
    progress: (AppHandle, String),
) -> Result<String, UpgradeFailure> {
    let Some(path) = winget_path() else {
        return Err(UpgradeFailure::plain(
            "Gerenciador de Pacotes do Windows (winget) não encontrado neste sistema",
        ));
    };

    args.extend([
        "--accept-source-agreements".to_string(),
        "--accept-package-agreements".to_string(),
        "--disable-interactivity".to_string(),
    ]);

    let mut child = crate::process::hidden_command(&path)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            UpgradeFailure::plain(format!("Não foi possível executar o winget: {error}"))
        })?;

    let (app, id) = progress;
    emit_progress(&app, &id, "started", "Instalação iniciada pelo winget");

    // Os pipes são drenados em threads próprias: sem isso o buffer do OS
    // enche, o winget trava escrevendo e o `try_wait` só veria um processo morto
    // de verdade — não um travamento de pipe.
    let mut readers = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        readers.push(spawn_output_reader(pipe, app.clone(), id.clone()));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(spawn_output_reader(pipe, app.clone(), id.clone()));
    }

    let status = wait_for_upgrade(&mut child, &app, &id)?;
    let output_text = join_readers(readers);

    if !status.success() {
        let failure = UpgradeFailure::from_exit_code(status.code(), &output_text);
        emit_progress(&app, &id, "failed", &failure.message);
        return Err(failure);
    }

    emit_progress(&app, &id, "finished", "Concluído");
    Ok(truncate_chars(&output_text, UPGRADE_OUTPUT_LIMIT))
}

/// Espera o processo com o teto de `UPGRADE_TIMEOUT`. No timeout o processo é
/// encerrado e a falha devolvida; as threads de leitura terminam sozinhas
/// quando o pipe fecha.
fn wait_for_upgrade(
    child: &mut std::process::Child,
    app: &AppHandle,
    id: &str,
) -> Result<std::process::ExitStatus, UpgradeFailure> {
    let deadline = Instant::now() + UPGRADE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    emit_progress(app, id, "timeout", "Tempo limite de 15 minutos excedido");
                    return Err(UpgradeFailure::plain(
                        "A atualização excedeu o tempo limite de 15 minutos e foi cancelada",
                    ));
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(error) => {
                return Err(UpgradeFailure::plain(format!(
                    "Falha ao acompanhar o processo do winget: {error}"
                )));
            }
        }
    }
}

fn join_readers(readers: Vec<std::thread::JoinHandle<String>>) -> String {
    let mut output = String::new();
    for reader in readers {
        if let Ok(text) = reader.join() {
            output.push_str(&text);
        }
    }
    output
}

/// Lê a saída de um pipe linha a linha, emitindo cada linha e devolvendo o
/// texto completo. `Read::read` bloqueia até haver dado, então a linha só é
/// emitida quando o winget a escreve de fato — nada de progresso inventado.
///
/// O winget redesenha a barra de progresso com `\r` na mesma linha; só o
/// último quadro de cada linha é aproveitado.
fn spawn_output_reader<R: std::io::Read + Send + 'static>(
    pipe: R,
    app: AppHandle,
    id: String,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(pipe);
        let mut full = String::new();
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Some(trimmed) = last_frame(&line) else {
                continue;
            };
            if full.len() < UPGRADE_OUTPUT_LIMIT {
                full.push_str(trimmed);
                full.push('\n');
            }
            emit_progress(&app, &id, "log", trimmed);
        }
        full
    })
}

/// Último quadro não vazio de uma linha redesenhada com `\r`.
fn last_frame(line: &str) -> Option<&str> {
    line.split('\r')
        .map(str::trim)
        .rfind(|frame| !frame.is_empty())
}

fn emit_progress(app: &AppHandle, id: &str, phase: &str, line: &str) {
    let _ = app.emit(
        UPGRADE_PROGRESS_EVENT,
        UpgradeProgress {
            id: id.to_string(),
            phase: phase.to_string(),
            line: truncate_chars(line, 300),
        },
    );
}

/// Corta por caracteres (nunca por byte), para não quebrar um caractere UTF-8
/// no meio e produzir uma `String` inválida.
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(limit).collect();
    truncated.push_str("\n(saída truncada)");
    truncated
}

#[tauri::command]
pub async fn list_pending_updates(force_refresh: Option<bool>) -> UpdatesSnapshot {
    let force = force_refresh.unwrap_or(false);
    match tauri::async_runtime::spawn_blocking(move || cached_or_fresh_snapshot(force)).await {
        Ok(snapshot) => snapshot,
        Err(_) => unavailable("A verificação de atualizações foi interrompida"),
    }
}

/// Lista do cache (TTL de 30 min) ou recém-consultada no winget. Bloqueante:
/// chamar só de dentro de `spawn_blocking`.
fn cached_or_fresh_snapshot(force_refresh: bool) -> UpdatesSnapshot {
    if !force_refresh {
        if let Ok(guard) = UPDATES_CACHE.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < UPDATES_CACHE_TTL {
                    return cached.snapshot.clone();
                }
            }
        }
    }

    let snapshot = collect_updates();
    if let Ok(mut guard) = UPDATES_CACHE.lock() {
        *guard = Some(UpdatesCache {
            snapshot: snapshot.clone(),
            fetched_at: Instant::now(),
        });
    }
    snapshot
}

/// Depois de instalar algo a lista em cache está defasada.
fn invalidate_updates_cache() {
    if let Ok(mut guard) = UPDATES_CACHE.lock() {
        *guard = None;
    }
}

fn collect_updates() -> UpdatesSnapshot {
    let Some(path) = winget_path() else {
        return unavailable(
            "Gerenciador de Pacotes do Windows (winget) não encontrado neste sistema",
        );
    };

    let mut child = match crate::process::hidden_command(&path)
        .args([
            "list",
            "--upgrade-available",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return unavailable(format!("Não foi possível executar o winget: {error}"));
        }
    };

    // Espera com timeout: `winget` em rede lenta não pode travar a UI.
    let deadline = Instant::now() + WINGET_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return unavailable(
                        "O winget excedeu o tempo limite ao consultar as atualizações",
                    );
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(error) => return unavailable(format!("Falha ao ler a saída do winget: {error}")),
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => return unavailable(format!("Falha ao coletar a saída do winget: {error}")),
    };

    // winget devolve código != 0 quando nada foi atualizado em `--upgrade`
    // sem argumentos; em `list` código 0 com lista vazia é o caso normal.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    let mut updates = parse_winget_upgrades(&combined);
    enrich_updates_with_icons(&mut updates);
    if updates.is_empty()
        && !combined.contains("atualiza")
        && parse_footer_total(&combined).is_none()
    {
        return unavailable(format!(
            "O winget não retornou uma lista legível{}",
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim().lines().next().unwrap_or(""))
            }
        ));
    }

    let total = updates.len();
    UpdatesSnapshot {
        updates,
        total_count: total,
        unavailable_reason: None,
        collected_at_unix_ms: now_unix_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture gerado com larguras de coluna coerentes (max entre cabeçalho e
    // células), reproduzindo a saída real do winget 1.29 em pt-BR.
    const PT_BR_SAMPLE: &str = "Nome                                       ID                                 Versão                        Disponível                    Origem\n------------------------------------------------------------------------------------------------------------------------------------------------\nAnaconda3 2024.06-1 (Python 3.12.4 64-bit) Anaconda.Anaconda3                 2024.06-1                     2026.07-1                     winget\nAnki Launcher                              Anki.Anki                          25.09                         26.09.3                       winget\nDocker Desktop                             Docker.DockerDesktop               4.35.1                        4.36.0                        winget\nWarp                                       Warp.Warp                          v0.2026.08.05.09.03.stable_01 v0.2026.09.30.08.29.stable_01 winget\nNVIDIA Nsight Compute 2026.2.0             Nvidia.Nsight.Compute              < 2026.3.1                    2026.3.1                      winget\nPlayStation® Accessories                   PlayStation.PlayStationAccessories 2.2.0.3                       2.2.1.2                       winget\n";
    const EN_SAMPLE: &str = "Name            Id                   Version Available Source\n-------------------------------------------------------------\nDocker Desktop  Docker.DockerDesktop 4.35.1  4.36.0    winget\nMozilla Firefox Mozilla.Firefox      133.0.0 133.0.3   winget\n";

    #[test]
    fn parses_localized_table_into_updates() {
        let updates = parse_winget_upgrades(PT_BR_SAMPLE);
        assert_eq!(updates.len(), 6);
        assert_eq!(updates[0].id, "Anaconda.Anaconda3");
        // Nome com espaços é preservado inteiro, sem cortar na coluna ID.
        assert_eq!(
            updates[0].name,
            "Anaconda3 2024.06-1 (Python 3.12.4 64-bit)"
        );
        assert_eq!(updates[0].current_version, "2024.06-1");
        assert_eq!(updates[0].available_version, "2026.07-1");
        assert_eq!(updates[0].source, "winget");
        assert_eq!(updates[1].id, "Anki.Anki");
    }

    /// Regressão do bug de accentuação: os cabeçalhos pt-BR têm letras
    /// acentuadas (2 bytes). Usar offsets de BYTE sobre as linhas de dados
    /// — que são ASCII — desloca as colunas 3, 4 e 5 e corrompe as versões.
    #[test]
    fn accents_in_header_do_not_shift_data_columns() {
        let updates = parse_winget_upgrades(PT_BR_SAMPLE);
        // Versões sem resíduo das colunas seguintes (bug byte vs char).
        for update in &updates {
            assert!(
                !update.current_version.contains("winget"),
                "versão atual corrompida por offset em bytes: {:?}",
                update.current_version
            );
            assert!(
                !update.available_version.contains("winget"),
                "versão disponível corrompida por offset em bytes: {:?}",
                update.available_version
            );
            // Cada versão é exatamente o token da sua coluna, sem resíduo da vizinha.
            let expected_current = [
                "2024.06-1",
                "25.09",
                "4.35.1",
                "v0.2026.08.05.09.03.stable_01",
                "< 2026.3.1",
                "2.2.0.3",
            ];
            let expected_available = [
                "2026.07-1",
                "26.09.3",
                "4.36.0",
                "v0.2026.09.30.08.29.stable_01",
                "2026.3.1",
                "2.2.1.2",
            ];
            assert_eq!(
                updates
                    .iter()
                    .map(|u| u.current_version.as_str())
                    .collect::<Vec<_>>(),
                expected_current
            );
            assert_eq!(
                updates
                    .iter()
                    .map(|u| u.available_version.as_str())
                    .collect::<Vec<_>>(),
                expected_available
            );
        }
        assert_eq!(updates[2].current_version, "4.35.1");
        assert_eq!(updates[2].available_version, "4.36.0");
    }

    #[test]
    fn handles_versions_longer_than_the_column_width() {
        let updates = parse_winget_upgrades(PT_BR_SAMPLE);
        let warp = updates.iter().find(|u| u.id == "Warp.Warp").expect("Warp");
        assert_eq!(warp.current_version, "v0.2026.08.05.09.03.stable_01");
        assert_eq!(warp.available_version, "v0.2026.09.30.08.29.stable_01");
    }

    #[test]
    fn keeps_less_than_version_marker_and_non_ascii_names() {
        let updates = parse_winget_upgrades(PT_BR_SAMPLE);
        let nsight = updates
            .iter()
            .find(|u| u.id == "Nvidia.Nsight.Compute")
            .expect("Nsight");
        assert_eq!(nsight.current_version, "< 2026.3.1");
        let ps = updates
            .iter()
            .find(|u| u.id.starts_with("PlayStation"))
            .expect("PlayStation");
        assert_eq!(ps.name, "PlayStation® Accessories");
    }

    #[test]
    fn ignores_footer_and_warning_lines() {
        let with_footer = format!(
            "{PT_BR_SAMPLE}57 atualizações disponíveis.\n\n1 pacotes têm números de versão que não podem ser determinados. Use --include-unknown para ver todos os resultados.\n"
        );
        let updates = parse_winget_upgrades(&with_footer);
        assert_eq!(updates.len(), 6, "rodapé e avisos não são atualizações");
    }

    #[test]
    fn works_with_english_headers() {
        let updates = parse_winget_upgrades(EN_SAMPLE);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].id, "Docker.DockerDesktop");
        assert_eq!(updates[0].current_version, "4.35.1");
        assert_eq!(updates[0].available_version, "4.36.0");
        assert_eq!(updates[1].id, "Mozilla.Firefox");
    }

    #[test]
    fn empty_table_yields_no_updates_and_no_crash() {
        let empty = "Nome                          ID                                 Versão                        Disponível                    Origem\n--------------------------------------------------------------------------------------------------------------------------------------\nNenhuma atualização de pacote disponível.\n";
        assert!(parse_winget_upgrades(empty).is_empty());
    }

    #[test]
    fn garbage_output_yields_no_updates() {
        assert!(parse_winget_upgrades("erro desconhecido").is_empty());
        assert!(parse_winget_upgrades("").is_empty());
    }

    #[test]
    fn column_offsets_require_exactly_five_columns() {
        assert!(column_offsets("Nome  ID  Versão  Disponível  Origem").is_some());
        assert!(column_offsets("Nome  ID").is_none());
        // Cabeçalho pt-BR real: acentos não podem alterar a contagem.
        let real = "Nome                                       ID                                 Versão                        Disponível                    Origem";
        let offsets = column_offsets(real).expect("5 colunas");
        assert_eq!(offsets.len(), 5);
        // O cabeçalho tem 4 colunas acentuadas (`ã`, `í`): o total em bytes
        // é MAIOR que o total em caracteres. É exatamente essa diferença que
        // corromperia o corte se os offsets fossem calculados em bytes.
        let char_len = real.chars().count();
        let byte_len = real.len();
        assert!(byte_len > char_len, "fixture deve conter acentuação");
        assert_eq!(char_len, 144);
        // 2 caracteres acentuados (`ã`, `í`) = 2 bytes extras: 144 + 2 = 146.
        assert_eq!(byte_len, 146);
    }

    #[test]
    fn slice_columns_never_splits_an_accented_character() {
        let line = "PlayStation® Accessories                   PlayStation.PlayStationAccessories 2.2.0.3                       2.2.1.2                       winget";
        let offsets = column_offsets("Nome                                       ID                                 Versão                        Disponível                    Origem").unwrap();
        let fields = slice_columns(line, &offsets);
        assert_eq!(fields[0], "PlayStation® Accessories");
        assert_eq!(fields[1], "PlayStation.PlayStationAccessories");
        assert_eq!(fields[4], "winget");
    }

    #[test]
    fn extracts_footer_total_in_multiple_locales() {
        assert_eq!(
            parse_footer_total("57 atualizações disponíveis.\n"),
            Some(57)
        );
        assert_eq!(parse_footer_total("3 upgrades available.\n"), Some(3));
        assert_eq!(parse_footer_total("sem número aqui"), None);
    }

    #[test]
    fn unavailable_snapshot_never_looks_like_zero_updates() {
        let snapshot = unavailable("winget ausente");
        assert_eq!(snapshot.total_count, 0);
        assert_eq!(
            snapshot.unavailable_reason.as_deref(),
            Some("winget ausente")
        );
    }

    #[test]
    fn accepts_real_winget_package_ids() {
        assert!(validate_package_id("Microsoft.PowerToys").is_ok());
        assert!(validate_package_id("  Docker.DockerDesktop  ").is_ok());
        // Pacotes do Microsoft Store são referenciados por ProductId numérico.
        assert!(validate_package_id("9NB1234N5L2T").is_ok());
    }

    #[test]
    fn rejects_empty_or_blank_ids() {
        assert!(validate_package_id("").is_err());
        assert!(validate_package_id("   ").is_err());
    }

    #[test]
    fn rejects_overlong_ids() {
        let long = "A".repeat(MAX_PACKAGE_ID_LEN + 1);
        assert!(validate_package_id(&long).is_err());
        // Exatamente no limite ainda é aceito.
        assert!(validate_package_id(&"A".repeat(MAX_PACKAGE_ID_LEN)).is_ok());
        // O limite conta caracteres, não bytes.
        let accents = "é".repeat(MAX_PACKAGE_ID_LEN);
        assert!(validate_package_id(&accents).is_ok());
    }

    /// O ID é repassado como argumento do subprocesso: uma aspa permitiria
    /// injetar outro argumento na linha de comando do winget.
    #[test]
    fn rejects_ids_with_quotes() {
        assert!(validate_package_id("Microsoft.Power\"Toys").is_err());
        assert!(validate_package_id("Microsoft.Power'Toys").is_err());
        assert!(validate_package_id("\"").is_err());
    }

    #[test]
    fn truncate_chars_keeps_text_short_or_marks_it() {
        assert_eq!(truncate_chars("curto", 10), "curto");
        let long = "á".repeat(50);
        let cut = truncate_chars(&long, 10);
        assert_eq!(cut.chars().filter(|c| *c == 'á').count(), 10);
        assert!(cut.ends_with("(saída truncada)"));
    }

    #[test]
    fn exit_code_requiring_admin_is_flagged() {
        // Código real do log: MSIX que registra serviço (Claude, WSL, Codex).
        let (message, needs_admin) = describe_exit_code(0x8007_3D28_u32 as i32);
        assert!(needs_admin);
        assert!(message.contains("administrador"));
        assert_eq!(format_exit_code(0x8007_3D28_u32 as i32), "0x80073D28");
    }

    #[test]
    fn known_winget_codes_are_translated_and_unknown_keeps_the_code() {
        let (message, needs_admin) = describe_exit_code(0x8A15_0101_u32 as i32);
        assert!(!needs_admin);
        assert!(message.contains("em execução"));
        let (message, _) = describe_exit_code(0x8A15_0006_u32 as i32);
        assert!(message.contains("instalador"));
        let (message, _) = describe_exit_code(0x8A15_FFFF_u32 as i32);
        assert!(message.contains("0x8A15FFFF"));
        let (message, needs_admin) = describe_exit_code(UAC_CANCELLED_EXIT_CODE);
        assert!(!needs_admin);
        assert!(message.contains("Controle de Conta de Usuário"));
        assert_eq!(format_exit_code(2), "2");
    }

    /// Regressão: a UI mostrava a tabela de pacotes como mensagem de erro.
    #[test]
    fn failure_detail_drops_table_and_progress_lines() {
        let output = format!(
            "{PT_BR_SAMPLE}██████████▒▒▒▒▒  40.0 MB / 98.5 MB\n-\nFalha na instalação com o código de saída: 0x80073d28 : Privilégios de administrador necessários\n"
        );
        let failure = UpgradeFailure::from_exit_code(Some(0x8007_3D28_u32 as i32), &output);
        assert!(failure.needs_admin);
        assert_eq!(failure.code.as_deref(), Some("0x80073D28"));
        let detail = failure.detail.expect("detalhe útil");
        assert!(detail.contains("código de saída"), "{detail}");
        assert!(
            !detail.contains("Anaconda.Anaconda3"),
            "tabela vazou: {detail}"
        );
        assert!(!detail.contains('█'), "progresso vazou: {detail}");
        assert!(relevant_output_detail(PT_BR_SAMPLE).is_none());
    }

    #[test]
    fn last_frame_keeps_only_the_final_redraw() {
        assert_eq!(last_frame("  10%\r  50%\r Instalado\r"), Some("Instalado"));
        assert_eq!(last_frame("   \r  "), None);
        assert_eq!(last_frame("linha simples"), Some("linha simples"));
    }

    #[test]
    fn elevated_script_quotes_arguments_and_maps_uac_cancel() {
        let script = elevated_upgrade_script(r"C:\Users\o'neil\winget.exe", "Anthropic.Claude");
        assert!(
            script.contains(r"-FilePath 'C:\Users\o''neil\winget.exe'"),
            "{script}"
        );
        assert!(
            script.contains("'--id','Anthropic.Claude','--exact'"),
            "{script}"
        );
        assert!(script.contains("-Verb RunAs"));
        assert!(script.contains("exit 1223"));
        assert!(!script.contains('"'), "aspas duplas quebram o argumento");
    }
}
