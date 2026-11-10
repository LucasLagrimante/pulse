//! Inventário de serviços do Windows.
//!
//! ## Por que CIM/WMI e não a API SCM
//!
//! A primeira versão usava `EnumServicesStatusExW` (advapi32), o caminho
//! "nativo". **Funciona apenas com privilégio administrativo:** medido nesta
//! máquina, `OpenSCManagerW(SC_MANAGER_CONNECT)` + `EnumServicesStatusExW`
//! devolvem `0x80070005` (ACCESS_DENIED) para usuário normal, em todas as
//! combinações de tipo de serviço testadas (`SERVICE_WIN32`, `SERVICE_DRIVER`,
//! kernel+fs+win32). O Pulse não exige elevação para ser útil, então essa API
//! não serve.
//!
//! `Get-CimInstance Win32_Service` lê os mesmos 313 serviços **sem privilégio**
//! em ~0,8 s. O custo é um `powershell.exe` sem janela
//! (`crate::process::hidden_command`), mesmo padrão de `performance.rs` e
//! `system_overview.rs`.
//!
//! ## Classificação (sistema / usuário / revisão)
//!
//! [`classify_service`] separa os serviços em três grupos a partir de fatos
//! verificáveis, nunca de um veredito sobre "malware": conta de execução
//! (`StartName` do WMI), localização do executável e fabricante declarado nos
//! recursos de versão.
//!
//! Sinais que justificam ir para "revisão" são **evidências**, e cada uma viaja
//! em `suspicionReasons` para a tela explicar o porquê. Lista vazia = sem sinal.
//! Um sinal é um convite a olhar, não uma acusação: por isso o rótulo é
//! "revisão" e a palavra malware não existe em lugar nenhum deste módulo.
//!
//! O `CompanyName` do version info é **auto-declarado** pelo fabricante — não é
//! assinatura validada (ver `docs/audit-2026-09-11/README.md`). Por isso ele
//! nunca aparece sozinho como motivo de suspeita: ausente, só pesa quando
//! combinado com um executável fora das pastas do Windows.
//!
//! ## Honestidade da leitura
//!
//! - `unavailableReason` preenchido = leitura impossível. Nunca vira lista vazia.
//! - `StartPending`/`StopPending` são distinguíveis de "Em execução".
//! - `State`/`StartMode` do WMI vêm em inglês; `describe_*` traduz para pt-BR.
//! - `category` `"unknown"` quando não há `StartName` **nem** executável: não é
//!   adivinhado para "sistema" só porque é o caso mais comum.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// TTL da enumeração. O WMI custa ~0,8 s e abre um processo; 15 s equilibra
/// frescor da lista e custo (cf. `CACHE_TTL` do overview).
const SERVICES_CACHE_TTL: Duration = Duration::from_secs(15);
/// Tempo máximo do PowerShell. Acima disso o comando é encerrado e a falha é
/// declarada, em vez de a UI esperar para sempre.
const WMI_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowsService {
    pub name: String,
    pub display_name: String,
    /// Estado em pt-BR: "Em execução", "Parado", "Pausado", "Iniciando",
    /// "Parando", "Retomando", "Pausando" ou "Estado desconhecido".
    pub state: String,
    /// Código numérico do estado, para filtrar sem reinterpretar texto.
    pub state_code: u32,
    /// Tipo de inicialização em pt-BR: "Automático", "Manual", "Desativado".
    pub start_type: String,
    pub start_type_code: u32,
    /// `Some` apenas quando o serviço realmente está executando.
    pub process_id: Option<u32>,
    pub executable_path: Option<String>,
    pub icon: Option<String>,
    /// Conta de execução do `StartName` do WMI (`LocalSystem`, `NT AUTHORITY\SYSTEM`,
    /// `DOMINIO\usuario`, ...). `None` quando o WMI não informou.
    pub start_name: Option<String>,
    /// `true` quando roda sob uma conta nominal (não as contas do Windows).
    pub is_user_service: bool,
    /// Grupo de origem: `"system"`, `"user"`, `"review"` ou `"unknown"`.
    pub category: &'static str,
    /// `CompanyName` dos recursos de versão do executável, se o arquivo existir e
    /// declarar fabricante. Não é assinatura validada — é auto-declarado.
    pub publisher: Option<String>,
    /// Evidências que levaram o serviço a `"review"`. Vazio = nenhum sinal.
    /// Sempre textual e verificável; nunca um rótulo de malware.
    pub suspicion_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesSnapshot {
    pub services: Vec<WindowsService>,
    pub running_count: usize,
    pub stopped_count: usize,
    pub paused_count: usize,
    pub total_count: usize,
    /// Preenchido quando a consulta ao SCM falhou. Nunca retorna lista vazia
    /// como se fosse "nenhum serviço": a ausência de dado é declarada.
    pub unavailable_reason: Option<String>,
    pub collected_at_unix_ms: u64,
}

struct ServicesCache {
    snapshot: ServicesSnapshot,
    fetched_at: Instant,
}

static SERVICES_CACHE: Mutex<Option<ServicesCache>> = Mutex::new(None);

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn empty_snapshot(reason: impl Into<String>) -> ServicesSnapshot {
    ServicesSnapshot {
        services: Vec::new(),
        running_count: 0,
        stopped_count: 0,
        paused_count: 0,
        total_count: 0,
        unavailable_reason: Some(reason.into()),
        collected_at_unix_ms: now_unix_ms(),
    }
}

/// Conta os estados a partir da lista — usado tanto pelo snapshot real quanto
/// pelos testes, garantindo que a métrica nunca seja um zero fictício.
pub fn tally(services: &[WindowsService]) -> (usize, usize, usize) {
    let mut running = 0;
    let mut stopped = 0;
    let mut paused = 0;
    for service in services {
        match service.state_code {
            1 => stopped += 1,
            4 => running += 1,
            7 => paused += 1,
            _ => {}
        }
    }
    (running, stopped, paused)
}

pub fn sort_services(services: &mut [WindowsService]) {
    services.sort_by(|left, right| {
        // Ativos primeiro (nos estados de espera logo depois), depois parado;
        // dentro de cada grupo,ordem alfabética por nome de exibição.
        let rank = |code: u32| match code {
            4 => 0,
            2 | 3 | 5 | 6 => 1,
            7 => 2,
            1 => 3,
            _ => 4,
        };
        rank(left.state_code)
            .cmp(&rank(right.state_code))
            .then_with(|| left.display_name.to_lowercase().cmp(&right.display_name.to_lowercase()))
    });
}

/// Traduz o `State` do WMI (inglês) para pt-BR e devolve o código numérico.
///
/// `StartPending`/`StopPending` são estados distintos de "Em execução": tratá-los
/// como ativos esconderia serviço ainda subindo.
pub fn describe_state(crm_state: &str) -> (String, u32) {
    match crm_state.trim() {
        "Running" => ("Em execução".to_string(), 4),
        "Stopped" => ("Parado".to_string(), 1),
        "Paused" => ("Pausado".to_string(), 7),
        "StartPending" => ("Iniciando".to_string(), 2),
        "StopPending" => ("Parando".to_string(), 3),
        "ContinuePending" => ("Retomando".to_string(), 5),
        "PausePending" => ("Pausando".to_string(), 6),
        // Estado desconhecido NÃO vira "Parado": seria uma mentira sobre o host.
        _ => ("Estado desconhecido".to_string(), 0),
    }
}

/// Traduz o `StartMode` do WMI (inglês) para pt-BR.
pub fn describe_start_type(mode: &str) -> (String, u32) {
    match mode.trim() {
        "Auto" => ("Automático".to_string(), 2),
        "Manual" => ("Manual".to_string(), 3),
        "Disabled" => ("Desativado".to_string(), 4),
        "Boot" => ("Boot".to_string(), 0),
        "System" => ("Sistema".to_string(), 1),
        _ => ("Desconhecido".to_string(), u32::MAX),
    }
}

/// Converte uma linha TSV do PowerShell em serviço.
///
/// Separador é `|` porque `PathName` do Windows traz aspas e espaços — um
/// separador por espaço seria ambíguo. São 8 campos e o `PathName` é **o último**
/// (`splitn(8)`), para que um `|` improvável dentro dele não desloque as demais
/// colunas. `StartName` e `CompanyName` são higienizados no PowerShell
/// (`-replace '\|', ' '`) pelo mesmo motivo.
///
/// `parse_service_line` sozinho não classifica: a categoria depende das raízes
/// graváveis do host, então fica para [`build_snapshot`] chamar
/// [`classify_service`] uma vez com o contexto já detectado. Manter o parse
/// determinístico é o que permite testar linhas TSV reais sem depender da
/// máquina.
pub fn parse_service_line(line: &str) -> Option<WindowsService> {
    let fields: Vec<&str> = line.splitn(8, '|').collect();
    if fields.len() < 5 {
        return None;
    }
    let name = fields[0].trim();
    if name.is_empty() {
        return None;
    }
    let display_name = fields[1].trim();
    let (state, state_code) = describe_state(fields[2]);
    let (start_type, start_type_code) = describe_start_type(fields[3]);
    // PID 0 no WMI significa "sem processo", não um processo de PID 0.
    let pid = fields[4].trim().parse::<u32>().ok().filter(|p| *p > 0);
    let text = |index: usize| -> Option<String> {
        fields
            .get(index)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let start_name = text(5);

    Some(WindowsService {
        name: name.to_string(),
        display_name: if display_name.is_empty() {
            name.to_string()
        } else {
            display_name.to_string()
        },
        state,
        state_code,
        start_type,
        start_type_code,
        process_id: pid,
        executable_path: text(7),
        icon: None,
        is_user_service: is_user_service(start_name.as_deref()),
        start_name,
        category: "unknown",
        publisher: text(6),
        suspicion_reasons: Vec::new(),
    })
}

/// Extrai o executável do `PathName`, que vem entre aspas ou com argumentos:
/// `"C:\Program Files\App\svc.exe" --service`.
pub fn extract_executable(image_path: &str) -> String {
    let trimmed = image_path.trim().trim_matches('"');
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let lower = trimmed.to_lowercase();
    if let Some(index) = lower.find(".exe") {
        return trimmed[..index + 4].to_string();
    }
    trimmed.to_string()
}

/* ------------------------------------------------------------------ */
/* Classificação: sistema / usuário / revisão                          */
/* ------------------------------------------------------------------ */

/// Contas internas do Windows. Qualquer outra `StartName` é conta nominal, ou
/// seja, o serviço roda como o usuário — não como o sistema.
///
/// O prefixo `NT AUTHORITY\` é removido antes da comparação: o WMI devolve as
/// três contas de duas formas (`LocalSystem` e `NT AUTHORITY\SYSTEM`,
/// `LocalService` e `NT AUTHORITY\LocalService`), e sem normalizar, 90 serviços
/// de sistema desta máquina foram medidos caindo em "usuário".
const SYSTEM_ACCOUNTS: [&str; 4] = ["system", "localservice", "networkservice", "localsystem"];

/// Domínio que o WMI usa para as contas internas.
const SYSTEM_ACCOUNT_DOMAIN: &str = "nt authority\\";

/// Executáveis do Windows cuja cópia em outro lugar é um forte indício — e é
/// também o que `svch0st.exe`-likes usam para se camuflar.
const WINDOWS_COMPONENTS: [&str; 12] = [
    "svchost.exe",
    "services.exe",
    "lsass.exe",
    "csrss.exe",
    "winlogon.exe",
    "smss.exe",
    "wininit.exe",
    "spoolsv.exe",
    "taskhostw.exe",
    "conhost.exe",
    "dwm.exe",
    "lsm.exe",
];

/// Distância de edição tolerada para considerar um nome "parecido" com um
/// componente do Windows.
///
/// **1, com transposição contando 1** — Damerau, não Levenshtein. Com limiar 2, o
/// português derruba o filtro: `servico.exe` está a distância 2 de
/// `services.exe` e todo serviço brasileiro chamado "servico" cairia em revisão.
/// Com 1, `scvhost.exe` (transposição) e `svch0st.exe` (dígito por letra)
/// continuam sendo pegos, e `servico.exe` não.
const LOOKALIKE_EDITS: usize = 1;

/// Raízes do host graváveis por qualquer usuário. Montadas a partir do ambiente
/// real, mas expostas como struct para que os testes usem caminhos fixos e não
/// dependam de `%APPDATA%` da máquina de teste.
///
/// `%PROGRAMDATA%` **não** entra: é machine-wide, e tratá-lo como gravável
/// marcou o Microsoft Defender, o Core Service e o SignalRgb nesta máquina —
/// 3 falsos positivos de 25. Um diretório de persistência legítimo é real, mas
/// como *sinal isolado* ele só produz barulho.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServiceTrustContext {
    /// Diretórios normalizados em minúsculas, sem barra final.
    pub user_writable_roots: Vec<String>,
}

impl ServiceTrustContext {
    /// Detecta as raízes graváveis do usuário logado.
    ///
    /// `dirs` já é dependência do projeto (`Cargo.toml`). `Downloads` entra à
    /// mão porque não existe como pasta base em `dirs`.
    pub fn detect() -> Self {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        candidates.extend(dirs::data_local_dir()); // %LOCALAPPDATA%
        candidates.extend(dirs::data_dir()); // %APPDATA%
        if let Some(home) = dirs::home_dir() {
            // %USERPROFILE% cobre o resto; Downloads entra porque `dirs` não a
            // expõe e é destino clássico de executável baixado.
            candidates.push(home.clone());
            candidates.push(home.join("Downloads"));
            candidates.push(home.join("AppData"));
        }
        candidates.extend(std::env::var_os("TEMP").map(std::path::PathBuf::from));
        candidates.extend(std::env::var_os("TMP").map(std::path::PathBuf::from));

        let mut roots = Vec::new();
        for candidate in candidates {
            push_normalized(&mut roots, &candidate.to_string_lossy());
        }
        Self {
            user_writable_roots: roots,
        }
    }

    /// Primeiro diretório gravável pelo usuário que contém `path`, se houver.
    pub fn writable_root_of(&self, path: &str) -> Option<&str> {
        let normalized = normalize_path(path);
        self.user_writable_roots
            .iter()
            .find(|root| path_is_inside(&normalized, root))
            .map(String::as_str)
    }
}

/// Minúsculas, barras invertidas e sem barra final — é o que torna a comparação
/// de caminho confiável entre o WMI (`C:\WINDOWS\System32`) e as variáveis de
/// ambiente.
fn normalize_path(path: &str) -> String {
    let mut normalized = path.trim().replace('/', "\\").to_lowercase();
    while normalized.ends_with('\\') && normalized.len() > 1 {
        normalized.pop();
    }
    normalized
}

fn push_normalized(roots: &mut Vec<String>, path: &str) {
    let normalized = normalize_path(path);
    if normalized.is_empty() || normalized == "\\" {
        return;
    }
    if !roots.iter().any(|root| root == &normalized) {
        roots.push(normalized);
    }
}

/// `child` está dentro de `root`? Compara por segmento: `C:\Users\a\AppData2`
/// NÃO está dentro de `C:\Users\a\AppData`.
fn path_is_inside(child: &str, root: &str) -> bool {
    child == root
        || (child.len() > root.len()
            && child.starts_with(root)
            && child.as_bytes().get(root.len()) == Some(&b'\\'))
}

/// Subárvores de `%LOCALAPPDATA%` que são **local de instalação por usuário**, não
/// pasta de persistência.
///
/// Medido nesta máquina: o ChatGPT é um serviço `LocalSystem` + automático em
/// `C:\Users\lucas\AppData\Local\Programs\ChatGPT\ChatGPT.exe`. Sem esta
/// exceção, o sinal forte mais forte do filtro acusaria software instalado
/// normalmente. A diferença é real e verificável: `Programs\...` é o que o
/// Windows e os instaladores usam para instalação por usuário; `%APPDATA%`,
/// `%TEMP%` e `Downloads` não recebem instalações.
const PER_USER_INSTALL_MARKERS: [&str; 3] = [
    "\\appdata\\local\\programs\\",
    "\\appdata\\local\\microsoft\\windowsapps\\",
    "\\appdata\\local\\packages\\",
];

/// Verdadeiro quando o executável está numa subárvore de instalação por usuário
/// e, portanto, não é sinal por estar em pasta gravável.
pub fn is_per_user_install_path(path: &str) -> bool {
    let normalized = normalize_path(path);
    PER_USER_INSTALL_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

/// Reconhece o grupo de serviços **por usuário** do Windows 10/11 pelo sufixo
/// `_<id da sessão>` no nome (ex.: `UserDataSvc_128821`).
///
/// Medido nesta máquina: 25 dos 30 serviços sem `StartName` seguem esse padrão
/// — o Windows cria um processo por usuário e por sessão, e o WMI não devolve a
/// conta. Sem esta regra, 10% da lista ficaria em "sem dados" mesmo sendo
/// claramente serviços de usuário.
///
/// É um fato do SO, não heurística: o padrão é o mesmo que o Gerenciador de
/// Tarefas e o Autoruns usam para identificar o grupo. Um executável malicioso
/// poderia adotar o sufixo, mas o resultado seria cair em "usuário" — que é onde
/// ele já estaria, então não abre nenhuma porta.
pub fn is_per_user_service_name(name: &str) -> bool {
    let Some((_, sufixo)) = name.rsplit_once('_') else {
        return false;
    };
    !sufixo.is_empty()
        && sufixo.len() <= 10
        && sufixo.chars().all(|c| c.is_ascii_digit())
}

/// Verdadeiro quando o `StartName` é uma das contas internas do Windows.
///
/// Compara a folha da conta: `NT AUTHORITY\LocalService`, `NT AUTHORITY\SYSTEM`
/// e `LocalSystem` são a mesma resposta com grafias diferentes.
pub fn is_system_account(start_name: Option<&str>) -> bool {
    let Some(name) = start_name else {
        return false;
    };
    let normalized = name.trim().to_lowercase();
    let leaf = normalized
        .strip_prefix(SYSTEM_ACCOUNT_DOMAIN)
        .unwrap_or(&normalized);
    SYSTEM_ACCOUNTS.contains(&leaf)
}

/// Verdadeiro quando o serviço roda sob uma conta nominal (não as do Windows).
///
/// `None` (WMI não informou) devolve `false`: não inventar vínculo com usuário
/// também é honestidade. O `category` separado é que carrega o "desconhecido".
pub fn is_user_service(start_name: Option<&str>) -> bool {
    match start_name {
        Some(name) => !name.trim().is_empty() && !is_system_account(Some(name)),
        None => false,
    }
}

/// O executável está em `System32`/`SysWOW64`?
pub fn is_windows_system_dir(path: &str) -> bool {
    let normalized = normalize_path(path);
    normalized.contains("\\windows\\system32\\") || normalized.contains("\\windows\\syswow64\\")
}

/// Distância de Damerau-Levenshtein (alinhamento ótimo) sem matriz — o pior
/// caso aqui é o nome de um executável, e é chamada uma vez por serviço.
///
/// Contar transposição como 1 é o que pega `scvhost.exe`; com Levenshtein puro
/// ela custaria 2 e escaparia do limiar.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let mut previous_previous: Vec<usize> = vec![0; right.len() + 1];
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            let mut best = (previous[j + 1] + 1).min(current[j] + 1).min(previous[j] + cost);
            if i > 0 && j > 0 && *left_char == right[j - 1] && left[i - 1] == *right_char {
                best = best.min(previous_previous[j - 1] + 1);
            }
            current[j + 1] = best;
        }
        std::mem::swap(&mut previous_previous, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// Componente do Windows com nome quase idêntico ao do executável, se houver.
pub fn windows_component_lookalike(executable: &str) -> Option<&'static str> {
    let file_name = executable
        .rsplit('\\')
        .next()
        .unwrap_or(executable)
        .trim()
        .to_lowercase();
    if file_name.is_empty() {
        return None;
    }
    WINDOWS_COMPONENTS.iter().copied().find(|component| {
        // Nome idêntico fora do System32 também é indício (cópia/relocation),
        // então a distância 0 conta.
        edit_distance(&file_name, component) <= LOOKALIKE_EDITS
    })
}

/// Resultado da classificação de um serviço.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    /// `"system"`, `"user"`, `"review"` ou `"unknown"`.
    pub category: &'static str,
    pub is_user_service: bool,
    pub reasons: Vec<String>,
}

/// Classifica um serviço em sistema / usuário / revisão a partir de fatos.
///
/// ## Só sinal forte leva a "revisão"
///
/// Medido nesta máquina com a primeira versão, que tratava qualquer coincidência
/// como sinal: **25 de 313 serviços (8%) caíram em revisão** e 21 deles eram
/// software legítimo — ChatGPT, Claude, WSL Service, Google Updater, AnyDesk,
/// Edge Update. Dois motivos, ambos corrigidos aqui:
///
/// 1. "Automático + conta de sistema + fora das pastas do Windows" é a
///    descrição de *todo* software instalado. Não é sinal, é o caso comum.
/// 2. "Sem fabricante declarado" aparece em drivers e serviços reais
///    (Easy Anti-Cheat, Corsair Device Listing, Grafica SignalRgb).
///
/// Portanto **revisão exige um sinal forte** — executável em diretório
/// gravável pelo usuário, ou nome imitando componente do Windows. Fatos
/// corroborantes (conta de sistema, inicialização automática, fabricante
/// ausente) **acrescentam** motivos quando um sinal forte já existe, mas nunca
/// abrem a porta sozinhos. É a diferença entre um filtro útil e alarme falso.
///
/// ## Precedência
///
/// Revisão vence sobre usuário. Um serviço de usuário rodando de `%APPDATA%` é
/// as duas coisas, e a informação que o usuário precisa primeiro é o sinal —
/// `is_user_service` continua disponível para o filtro "de usuário" não o
/// perder de vista.
pub fn classify_service(service: &WindowsService, context: &ServiceTrustContext) -> Classification {
    let executable = service
        .executable_path
        .as_deref()
        .map(extract_executable)
        .filter(|path| !path.is_empty());
    let start_name = service.start_name.as_deref();
    // Grupo por usuário do Windows é reconhecido pelo nome mesmo sem `StartName`.
    let per_user_name = start_name.is_none() && is_per_user_service_name(&service.name);
    let user_account = is_user_service(start_name) || per_user_name;
    let system_account = is_system_account(start_name);

    // Sem conta e sem executável não há o que afirmar: "unknown", nunca "system".
    if start_name.is_none() && executable.is_none() {
        return Classification {
            category: "unknown",
            is_user_service: false,
            reasons: Vec::new(),
        };
    }

    let mut strong: Vec<String> = Vec::new();
    let mut corroborating: Vec<String> = Vec::new();
    let in_windows_dir = executable
        .as_deref()
        .map(is_windows_system_dir)
        .unwrap_or(false);

    if let Some(path) = executable.as_deref() {
        // Instalação por usuário não é persistência oculta: `Programs\...` é o
        // destino padrão de instalador em `%LOCALAPPDATA%`.
        if let Some(root) = context
            .writable_root_of(path)
            .filter(|_| !is_per_user_install_path(path))
        {
            strong.push(format!(
                "Executável em diretório gravável pelo usuário ({root})"
            ));
        }
        if !in_windows_dir {
            if let Some(component) = windows_component_lookalike(path) {
                strong.push(format!(
                    "Nome do executável muito parecido com um componente do Windows ({component})"
                ));
            }
            if service
                .publisher
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                corroborating.push(
                    "Executável fora das pastas do Windows sem fabricante declarado nos recursos de versão"
                        .to_string(),
                );
            }
        }
    }

    // Fatos que sozinhos não justificam alerta, mas explicam por que o sinal
    // forte importa mais.
    if system_account && service.start_type_code == 2 {
        corroborating.push(
            "Inicia automaticamente com conta de sistema, o que amplia o impacto do sinal acima"
                .to_string(),
        );
    }
    if user_account && service.start_type_code == 2 {
        corroborating.push("Inicia automaticamente na inicialização do Windows".to_string());
    }
    if per_user_name {
        corroborating.push(
            "Serviço por usuário do Windows (nome com o identificador da sessão)".to_string(),
        );
    }

    let tem_sinal_forte = !strong.is_empty();
    let mut reasons = strong;
    // Corroborantes só acompanham um sinal forte. Encher `reasons` de um serviço
    // "system" faria a tela pintar uma linha de aviso em cima da maioria da
    // lista — e aí o filtro vira decoração em vez de alerta.
    if tem_sinal_forte {
        reasons.extend(corroborating);
    }
    let category = if tem_sinal_forte {
        "review"
    } else if user_account {
        "user"
    } else if system_account {
        "system"
    } else {
        "unknown"
    };

    Classification {
        category,
        is_user_service: user_account,
        reasons,
    }
}

/// Comando PowerShell que emite uma linha TSV por serviço.
///
/// `ForEach-Object` + `'{0}|{1}' -f` evita o custo e a fragilidade do
/// `ConvertTo-Json`, que serializa centenas de objetos para depois re-parsear.
/// `Sort-Object Name` deixa a saída determinística (útil para testes e diff).
///
/// `$OutputEncoding`/`[Console]::OutputEncoding` são UTF-8 **de propósito**:
/// sem isso o PowerShell grava em CP1252 e "Usuário" volta como `Usu?rio`
/// (observado na tela). `-NonInteractive` impede prompt de console.
///
/// São 8 campos, com `PathName` por último: `splitn(8)` no Rust garante que um
/// `|` dentro do caminho não desloque as demais colunas.
///
/// `CompanyName` é lido **só** para executáveis fora de `System32`/`SysWOW64`.
/// Abrir os recursos de versão dos ~300 binários do Windows custaria I/O sem
/// ganho real: dentro do System32 o sinal não pesa (ver `classify_service`).
const ENUMERATE_COMMAND: &str = r#"
$ErrorActionPreference='Stop'
$OutputEncoding=[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false)
Get-CimInstance Win32_Service -ErrorAction Stop |
Sort-Object -Property Name |
ForEach-Object {
  $image = [string]$_.PathName
  $lower = $image.ToLowerInvariant()
  $company = ''
  if ($lower -notlike '*\windows\system32\*' -and $lower -notlike '*\windows\syswow64\*') {
    $exe = $image.Trim()
    if ($exe.StartsWith('"')) {
      $end = $exe.IndexOf('"', 1)
      if ($end -gt 1) { $exe = $exe.Substring(1, $end - 1) }
    } else {
      $exe = ($exe -split '\s+')[0]
    }
    if ($exe) {
      try {
        $info = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($exe)
        if ($info.CompanyName) { $company = [string]$info.CompanyName }
      } catch {}
    }
  }
  '{0}|{1}|{2}|{3}|{4}|{5}|{6}|{7}' -f $_.Name, $_.DisplayName, $_.State, $_.StartMode, $_.ProcessId, (([string]$_.StartName) -replace '\|', ' '), (($company -replace '\|', ' ') -replace '\s+', ' ').Trim(), $image }
"#;

#[cfg(windows)]
mod platform {
    use super::*;

    pub fn enumerate() -> Result<Vec<WindowsService>, String> {
        let mut child = match crate::process::hidden_command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", ENUMERATE_COMMAND])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return Err(format!("Não foi possível iniciar o PowerShell: {error}")),
        };

        // Espera com timeout: um WMI travado não pode segurar a UI para sempre.
        let deadline = Instant::now() + WMI_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(
                            "O Windows demorou demais para responder sobre os serviços".to_string()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
                Err(error) => return Err(format!("Falha ao ler a lista de serviços: {error}")),
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|error| format!("Falha ao coletar a saída do WMI: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let services: Vec<WindowsService> =
            stdout.lines().filter_map(parse_service_line).collect();

        if services.is_empty() {
            // Vazio COM falha é dado indisponível: declarar é obrigatório para
            // não parecer que a máquina não tem serviços.
            let reason = if output.status.success() {
                "O Windows não retornou detalhes de serviços (WMI indisponível)".to_string()
            } else {
                let detail = stderr
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("código de saída desconhecido");
                format!("Falha ao ler os serviços do Windows: {detail}")
            };
            return Err(reason);
        }

        Ok(services)
    }

    /// Inicia ou para um serviço via `sc.exe`.
    ///
    /// `StartService`/`ControlService` do SCM exigiriam elevação (medido:
    /// ACCESS_DENIED para usuário normal), mas `sc.exe start/stop` delega ao
    /// mesmo serviço e o Windows concede a operação a serviços de usuário sem
    /// admin. A recusa do Windows é devolvida como mensagem — nunca engolida.
    pub fn control(name: &str, action: &str) -> Result<(), String> {
        let verb = match action {
            "start" => "start",
            "stop" => "stop",
            other => return Err(format!("Ação de serviço desconhecida: {other}")),
        };
        let output = crate::process::hidden_command("sc.exe")
            .args([verb, name])
            .output()
            .map_err(|error| format!("Não foi possível executar o sc.exe: {error}"))?;

        if output.status.success() {
            return Ok(());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            format!("código de saída {}", output.status.code().unwrap_or(-1))
        };
        Err(format!("O Windows recusou a operação: {detail}"))
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn enumerate() -> Result<Vec<WindowsService>, String> {
        Err("A leitura de serviços só é suportada no Windows".to_string())
    }

    pub fn control(_name: &str, _action: &str) -> Result<(), String> {
        Err("O controle de serviços só é suportado no Windows".to_string())
    }
}

fn build_snapshot(services: Vec<WindowsService>) -> ServicesSnapshot {
    build_snapshot_with_context(services, ServiceTrustContext::detect())
}

/// Igual a [`build_snapshot`], mas com contexto explícito — os testes precisam
/// de raízes fixas, e uma detecção de ambiente por snapshot é o que mantém a
/// classificação determinística fora da máquina de teste.
fn build_snapshot_with_context(
    mut services: Vec<WindowsService>,
    context: ServiceTrustContext,
) -> ServicesSnapshot {
    for service in services.iter_mut() {
        if let Some(path) = service.executable_path.as_ref() {
            service.icon = crate::app_icons::icon_for_command(path);
        }
        let classification = classify_service(service, &context);
        service.is_user_service = classification.is_user_service;
        service.category = classification.category;
        service.suspicion_reasons = classification.reasons;
    }
    sort_services(&mut services);
    let (running, stopped, paused) = tally(&services);
    let total = services.len();
    ServicesSnapshot {
        services,
        running_count: running,
        stopped_count: stopped,
        paused_count: paused,
        total_count: total,
        unavailable_reason: None,
        collected_at_unix_ms: now_unix_ms(),
    }
}

/// Retorna do cache quando válido. Chamadas repetidas dentro do TTL não
/// abrem o SCM de novo.
fn cached() -> Option<ServicesSnapshot> {
    let guard = SERVICES_CACHE.lock().ok()?;
    let cached = guard.as_ref()?;
    if cached.fetched_at.elapsed() < SERVICES_CACHE_TTL {
        return Some(cached.snapshot.clone());
    }
    None
}

fn store(snapshot: &ServicesSnapshot) {
    if let Ok(mut guard) = SERVICES_CACHE.lock() {
        *guard = Some(ServicesCache {
            snapshot: snapshot.clone(),
            fetched_at: Instant::now(),
        });
    }
}

#[tauri::command]
pub async fn list_windows_services(force_refresh: Option<bool>) -> ServicesSnapshot {
    if !force_refresh.unwrap_or(false) {
        if let Some(snapshot) = cached() {
            return snapshot;
        }
    }

    let snapshot = match tauri::async_runtime::spawn_blocking(platform::enumerate).await {
        Ok(Ok(services)) => build_snapshot(services),
        Ok(Err(reason)) => empty_snapshot(reason),
        Err(_) => empty_snapshot("A coleta de serviços foi interrompida"),
    };
    store(&snapshot);
    snapshot
}

/// Snapshot para a análise com IA: reaproveita o último coletado mesmo fora do
/// TTL (é a lista que o usuário está vendo na tela) e só enumera se não houver.
pub async fn snapshot_for_analysis() -> ServicesSnapshot {
    let last = SERVICES_CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|cache| cache.snapshot.clone()))
        .filter(|snapshot| snapshot.unavailable_reason.is_none());
    match last {
        Some(snapshot) => snapshot,
        None => list_windows_services(Some(false)).await,
    }
}

/// Inicia ou para um serviço. Operação restrita: só atua em serviços de
/// usuário e devolve a mensagem de erro original quando o Windows recusa.
#[tauri::command]
pub async fn set_windows_service_state(
    name: String,
    action: String,
) -> Result<ServicesSnapshot, String> {
    let target = name.trim().to_string();
    if target.is_empty() {
        return Err("Serviço não informado".to_string());
    }
    if action != "start" && action != "stop" {
        return Err("Ação de serviço desconhecida".to_string());
    }
    // O nome vai como argumento separado do `sc.exe`; aspas seriam literais e um
    // nome gigantesco não é serviço. Rejeitar aqui é mais barato que no SO.
    if target.contains('"') || target.chars().count() > 256 {
        return Err("Nome de serviço inválido".to_string());
    }

    tauri::async_runtime::spawn_blocking(move || platform::control(&target, &action))
        .await
        .map_err(|_| "Falha ao executar a ação de serviço".to_string())??;

    // Invalida o cache: o estado exibido precisa refletir a mudança.
    if let Ok(mut guard) = SERVICES_CACHE.lock() {
        *guard = None;
    }
    Ok(list_windows_services(Some(true)).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, state_code: u32) -> WindowsService {
        WindowsService {
            name: name.to_string(),
            display_name: name.to_string(),
            state: String::new(),
            state_code,
            start_type: "Automático".to_string(),
            start_type_code: 2,
            process_id: None,
            executable_path: None,
            icon: None,
            start_name: None,
            is_user_service: false,
            category: "unknown",
            publisher: None,
            suspicion_reasons: Vec::new(),
        }
    }

    /// Contexto com raízes fixas: os testes não podem depender de `%APPDATA%`
    /// da máquina em que rodam.
    fn trust_context() -> ServiceTrustContext {
        // Espelha o que `detect()` monta de verdade, sem `%PROGRAMDATA%` — que é
        // machine-wide e faria o Defender cair em revisão.
        ServiceTrustContext {
            user_writable_roots: vec![
                r"c:\users\lucas\appdata\local".to_string(),
                r"c:\users\lucas\appdata\roaming".to_string(),
                r"c:\users\lucas\downloads".to_string(),
                r"c:\users\lucas".to_string(),
                r"c:\temp".to_string(),
            ],
        }
    }

    /// Monta um serviço de teste com os campos que a classificação consome.
    fn classifiable(
        start_name: Option<&str>,
        executable: Option<&str>,
        start_type_code: u32,
        publisher: Option<&str>,
    ) -> WindowsService {
        let mut svc = service("Alvo", 4);
        svc.start_name = start_name.map(str::to_string);
        svc.executable_path = executable.map(str::to_string);
        svc.start_type_code = start_type_code;
        svc.publisher = publisher.map(str::to_string);
        svc
    }

    #[test]
    fn translates_every_wmi_state() {
        assert_eq!(describe_state("Running"), ("Em execução".to_string(), 4));
        assert_eq!(describe_state("Stopped"), ("Parado".to_string(), 1));
        assert_eq!(describe_state("Paused"), ("Pausado".to_string(), 7));
        assert_eq!(describe_state("StartPending"), ("Iniciando".to_string(), 2));
        assert_eq!(describe_state("StopPending"), ("Parando".to_string(), 3));
        assert_eq!(describe_state("ContinuePending"), ("Retomando".to_string(), 5));
        assert_eq!(describe_state("PausePending"), ("Pausando".to_string(), 6));
    }

    /// Estado desconhecido NÃO pode virar "Parado": mentiria sobre o host.
    #[test]
    fn unknown_state_is_exposed_not_disguised_as_stopped() {
        let (label, code) = describe_state("SomethingElse");
        assert_eq!(label, "Estado desconhecido");
        assert_eq!(code, 0);
        assert_ne!(label, "Parado");
    }

    #[test]
    fn translates_wmi_start_modes() {
        assert_eq!(describe_start_type("Auto"), ("Automático".to_string(), 2));
        assert_eq!(describe_start_type("Manual"), ("Manual".to_string(), 3));
        assert_eq!(describe_start_type("Disabled"), ("Desativado".to_string(), 4));
        assert_eq!(describe_start_type("Boot").1, 0);
        assert_eq!(describe_start_type("System").1, 1);
        assert_eq!(describe_start_type("?").1, u32::MAX);
    }

    /// Linha TSV no formato exato do `ENUMERATE_COMMAND` (8 campos,
    /// `PathName` por último). Os valores de `StartName`/`CompanyName` são os
    /// que o WMI devolve para `Appinfo` nesta máquina.
    #[test]
    fn parses_a_real_tsv_line_with_quoted_path() {
        let line = concat!(
            r#"Appinfo|Application Information|Running|Auto|11380|"#,
            r#"LocalSystem||Microsoft Corporation|"#,
            r#""C:\WINDOWS\system32\svchost.exe" -k netsvcs -p"#,
        );
        let parsed = parse_service_line(line).expect("linha válida");
        assert_eq!(parsed.name, "Appinfo");
        assert_eq!(parsed.display_name, "Application Information");
        assert_eq!(parsed.state, "Em execução");
        assert_eq!(parsed.state_code, 4);
        assert_eq!(parsed.start_type, "Automático");
        assert_eq!(parsed.process_id, Some(11380));
        assert_eq!(parsed.start_name.as_deref(), Some("LocalSystem"));
        assert!(!parsed.is_user_service);
        assert!(parsed
            .executable_path
            .as_deref()
            .unwrap_or_default()
            .contains("svchost.exe"));
    }

    /// `CompanyName` vazio e `PathName` ausente continuam produzindo `None` —
    /// campo ausente vira `None`, nunca string vazia.
    #[test]
    fn missing_optional_fields_become_none_not_empty_string() {
        let line = r#"Spooler|Print Spooler|Stopped|Auto|0|LocalSystem||C:\WINDOWS\System32\spoolsv.exe"#;
        let parsed = parse_service_line(line).expect("linha válida");
        assert_eq!(parsed.publisher, None);
        assert_eq!(parsed.start_name.as_deref(), Some("LocalSystem"));
        assert!(parsed.suspicion_reasons.is_empty());
    }

    /// `splitn(8)` é o que garante que um `|` dentro do `PathName` não come as
    /// colunas da frente: `StartName` continua legível.
    #[test]
    fn pipe_inside_path_does_not_shift_previous_columns() {
        let line = concat!(
            "Weird|Weird Service|Running|Auto|42|LocalSystem||",
            r#""C:\Temp\a|b.exe" --run"#,
        );
        let parsed = parse_service_line(line).expect("linha válida");
        assert_eq!(parsed.start_name.as_deref(), Some("LocalSystem"));
        assert_eq!(parsed.process_id, Some(42));
        assert_eq!(
            parsed.executable_path.as_deref(),
            Some(r#""C:\Temp\a|b.exe" --run"#)
        );
    }

    /// Acentos no `StartName` (`DOMINIO\joão.silva`) precisam sobreviver ao
    /// CP1252 do PowerShell, senão a separação usuário/sistema fica silenciosamente
    /// errada — o serviço cairia em "sistema" por `LocalSystem` nunca casar.
    #[test]
    fn preserves_accented_start_name_as_user_account() {
        let line = r#"MeuSvc|Meu Serviço|Running|Auto|77|DOMAINO\joão.silva|ACME|"C:\Program Files\ACME\svc.exe""#;
        let parsed = parse_service_line(line).expect("linha válida");
        assert!(!parsed.start_name.as_deref().unwrap_or_default().contains('\u{FFFD}'));
        assert_eq!(parsed.start_name.as_deref(), Some("DOMAINO\\joão.silva"));
        assert!(parsed.is_user_service);
        assert_eq!(parsed.publisher.as_deref(), Some("ACME"));
    }

    /// PID 0 no WMI significa "sem processo" — nunca um processo de PID 0.
    #[test]
    fn stopped_service_has_no_pid_instead_of_pid_zero() {
        let line = r"Spooler|Print Spooler|Stopped|Auto|0|LocalSystem||C:\WINDOWS\System32\spoolsv.exe";
        let parsed = parse_service_line(line).expect("linha válida");
        assert_eq!(parsed.state_code, 1);
        assert_eq!(parsed.process_id, None);
    }

    #[test]
    fn rejects_blank_and_malformed_lines() {
        assert!(parse_service_line("").is_none());
        assert!(parse_service_line("   |   |   ").is_none());
        assert!(parse_service_line("so|campos").is_none());
        assert!(parse_service_line("|Nome|Running|Auto|1|x").is_none());
    }

    #[test]
    fn empty_display_name_falls_back_to_service_name() {
        let line = "Fax|Fax||Running|Manual|0|LocalSystem||";
        let parsed = parse_service_line(line).expect("linha válida");
        assert_eq!(parsed.display_name, "Fax");
    }

    #[test]
    fn extracts_executable_from_quoted_and_bare_paths() {
        assert_eq!(
            extract_executable(r#"C:\Program Files\App\svc.exe" --service"#),
            r"C:\Program Files\App\svc.exe"
        );
        assert_eq!(
            extract_executable(r"C:\Windows\System32\svchost.exe -k netsvcs"),
            r"C:\Windows\System32\svchost.exe"
        );
        assert_eq!(extract_executable("   "), "");
    }

    #[test]
    fn counts_states_separately() {
        let services = vec![
            service("Audio", 4),
            service("Spooler", 4),
            service("BITS", 1),
            service("SysMain", 7),
            service("Transicao", 2),
        ];
        assert_eq!(tally(&services), (2, 1, 1));
    }

    #[test]
    fn sorts_running_then_transition_then_stopped() {
        let mut services = vec![
            service("Zeta Parado", 1),
            service("Alfa Rodando", 4),
            service("Beta Iniciando", 2),
            service("Gama Pausado", 7),
        ];
        sort_services(&mut services);
        let order: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            order,
            vec!["Alfa Rodando", "Beta Iniciando", "Gama Pausado", "Zeta Parado"]
        );
    }

    #[test]
    fn snapshot_totals_match_the_service_list() {
        let snapshot = build_snapshot(vec![service("A", 4), service("B", 1)]);
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(snapshot.running_count, 1);
        assert_eq!(snapshot.stopped_count, 1);
        assert!(snapshot.unavailable_reason.is_none());
    }

    #[test]
    fn failed_collection_declares_reason_instead_of_empty_success() {
        let snapshot = empty_snapshot("WMI indisponível");
        assert_eq!(snapshot.total_count, 0);
        assert_eq!(
            snapshot.unavailable_reason.as_deref(),
            Some("WMI indisponível"),
            "falha de coleta precisa ser declarada, nunca mascarada"
        );
    }

    /* ---------------- classificação ---------------- */

    #[test]
    fn accounts_of_windows_are_not_user_services() {
        // O WMI grafou estas contas de duas formas na mesma máquina; as duas
        // precisam casar, senão 90 serviços de sistema viram "de usuário".
        for account in [
            "LocalSystem",
            "localsystem",
            "LocalService",
            "NetworkService",
            "NT AUTHORITY\\SYSTEM",
            "NT AUTHORITY\\LocalService",
            "NT Authority\\NetworkService",
            "nt authority\\system",
        ] {
            assert!(is_system_account(Some(account)), "{account} deveria ser conta do sistema");
            assert!(!is_user_service(Some(account)), "{account} não é conta de usuário");
        }
        assert!(is_user_service(Some("LUCAS-PC\\lucas")));
        assert!(is_user_service(Some("DOMINIO\\joão.silva")));
        // Sem StartName não se inventa vínculo com usuário.
        assert!(!is_user_service(None));
        assert!(!is_system_account(None));
        assert!(!is_user_service(Some("   ")));
    }

    #[test]
    fn system_service_in_system32_is_not_flagged_for_review() {
        let svc = classifiable(
            Some("LocalSystem"),
            Some(r#"C:\WINDOWS\system32\svchost.exe" -k netsvcs"#),
            2,
            None,
        );
        let result = classify_service(&svc, &trust_context());
        assert_eq!(result.category, "system", "motivos: {:?}", result.reasons);
        assert!(result.reasons.is_empty());
    }

    /// Serviço de usuário instalado normalmente não pode virar "revisão": se
    /// assim fosse, o filtro viraria alarme falso.
    #[test]
    fn installed_user_service_stays_in_user_category() {
        let svc = classifiable(
            Some("LUCAS-PC\\lucas"),
            Some(r"C:\Program Files\MeuApp\servico.exe"),
            3,
            Some("MeuApp Ltda"),
        );
        let result = classify_service(&svc, &trust_context());
        assert_eq!(result.category, "user", "motivos: {:?}", result.reasons);
        assert!(result.is_user_service);
        assert!(result.reasons.is_empty());
    }

    /// Regressão de alarme falso em português: `servico.exe` está a distância
    /// de edição 2 (Levenshtein) de `services.exe`. Com limiar 2, todo
    /// chamado "servico" cairia em revisão — o filtro precisa ser insensível ao
    /// idioma do usuário.
    #[test]
    fn portuguese_service_name_is_not_a_windows_component_lookalike() {
        assert_eq!(
            windows_component_lookalike(r"C:\Program Files\Loja\servico.exe"),
            None,
            "'servico.exe' é português, não imitação de services.exe"
        );
        assert_eq!(classify_service(
            &classifiable(
                Some("LUCAS-PC\\lucas"),
                Some(r"C:\Program Files\Loja\servico.exe"),
                3,
                Some("Loja")
            ),
            &trust_context(),
        )
        .category, "user");
    }

    /// Transposição tem que contar 1: `scvhost.exe` é camuflagem clássica e
    /// custaria 2 em Levenshtein puro.
    #[test]
    fn transposition_letter_swap_counts_as_one_edit() {
        assert_eq!(
            windows_component_lookalike(r"C:\Temp\scvhost.exe"),
            Some("svchost.exe")
        );
    }

    /// O caminho que importa é o executável, não o `PathName` cru: um serviço
    /// em `%APPDATA%` é sinal mesmo com argumentos no final.
    #[test]
    fn writable_directory_flags_review_with_honest_reason() {
        let svc = classifiable(
            Some("LocalSystem"),
            Some(r#""C:\Users\lucas\AppData\Roaming\algo\svc.exe" --run"#),
            3,
            Some("Algo"),
        );
        let result = classify_service(&svc, &trust_context());
        assert_eq!(result.category, "review");
        assert!(
            result
                .reasons
                .iter()
                .any(|reason| reason.contains("gravável pelo usuário")
                    && reason.contains(r"c:\users\lucas\appdata\roaming")),
            "motivos: {:?}",
            result.reasons
        );
    }

    /// Fabricante ausente **não** leva a revisão sozinho: medido nesta máquina,
/// Easy Anti-Cheat, Corsair Device Listing e Grafana SignalRgb não declaram
/// `CompanyName` e são legítimos. Vira corroborante de um sinal forte.
#[test]
    fn missing_publisher_alone_does_not_flag_review() {
        let fora = classifiable(
            Some("LocalSystem"),
            Some(r"C:\Program Files\App\svc.exe"),
            3,
            None,
        );
        assert_eq!(classify_service(&fora, &trust_context()).category, "system");

        // Junto com um sinal forte, o mesmo binário entra em revisão e o
        // fabricante ausente vira motivo adicional.
        let combo = classifiable(
            Some("LocalSystem"),
            Some(r"C:\Users\lucas\AppData\Local\svc.exe"),
            2,
            None,
        );
        let resultado = classify_service(&combo, &trust_context());
        assert_eq!(resultado.category, "review");
        assert!(resultado.reasons.iter().any(|r| r.contains("sem fabricante")));
        assert!(resultado.reasons.iter().any(|r| r.contains("conta de sistema")));
    }

    /// Regressão medida nesta máquina: com `%PROGRAMDATA%` na lista de pastas
    /// graváveis, o Windows Defender, o Defender Core Service e o SignalRgb
    /// caíam em revisão. `%PROGRAMDATA%` é machine-wide e saiu da lista.
    #[test]
    fn programdata_services_are_not_flagged_as_user_writable() {
        for (nome, caminho) in [
            ("WinDefend", r"C:\ProgramData\Microsoft\Windows Defender\Platform\4.18\MpCmdRun.exe"),
            ("Sense", r"C:\ProgramData\Microsoft\Windows Defender\Platform\MsSense.exe"),
            ("SignalRgb", r"C:\ProgramData\SignalRgb\SignalRgb.Service.exe"),
        ] {
            let svc = classifiable(Some("LocalSystem"), Some(caminho), 2, Some("Microsoft Corporation"));
            let resultado = classify_service(&svc, &trust_context());
            assert_eq!(
                resultado.category, "system",
                "{nome} não pode cair em revisão: {resultado:?}"
            );
        }
        assert!(
            !ServiceTrustContext::detect().user_writable_roots
                .iter()
                .any(|root| root.contains("programdata")),
            "%PROGRAMDATA% não é raiz gravável por usuário neste filtro"
        );
    }

    /// Regressão medida: "automático + LocalSystem + fora do Windows" marcava
    /// 21 serviços legítimos (ChatGPT, Claude, WSL Service, Google Updater,
    /// AnyDesk, Edge Update). Descrever software instalado não é sinal.
    #[test]
    fn auto_start_system_account_outside_windows_is_not_review_on_its_own() {
        for (nome, caminho, fabricante) in [
            // ChatGPT é `LocalSystem` + automático **e** mora em pasta gravável:
            // os dois motivos de alerta caem, e ainda assim é software legítimo.
            ("ChatGPT", r"C:\Users\lucas\AppData\Local\Programs\ChatGPT\ChatGPT.exe", None),
            ("WSLService", r"C:\Program Files\WSL\wslservice.exe", Some("Microsoft Corporation")),
            ("GoogleUpdater", r"C:\Program Files\Google\Update\GoogleUpdater.exe", None),
            ("AnyDesk", r"C:\Program Files (x86)\AnyDesk\AnyDesk.exe", Some("AnyDesk Software GmbH")),
        ] {
            let mut svc = classifiable(Some("LocalSystem"), Some(caminho), 2, fabricante);
            svc.name = nome.to_string();
            let resultado = classify_service(&svc, &trust_context());
            assert_ne!(
                resultado.category, "review",
                "{nome} é software legítimo e entrou em revisão: {:?}",
                resultado.reasons
            );
        }
    }

    /// A exceção é só para `Programs\...`. Um serviço jogado direto em
    /// `%APPDATA%` continua sendo sinal — o filtro não pode virar "ignorei
    /// qualquer coisa em AppData".
    #[test]
    fn per_user_install_exception_is_limited_to_install_subtrees() {
        assert!(is_per_user_install_path(
            r"C:\Users\lucas\AppData\Local\Programs\ChatGPT\ChatGPT.exe"
        ));
        assert!(is_per_user_install_path(
            r"C:\Users\lucas\AppData\Local\Microsoft\WindowsApps\App.exe"
        ));
        assert!(!is_per_user_install_path(
            r"C:\Users\lucas\AppData\Roaming\algo\svc.exe"
        ));
        let roaming = classifiable(
            Some("LocalSystem"),
            Some(r"C:\Users\lucas\AppData\Roaming\algo\svc.exe"),
            2,
            None,
        );
        assert_eq!(classify_service(&roaming, &trust_context()).category, "review");
    }

    #[test]
    fn lookalike_of_windows_component_is_flagged() {
        assert_eq!(
            windows_component_lookalike(r"C:\Temp\svch0st.exe"),
            Some("svchost.exe")
        );
        assert_eq!(
            windows_component_lookalike(r"C:\Temp\lsass.exe"),
            Some("lsass.exe")
        );
        assert_eq!(windows_component_lookalike(r"C:\Temp\helper.exe"), None);
        assert_eq!(windows_component_lookalike(""), None);
    }

    /// Precedência documentada: revisão vence, mas o serviço continua marcado
    /// como de usuário para o filtro correspondente não o perder.
    #[test]
    fn review_wins_over_user_but_keeps_the_user_flag() {
        let svc = classifiable(
            Some("LUCAS-PC\\lucas"),
            Some(r"C:\Users\lucas\Downloads\svchost.exe"),
            3,
            None,
        );
        let result = classify_service(&svc, &trust_context());
        assert_eq!(result.category, "review");
        assert!(result.is_user_service);
        assert!(result.reasons.len() >= 2);
    }

    /// `AppData2` não está dentro de `AppData` — comparação por segmento.
    #[test]
    fn writable_root_match_respects_path_segments() {
        // Só as raízes que interessam ao teste: sem `c:\users\lucas` no meio,
        // `AppData2` seria filho de `AppData` por engano de string.
        let context = ServiceTrustContext {
            user_writable_roots: vec![r"c:\users\lucas\appdata\local".to_string()],
        };
        assert!(context
            .writable_root_of(r"C:\Users\lucas\AppData\Local\Temp\svc.exe")
            .is_some());
        assert!(context
            .writable_root_of(r"C:\Users\lucas\AppData\Local2\svc.exe")
            .is_none());
        assert!(context.writable_root_of(r"C:\Windows\System32\svc.exe").is_none());
        // Barra normalizada: o WMI às vezes emite "/" e caminho com barra final.
        assert!(trust_context()
            .writable_root_of("C:/Temp/vendor/")
            .is_some());
    }

    /// Sem `StartName` nem executável o grupo é "unknown", nunca "system".
    #[test]
    fn missing_everything_is_unknown_not_system() {
        let result = classify_service(&service("SemDados", 1), &trust_context());
        assert_eq!(result.category, "unknown");
        assert!(result.reasons.is_empty());
    }

    /// `build_snapshot` classifica a lista inteira com um único contexto.
    #[test]
    fn snapshot_classifies_every_service() {
        let mut sistema = classifiable(Some("LocalSystem"), Some(r"C:\Windows\System32\svchost.exe"), 2, None);
        sistema.name = "Sistema".to_string();
        let mut usuario = classifiable(Some("LUCAS-PC\\lucas"), Some(r"C:\Program Files\App\svc.exe"), 3, Some("App"));
        usuario.name = "Usuario".to_string();
        let mut revisao = classifiable(Some("LocalSystem"), Some(r"C:\Temp\wmiapv.exe"), 3, Some("X"));
        revisao.name = "Revisao".to_string();

        let snapshot = build_snapshot_with_context(vec![sistema, usuario, revisao], trust_context());
        let by_name = |name: &str| {
            snapshot
                .services
                .iter()
                .find(|svc| svc.name == name)
                .unwrap_or_else(|| panic!("{name} ausente do snapshot"))
        };
        assert_eq!(by_name("Sistema").category, "system");
        assert_eq!(by_name("Usuario").category, "user");
        assert!(by_name("Usuario").is_user_service);
        assert_eq!(by_name("Revisao").category, "review");
        assert!(!by_name("Revisao").suspicion_reasons.is_empty());
        assert_eq!(snapshot.total_count, 3);
    }

    /* ---------------- leitura real ---------------- */

    /// O grupo por usuário do Windows é reconhecido pelo sufixo `_<sessão>`,
    /// mesmo sem `StartName` — são 25 dos 30 serviços sem conta nesta máquina.
    #[test]
    fn per_user_group_is_recognized_by_session_suffix() {
        for nome in [
            "UserDataSvc_128821",
            "WpnUserService_128821",
            "cbdhsvc_128821",
            "DevicesFlowUserSvc_128821",
        ] {
            assert!(is_per_user_service_name(nome), "{nome} deveria ser grupo por usuário");
        }
        for nome in ["AudioSrv", "Appinfo", "meu_servico", "svc_", "svc_abc", "svc_12345678901"] {
            assert!(!is_per_user_service_name(nome), "{nome} não é grupo por usuário");
        }

        let mut svc = classifiable(
            None,
            Some(r"C:\WINDOWS\system32\svchost.exe -k UnistackSvcGroup"),
            3,
            None,
        );
        svc.name = "UserDataSvc_128821".to_string();
        let resultado = classify_service(&svc, &trust_context());
        assert_eq!(resultado.category, "user");
        assert!(resultado.is_user_service);
        // Sem sinal forte, o serviço não entra em revisão nem ganha aviso.
        assert!(resultado.reasons.is_empty());
    }

    /// Um serviço com `StartName` **de sistema** não vira "de usuário" só porque
    /// o nome tem sufixo de sessão: a conta explícita ganha.
    #[test]
    fn explicit_system_account_beats_session_suffix() {
        let mut svc = classifiable(
            Some("LocalSystem"),
            Some(r"C:\Windows\System32\svchost.exe -k netsvcs"),
            2,
            None,
        );
        svc.name = "ImpostorSvc_128821".to_string();
        assert_eq!(classify_service(&svc, &trust_context()).category, "system");
    }

    /// Regressão de encoding: o PowerShell grava em CP1252 por padrão e
    /// "Acesso a Dados do Usuário" voltava como "Usu?rio" na tela. Este teste
    /// falha se qualquer serviço com acento for corrompido.
    #[cfg(windows)]
    #[test]
    fn real_enumeration_preserves_ported_accents() {
        let services =
            platform::enumerate().unwrap_or_else(|reason| panic!("WMI falhou: {reason}"));
        let corrupted: Vec<String> = services
            .iter()
            .filter(|service| {
                service.name.contains('\u{FFFD}') || service.display_name.contains('\u{FFFD}')
            })
            .map(|service| service.display_name.clone())
            .collect();
        assert!(
            corrupted.is_empty(),
            "acentos corrompidos pelo encoding do PowerShell: {corrupted:?}"
        );
        assert!(
            services
                .iter()
                .any(|service| service.display_name.contains("Usuário")),
            "nenhum serviço com acento em português foi lido — o texto pode estar \
             sendo decodificado com a codificação errada"
        );
    }

    /// `StartName` é a base da separação usuário/sistema: sem ele, todo serviço
/// cairia em "unknown". Alguns serviços do Windows realmente não têm conta
/// (drivers de kernel, serviços sem imagem), então o teste exige **a maioria**
/// preenchida e nenhum valor lixo — e que a leitura não dependa de elevação.
#[cfg(windows)]
    #[test]
    fn real_enumeration_reads_start_name_and_classifies() {
        let services = platform::enumerate().unwrap_or_else(|reason| panic!("WMI falhou: {reason}"));
        let with_start_name = services
            .iter()
            .filter(|svc| svc.start_name.is_some())
            .count();
        assert!(
            with_start_name * 10 >= services.len() * 9,
            "só {with_start_name} de {} serviços trouxeram StartName",
            services.len()
        );
        assert!(
            services
                .iter()
                .any(|svc| is_system_account(svc.start_name.as_deref())),
            "nenhum serviço com conta interna do Windows — StartName pode estar mal lido"
        );
        assert!(
            services
                .iter()
                .all(|svc| !svc.start_name.as_deref().unwrap_or_default().contains('\u{FFFD}')),
            "acentos corrompidos em StartName"
        );

        let snapshot = build_snapshot(services);
        let total = snapshot.total_count;
        let contagem = |categoria: &str| {
            snapshot
                .services
                .iter()
                .filter(|svc| svc.category == categoria)
                .count()
        };
        let system = contagem("system");
        let review = contagem("review");
        let unknown = contagem("unknown");

        assert!(system > 50, "só {system} serviços de sistema — sinal de campo trocado");
        // Nenhum serviço pode ficar sem categoria: sem StartName e sem
        // executável é "unknown", nunca um rótulo inventado.
        assert_eq!(system + review + contagem("user") + unknown, total);
        assert!(snapshot
            .services
            .iter()
            .all(|svc| ["system", "user", "review", "unknown"].contains(&svc.category)));
        // Motivo textual é obrigatório em revisão e proibido fora dela: um
        // "system" com motivo faria a tela avisar sobre a maioria da lista.
        for svc in &snapshot.services {
            if svc.category == "review" {
                assert!(
                    !svc.suspicion_reasons.is_empty(),
                    "{} em revisão sem motivo",
                    svc.display_name
                );
            } else {
                assert!(
                    svc.suspicion_reasons.is_empty(),
                    "{} fora de revisão mas com motivo {:?}",
                    svc.display_name,
                    svc.suspicion_reasons
                );
            }
        }
        assert!(
            snapshot.services.iter().all(|svc| !svc.suspicion_reasons.iter().any(
                |reason| {
                    let lower = reason.to_lowercase();
                    lower.contains("malware") || lower.contains("vírus")
                }
            )),
            "a lista de motivos jamais pode rotular o executável como malware"
        );
        // Limiar de alarme falso: a primeira versão marcava 8% da máquina
        // (25/313) com 21 falsos positivos. Acima de 3% o filtro parou de ser
        // útil e virou decoration.
        assert!(
            review * 100 < total * 3,
            "{review} de {total} serviços em revisão — acima do tolerável para alarme falso"
        );
    }

    /// Integração real: consulta o WMI desta máquina. Este teste é a razão de o
    /// módulo não usar a API SCM — se voltar a dar ACCESS_DENIED, ele falha.
    #[cfg(windows)]
    #[test]
    fn real_enumeration_reads_services_without_elevation() {
        let services = platform::enumerate().unwrap_or_else(|reason| panic!("WMI falhou: {reason}"));
        assert!(
            services.len() > 100,
            "um Windows real tem centenas de serviços, veio {}",
            services.len()
        );
        assert!(
            services.iter().any(|s| s.state_code == 4),
            "nenhum serviço em execução listado"
        );
        assert!(
            services.iter().all(|s| !s.name.is_empty()),
            "serviço sem nome"
        );
        assert!(
            services.iter().any(|s| s.state_code == 1),
            "nenhum serviço parado listado"
        );
    }
}
