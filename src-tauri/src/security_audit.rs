//! Auditoria determinística de acesso remoto (módulo Segurança).
//!
//! Responde de forma verificável e sem falso alarme à pergunta:
//! "Há alguém conectado à minha tela ou alguma porta/agente de acesso remoto ativo?"
//!
//! Princípios:
//! - Evidência, nunca rótulo de malware.
//! - Zero dados fictícios: qualquer checagem que falhe devolve `unavailableReason`
//!   e impede veredito `clean` (devolve `partial`).
//! - Sem PowerShell no caminho principal no Windows: Registro (RegOpenKeyExW),
//!   SCM (QueryServiceStatusEx), tabela TCP do kernel (GetExtendedTcpTable),
//!   Toolhelp32 (CreateToolhelp32Snapshot) e `qwinsta.exe` com janela oculta.
//! - Diferenciação explícita de `msrdc.exe` dentro da pasta do WSL (`wslg_internal`)
//!   versus clientes RDP (`mstsc`/`msrdcw`) e agentes de terceiros (AnyDesk,
//!   TeamViewer, RustDesk, VNC, Chrome Remote Desktop, Parsec, etc.).

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::debug::{self, Failure};

/// Portas padrão de protocolos/agentes de acesso remoto verificadas além da porta RDP real.
const KNOWN_REMOTE_PORTS: &[(u16, &str)] = &[
    (3389, "RDP (padrão)"),
    (5900, "VNC"),
    (5901, "VNC"),
    (5902, "VNC"),
    (5903, "VNC"),
    (5938, "TeamViewer"),
    (7070, "AnyDesk"),
    (21115, "RustDesk"),
    (21116, "RustDesk"),
    (21117, "RustDesk"),
    (21118, "RustDesk"),
    (21119, "RustDesk"),
    (22, "SSH"),
];

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditVerdict {
    ActiveRemoteSession,
    RemoteSurfacePresent,
    WslgOnly,
    Clean,
    Partial,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Attention,
    Alert,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessClassification {
    WslgInternal,
    RdpService,
    RdpClient,
    RemoteAgent,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionDirection {
    InboundRdp,
    ListeningRdp,
    ListeningRemotePort,
    RemoteAgentEstablished,
    RemoteAgentListening,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RdpConfiguration {
    /// `Some(true)` = RDP aceitando conexões; `Some(false)` = desabilitado; `None` = não medido.
    pub enabled: Option<bool>,
    /// Porta real configurada (ex: 3389 ou porta customizada no registro / xrdp.ini).
    pub configured_port: Option<u16>,
    pub is_custom_port: bool,
    /// Network Level Authentication exigido (`UserAuthentication == 1` no Windows).
    pub nla_required: Option<bool>,
    /// Estado do serviço `TermService` (Windows) ou `xrdp` (Linux).
    pub service_status: Option<String>,
    pub service_pid: Option<u32>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionEntry {
    pub session_name: String,
    pub username: Option<String>,
    pub session_id: u32,
    pub state: String,
    pub is_remote: bool,
    pub is_active_remote: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteNetworkEntry {
    pub protocol: String,
    pub direction: ConnectionDirection,
    pub local_address: String,
    pub local_port: u16,
    pub remote_address: Option<String>,
    pub remote_port: Option<u16>,
    pub is_loopback: bool,
    pub is_public_bind: bool,
    pub pid: u32,
    pub process_name: String,
    pub executable_path: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProcessEntry {
    pub pid: u32,
    pub name: String,
    pub executable_path: Option<String>,
    pub classification: ProcessClassification,
    pub label: String,
    pub explanation: String,
    pub external_connections: usize,
    pub listening_ports: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuditCheckResult {
    pub id: String,
    pub title: String,
    pub status: CheckStatus,
    pub summary: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessAudit {
    pub verdict: AuditVerdict,
    pub verdict_title: String,
    pub verdict_summary: String,
    pub rdp_config: RdpConfiguration,
    pub sessions: Vec<RemoteSessionEntry>,
    pub sessions_unavailable_reason: Option<String>,
    pub connections: Vec<RemoteNetworkEntry>,
    pub connections_unavailable_reason: Option<String>,
    pub processes: Vec<RemoteProcessEntry>,
    pub processes_unavailable_reason: Option<String>,
    pub checks: Vec<AuditCheckResult>,
    pub collected_at_unix_ms: u64,
    pub elevated: bool,
}

/// Snapshot bruto de processo usado na etapa pura de classificação e correlação.
#[derive(Debug, Clone)]
pub(crate) struct RawProcess {
    pub pid: u32,
    pub name: String,
    pub executable_path: Option<String>,
}

/// Linha TCP bruta (LISTEN ou ESTABLISHED) usada na correlação determinística.
#[derive(Debug, Clone)]
pub(crate) struct RawTcpSocket {
    pub pid: u32,
    pub is_listening: bool,
    pub local_address: String,
    pub local_port: u16,
    pub remote_address: Option<String>,
    pub remote_port: Option<u16>,
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn is_loopback_addr(addr: &str) -> bool {
    addr == "127.0.0.1"
        || addr == "::1"
        || addr == "0.0.0.0"
        || addr == "::"
        || addr.starts_with("127.")
        || addr.eq_ignore_ascii_case("::ffff:127.0.0.1")
}

pub(crate) fn is_public_bind_addr(addr: &str) -> bool {
    addr == "0.0.0.0" || addr == "::"
}

fn known_remote_port_label(port: u16) -> Option<&'static str> {
    KNOWN_REMOTE_PORTS
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, label)| *label)
}

/// Classifica um processo em execução de forma determinística.
/// Retorna `None` para processos comuns que não têm relação com acesso remoto ou WSLg.
pub(crate) fn classify_process(
    proc_item: &RawProcess,
    term_service_pid: Option<u32>,
) -> Option<(ProcessClassification, String, String)> {
    let lower_name = proc_item.name.to_ascii_lowercase();
    let stem = lower_name.strip_suffix(".exe").unwrap_or(&lower_name);
    let lower_path = proc_item
        .executable_path
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    if let Some(ts_pid) = term_service_pid {
        if ts_pid > 0 && proc_item.pid == ts_pid {
            return Some((
                ProcessClassification::RdpService,
                "Serviço de Área de Trabalho Remota (TermService)".to_string(),
                "Processo do Windows responsável por receber conexões RDP quando habilitado."
                    .to_string(),
            ));
        }
    }

    // Diferenciação crucial do WSLg: msrdc.exe dentro da instalação do WSL é integração local.
    if stem == "msrdc" {
        let is_wsl_path = lower_path.contains("\\wsl\\")
            || lower_path.contains("/wsl/")
            || lower_path.contains("windowssubsystemforlinux");
        if is_wsl_path {
            return Some((
                ProcessClassification::WslgInternal,
                "Cliente Remote Desktop interno do WSLg".to_string(),
                "O WSL usa msrdc.exe localmente para integrar janelas gráficas Linux ao Windows. Não indica acesso externo.".to_string(),
            ));
        }
        return Some((
            ProcessClassification::RdpClient,
            "Cliente Microsoft Remote Desktop (msrdc)".to_string(),
            "Executável msrdc fora da pasta padrão do WSL; verifique o caminho completo."
                .to_string(),
        ));
    }

    if stem == "wslhost" || stem == "wslservice" {
        return Some((
            ProcessClassification::WslgInternal,
            "Componente interno do WSL".to_string(),
            "Processo padrão do Subsistema do Windows para Linux (WSLg).".to_string(),
        ));
    }

    if stem == "msrdcw" || stem == "mstsc" || stem == "remmina" || stem == "xfreerdp" {
        return Some((
            ProcessClassification::RdpClient,
            format!("Cliente RDP de saída ({})", proc_item.name),
            "Cliente usado para conectar-se a outra máquina; sozinho não permite entrada nesta máquina.".to_string(),
        ));
    }

    // Agentes e servidores de acesso remoto conhecidos.
    let agent_label: Option<&'static str> = match stem {
        "anydesk" => Some("AnyDesk"),
        "teamviewer" | "teamviewer_service" | "teamviewerd" => Some("TeamViewer"),
        "rustdesk" => Some("RustDesk"),
        "parsecd" | "pservice" => Some("Parsec"),
        "remoting_host" | "chrome-remote-desktop" | "chrome-remote-desktop-host" => {
            Some("Chrome Remote Desktop")
        }
        "tvnserver" => Some("TightVNC Server"),
        "vncserver" | "winvnc" | "winvnc4" | "uvnc_service" | "x11vnc" | "wayvnc" | "xvnc"
        | "xtigervnc" | "tigervnc" => Some("Servidor VNC"),
        "xrdp" | "xrdp-sesman" => Some("Servidor xrdp"),
        "srservice" | "splashtop" | "srmanager" | "strwinclt" => Some("Splashtop"),
        "screenconnect.clientservice" | "screenconnect.windowsclient" => {
            Some("ConnectWise ScreenConnect")
        }
        "logmein" | "lmiguardiansvc" => Some("LogMeIn"),
        "sunshine" => Some("Sunshine (streaming remoto)"),
        "meshagent" => Some("MeshCentral Agent"),
        "tacticalrmm" => Some("Tactical RMM"),
        "dwagent" => Some("DWService Agent"),
        "ultraviewer_desktop" | "ultraviewer_service" => Some("UltraViewer"),
        "supremo" | "supremoservice" => Some("Supremo"),
        "nxserver" | "nxd" | "nxnode" => Some("NoMachine"),
        "sshd" => Some("Servidor OpenSSH (sshd)"),
        _ if stem.starts_with("anydesk") => Some("AnyDesk"),
        _ if stem.starts_with("teamviewer") => Some("TeamViewer"),
        _ if stem.starts_with("rustdesk") => Some("RustDesk"),
        _ if stem.starts_with("splashtop") => Some("Splashtop"),
        _ if stem.starts_with("screenconnect") => Some("ConnectWise ScreenConnect"),
        _ if stem.starts_with("ammyy") => Some("Ammyy Admin"),
        _ if stem.starts_with("ultraviewer") => Some("UltraViewer"),
        _ => None,
    };

    agent_label.map(|brand| {
        (
            ProcessClassification::RemoteAgent,
            format!("Agente de acesso remoto ({brand})"),
            format!(
                "{brand} está em execução. Confirme se você instalou e espera que este acesso esteja ativo."
            ),
        )
    })
}

/// Interpreta a saída tabular de `qwinsta` (funciona em en-US e pt-BR).
///
/// Exemplos reais:
/// ```text
///  SESSIONNAME       USERNAME                 ID  STATE   TYPE        DEVICE
///  services                                    0  Disc
/// >console           lucas                     1  Active
///  rdp-tcp#0         admin                     2  Active  rdpwd
///  rdp-tcp                                 65536  Listen
/// ```
pub(crate) fn parse_qwinsta_output(raw: &str) -> Vec<RemoteSessionEntry> {
    let mut entries = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cleaned = trimmed.trim_start_matches('>').trim();
        let tokens: Vec<&str> = cleaned.split_whitespace().collect();
        if tokens.len() < 3 {
            continue;
        }

        // Ignora a linha de cabeçalho (SESSIONNAME / NOMEDASESSÃO).
        let first_upper = tokens[0].to_ascii_uppercase();
        if first_upper.starts_with("SESSION") || first_upper.starts_with("NOME") {
            continue;
        }

        // Encontra o token numérico de ID da sessão (pode estar em tokens[1] ou tokens[2]).
        let id_pos = tokens
            .iter()
            .enumerate()
            .skip(1)
            .take(3)
            .find_map(|(idx, tok)| tok.parse::<u32>().ok().map(|id| (idx, id)));

        let Some((id_idx, session_id)) = id_pos else {
            continue;
        };

        let session_name = tokens[0].to_string();
        let username = if id_idx == 2 {
            Some(tokens[1].to_string())
        } else if id_idx > 2 {
            Some(tokens[1..id_idx].join(" "))
        } else {
            None
        };

        let raw_state = tokens.get(id_idx + 1).copied().unwrap_or("");
        let state = normalize_session_state(raw_state);
        let lower_name = session_name.to_ascii_lowercase();
        let is_remote = lower_name.starts_with("rdp-tcp")
            || lower_name.starts_with("ica-")
            || lower_name.starts_with("ssh");
        let is_active_remote = is_remote && state == "Active";

        entries.push(RemoteSessionEntry {
            session_name,
            username,
            session_id,
            state,
            is_remote,
            is_active_remote,
        });
    }

    entries
}

fn normalize_session_state(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower == "active" || lower == "ativo" || lower == "ativa" {
        "Active".to_string()
    } else if lower.starts_with("disc") || lower.starts_with("desc") {
        "Disconnected".to_string()
    } else if lower.starts_with("listen")
        || lower.starts_with("ouvir")
        || lower.starts_with("escuta")
    {
        "Listening".to_string()
    } else if lower.starts_with("conn") || lower.starts_with("conec") {
        "Connected".to_string()
    } else if raw.is_empty() {
        "Other".to_string()
    } else {
        raw.to_string()
    }
}

/// Lê a porta real configurada no `/etc/xrdp/xrdp.ini` (Linux).
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
pub(crate) fn parse_xrdp_ini_port(content: &str) -> Option<u16> {
    let mut in_globals = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_globals = trimmed.eq_ignore_ascii_case("[globals]");
            continue;
        }
        if in_globals {
            if let Some((key, value)) = trimmed.split_once('=') {
                if key.trim().eq_ignore_ascii_case("port") {
                    let raw = value.trim();
                    // Formatos aceitos pelo xrdp: `3389`, `tcp://:3390`, `tcp://0.0.0.0:3390`
                    let port_str = raw.rsplit(':').next().unwrap_or(raw);
                    if let Ok(port) = port_str.parse::<u16>() {
                        if port > 0 {
                            return Some(port);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Monta o relatório determinístico completo a partir das evidências coletadas.
/// Função pura — testável em qualquer plataforma.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_audit_report(
    rdp_config: RdpConfiguration,
    sessions: Vec<RemoteSessionEntry>,
    sessions_unavailable_reason: Option<String>,
    raw_sockets: Option<Vec<RawTcpSocket>>,
    connections_unavailable_reason: Option<String>,
    raw_processes: Option<Vec<RawProcess>>,
    processes_unavailable_reason: Option<String>,
    elevated: bool,
) -> RemoteAccessAudit {
    let rdp_port = rdp_config.configured_port.unwrap_or(3389);
    let term_service_pid = rdp_config.service_pid;

    // Índice rápido de PID -> processo bruto.
    let mut proc_by_pid: HashMap<u32, &RawProcess> = HashMap::new();
    let mut classified_by_pid: HashMap<u32, (ProcessClassification, String, String)> =
        HashMap::new();

    if let Some(ref procs) = raw_processes {
        for p in procs {
            proc_by_pid.insert(p.pid, p);
            if let Some(info) = classify_process(p, term_service_pid) {
                classified_by_pid.insert(p.pid, info);
            }
        }
    }

    // Correlaciona sockets TCP com porta RDP real, TermService, portas conhecidas e agentes.
    let mut connections: Vec<RemoteNetworkEntry> = Vec::new();
    let mut external_count_by_pid: HashMap<u32, usize> = HashMap::new();
    let mut listening_ports_by_pid: HashMap<u32, Vec<u16>> = HashMap::new();

    if let Some(ref sockets) = raw_sockets {
        for sock in sockets {
            let proc_ref = proc_by_pid.get(&sock.pid).copied();
            let process_name = proc_ref
                .map(|p| p.name.clone())
                .unwrap_or_else(|| format!("PID {}", sock.pid));
            let executable_path = proc_ref.and_then(|p| p.executable_path.clone());
            let proc_class = classified_by_pid.get(&sock.pid).map(|(c, _, _)| *c);

            let is_loopback = if sock.is_listening {
                !is_public_bind_addr(&sock.local_address) && is_loopback_addr(&sock.local_address)
            } else {
                sock.remote_address
                    .as_deref()
                    .map(is_loopback_addr)
                    .unwrap_or(true)
            };
            let is_public_bind = sock.is_listening && is_public_bind_addr(&sock.local_address);

            if sock.is_listening {
                let ports = listening_ports_by_pid.entry(sock.pid).or_default();
                if !ports.contains(&sock.local_port) {
                    ports.push(sock.local_port);
                }
            } else if !is_loopback {
                *external_count_by_pid.entry(sock.pid).or_insert(0) += 1;
            }

            let is_rdp_port = sock.local_port == rdp_port;
            let is_term_service = term_service_pid.is_some_and(|tp| tp > 0 && sock.pid == tp);
            let known_port = known_remote_port_label(sock.local_port);

            let entry: Option<(ConnectionDirection, String)> =
                if !sock.is_listening && (is_rdp_port || is_term_service) {
                    let reason = if is_rdp_port && rdp_config.is_custom_port {
                        format!(
                            "Conexão estabelecida na porta RDP customizada ({})",
                            sock.local_port
                        )
                    } else if is_rdp_port {
                        format!(
                            "Conexão estabelecida na porta RDP configurada ({})",
                            sock.local_port
                        )
                    } else {
                        format!(
                            "Conexão estabelecida pertencente ao serviço TermService (porta {})",
                            sock.local_port
                        )
                    };
                    Some((ConnectionDirection::InboundRdp, reason))
                } else if sock.is_listening && (is_rdp_port || is_term_service) {
                    let reason = if rdp_config.is_custom_port && is_rdp_port {
                        format!("Porta RDP customizada ({}) em escuta", sock.local_port)
                    } else {
                        format!("Porta RDP ({}) em escuta", sock.local_port)
                    };
                    Some((ConnectionDirection::ListeningRdp, reason))
                } else if !sock.is_listening && known_port.is_some() && !is_loopback {
                    let label = known_port.unwrap_or("Acesso remoto");
                    Some((
                        ConnectionDirection::RemoteAgentEstablished,
                        format!(
                            "Conexão estabelecida na porta local {} ({label})",
                            sock.local_port
                        ),
                    ))
                } else if sock.is_listening && known_port.is_some() {
                    let label = known_port.unwrap_or("Acesso remoto");
                    Some((
                        ConnectionDirection::ListeningRemotePort,
                        format!("Porta {} ({label}) em escuta", sock.local_port),
                    ))
                } else if matches!(proc_class, Some(ProcessClassification::RemoteAgent)) {
                    if sock.is_listening {
                        Some((
                            ConnectionDirection::RemoteAgentListening,
                            format!(
                                "Agente remoto ({process_name}) escutando na porta {}",
                                sock.local_port
                            ),
                        ))
                    } else if !is_loopback {
                        Some((
                            ConnectionDirection::RemoteAgentEstablished,
                            format!("Conexão externa ativa do agente remoto ({process_name})"),
                        ))
                    } else {
                        None
                    }
                } else if matches!(proc_class, Some(ProcessClassification::RdpClient))
                    && !sock.is_listening
                    && !is_loopback
                {
                    Some((
                        ConnectionDirection::RemoteAgentEstablished,
                        format!("Cliente RDP ({process_name}) com conexão externa de saída"),
                    ))
                } else {
                    None
                };

            if let Some((direction, reason)) = entry {
                connections.push(RemoteNetworkEntry {
                    protocol: "TCP".to_string(),
                    direction,
                    local_address: sock.local_address.clone(),
                    local_port: sock.local_port,
                    remote_address: sock.remote_address.clone(),
                    remote_port: sock.remote_port,
                    is_loopback,
                    is_public_bind,
                    pid: sock.pid,
                    process_name,
                    executable_path,
                    reason,
                });
            }
        }
    }

    // Ordena conexões: mais críticas primeiro (InboundRdp -> RemoteAgentEstablished -> Listening).
    connections.sort_by_key(|c| {
        let rank = match c.direction {
            ConnectionDirection::InboundRdp if !c.is_loopback => 0,
            ConnectionDirection::RemoteAgentEstablished if !c.is_loopback => 1,
            ConnectionDirection::ListeningRdp => 2,
            ConnectionDirection::ListeningRemotePort => 3,
            ConnectionDirection::RemoteAgentListening => 4,
            _ => 5,
        };
        (rank, c.local_port, c.pid)
    });

    // Monta lista final de processos relevantes.
    let mut processes: Vec<RemoteProcessEntry> = Vec::new();
    if let Some(ref procs) = raw_processes {
        let mut seen_pids = HashSet::new();
        for p in procs {
            if !seen_pids.insert(p.pid) {
                continue;
            }
            if let Some((classification, label, explanation)) = classified_by_pid.get(&p.pid) {
                let mut ports = listening_ports_by_pid.remove(&p.pid).unwrap_or_default();
                ports.sort_unstable();
                processes.push(RemoteProcessEntry {
                    pid: p.pid,
                    name: p.name.clone(),
                    executable_path: p.executable_path.clone(),
                    classification: *classification,
                    label: label.clone(),
                    explanation: explanation.clone(),
                    external_connections: external_count_by_pid.get(&p.pid).copied().unwrap_or(0),
                    listening_ports: ports,
                });
            }
        }
    }

    // Ordena processos: agentes remotos -> serviço RDP -> clientes -> WSLg interno.
    processes.sort_by_key(|p| {
        let rank = match p.classification {
            ProcessClassification::RemoteAgent => 0,
            ProcessClassification::RdpService => 1,
            ProcessClassification::RdpClient => 2,
            ProcessClassification::WslgInternal => 3,
        };
        (rank, p.name.to_ascii_lowercase(), p.pid)
    });

    // --- 4 checagens determinísticas ---
    let check_rdp = build_rdp_check(&rdp_config);
    let check_sessions = build_sessions_check(&sessions, sessions_unavailable_reason.as_deref());
    let check_network = build_network_check(
        &connections,
        rdp_port,
        rdp_config.is_custom_port,
        connections_unavailable_reason.as_deref(),
    );
    let check_processes =
        build_processes_check(&processes, processes_unavailable_reason.as_deref());

    let checks = vec![check_rdp, check_sessions, check_network, check_processes];

    // --- Veredito determinístico final ---
    let has_active_remote_session = sessions.iter().any(|s| s.is_active_remote);
    let has_active_inbound_rdp = connections
        .iter()
        .any(|c| c.direction == ConnectionDirection::InboundRdp && !c.is_loopback);
    let has_active_remote_agent_conn = connections
        .iter()
        .any(|c| c.direction == ConnectionDirection::RemoteAgentEstablished && !c.is_loopback);

    let has_remote_agent_running = processes
        .iter()
        .any(|p| p.classification == ProcessClassification::RemoteAgent);
    let has_rdp_enabled_or_listening = rdp_config.enabled == Some(true)
        || connections.iter().any(|c| {
            matches!(
                c.direction,
                ConnectionDirection::ListeningRdp | ConnectionDirection::ListeningRemotePort
            )
        });
    let has_wslg_only = processes
        .iter()
        .any(|p| p.classification == ProcessClassification::WslgInternal);
    let has_unavailable = checks.iter().any(|c| c.status == CheckStatus::Unavailable);

    let (verdict, verdict_title, verdict_summary) = if has_active_remote_session
        || has_active_inbound_rdp
    {
        let active_count = sessions.iter().filter(|s| s.is_active_remote).count();
        let inbound_count = connections
            .iter()
            .filter(|c| c.direction == ConnectionDirection::InboundRdp && !c.is_loopback)
            .count();
        (
                AuditVerdict::ActiveRemoteSession,
                "Sessão remota ativa detectada agora".to_string(),
                format!(
                    "Encontrada evidência direta de acesso remoto em andamento ({active_count} sessão(ões) remota(s) ativa(s), {inbound_count} conexão(ões) TCP externa(s) na porta RDP {rdp_port}). Revise os endereços e sessões abaixo."
                ),
            )
    } else if has_active_remote_agent_conn {
        (
                AuditVerdict::RemoteSurfacePresent,
                "Agente de acesso remoto com conexão externa ativa".to_string(),
                "Nenhuma sessão RDP nativa ativa, mas há processo de acesso remoto com conexão externa estabelecida neste momento. Confirme se o uso é intencional.".to_string(),
            )
    } else if has_rdp_enabled_or_listening || has_remote_agent_running {
        let mut reasons = Vec::new();
        if rdp_config.enabled == Some(true) {
            reasons.push(format!("RDP habilitado na porta {rdp_port}"));
        }
        if has_remote_agent_running {
            let count = processes
                .iter()
                .filter(|p| p.classification == ProcessClassification::RemoteAgent)
                .count();
            reasons.push(format!("{count} agente(s) de acesso remoto em execução"));
        }
        if reasons.is_empty() {
            reasons.push("porta de acesso remoto em escuta".to_string());
        }
        (
            AuditVerdict::RemoteSurfacePresent,
            "Sem sessão ativa, mas há acesso remoto habilitado ou agente em execução".to_string(),
            format!(
                "Não há evidência de alguém conectado agora, porém foi detectado: {}.",
                reasons.join(" e ")
            ),
        )
    } else if has_unavailable {
        let missing = checks
            .iter()
            .filter(|c| c.status == CheckStatus::Unavailable)
            .map(|c| c.title.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        (
                AuditVerdict::Partial,
                "Verificação parcial — nem todos os sinais puderam ser lidos".to_string(),
                format!(
                    "Os sinais coletados não mostram acesso remoto ativo, mas não foi possível concluir: {missing}. Por honestidade, o diagnóstico não é marcado como completo."
                ),
            )
    } else if has_wslg_only {
        (
                AuditVerdict::WslgOnly,
                "Sem indício de acesso remoto — apenas integração interna do WSLg".to_string(),
                "RDP está desabilitado, não há sessões remotas nem agentes de terceiros. Os componentes Remote Desktop encontrados pertencem ao WSLg local (janelas gráficas Linux).".to_string(),
            )
    } else {
        (
                AuditVerdict::Clean,
                "Nenhum indício de acesso remoto ativo".to_string(),
                "RDP desabilitado, sem sessões remotas ativas, sem conexões em portas de acesso remoto e sem agentes conhecidos em execução.".to_string(),
            )
    };

    RemoteAccessAudit {
        verdict,
        verdict_title,
        verdict_summary,
        rdp_config,
        sessions,
        sessions_unavailable_reason,
        connections,
        connections_unavailable_reason,
        processes,
        processes_unavailable_reason,
        checks,
        collected_at_unix_ms: now_unix_ms(),
        elevated,
    }
}

fn build_rdp_check(cfg: &RdpConfiguration) -> AuditCheckResult {
    if let Some(ref reason) = cfg.unavailable_reason {
        return AuditCheckResult {
            id: "rdp_config".to_string(),
            title: "Configuração e porta do RDP".to_string(),
            status: CheckStatus::Unavailable,
            summary: format!("Não foi possível ler a configuração do RDP: {reason}"),
            details: Vec::new(),
        };
    }

    let mut details = Vec::new();
    match cfg.enabled {
        Some(true) => details.push("Entrada RDP: habilitada (fDenyTSConnections = 0)".to_string()),
        Some(false) => {
            details.push("Entrada RDP: desabilitada (fDenyTSConnections = 1)".to_string())
        }
        None => details.push("Entrada RDP: estado não informado".to_string()),
    }
    if let Some(port) = cfg.configured_port {
        if cfg.is_custom_port {
            details.push(format!(
                "Porta configurada: {port} (porta customizada — diferente da 3389)"
            ));
        } else {
            details.push(format!("Porta configurada: {port} (padrão)"));
        }
    }
    if let Some(nla) = cfg.nla_required {
        details.push(format!(
            "Autenticação no nível de rede (NLA): {}",
            if nla { "exigida" } else { "desativada" }
        ));
    }
    if let Some(ref svc) = cfg.service_status {
        let pid_suffix = cfg
            .service_pid
            .filter(|pid| *pid > 0)
            .map(|pid| format!(" (PID {pid})"))
            .unwrap_or_default();
        details.push(format!("Serviço RDP: {svc}{pid_suffix}"));
    }

    let (status, summary) = match cfg.enabled {
        Some(false) => (
            CheckStatus::Ok,
            "RDP configurado para recusar conexões de entrada.".to_string(),
        ),
        Some(true) => {
            let port_desc = cfg
                .configured_port
                .map(|p| format!("porta {p}"))
                .unwrap_or_else(|| "porta não identificada".to_string());
            (
                CheckStatus::Attention,
                format!("RDP habilitado para receber conexões ({port_desc})."),
            )
        }
        None => (
            CheckStatus::Unavailable,
            "Estado do RDP não pôde ser confirmado.".to_string(),
        ),
    };

    AuditCheckResult {
        id: "rdp_config".to_string(),
        title: "Configuração e porta do RDP".to_string(),
        status,
        summary,
        details,
    }
}

fn build_sessions_check(
    sessions: &[RemoteSessionEntry],
    unavailable_reason: Option<&str>,
) -> AuditCheckResult {
    if let Some(reason) = unavailable_reason {
        return AuditCheckResult {
            id: "sessions".to_string(),
            title: "Sessões do sistema".to_string(),
            status: CheckStatus::Unavailable,
            summary: format!("Não foi possível listar sessões: {reason}"),
            details: Vec::new(),
        };
    }

    let active_remote: Vec<&RemoteSessionEntry> =
        sessions.iter().filter(|s| s.is_active_remote).collect();
    let other_remote: Vec<&RemoteSessionEntry> = sessions
        .iter()
        .filter(|s| s.is_remote && !s.is_active_remote)
        .collect();

    let mut details: Vec<String> = sessions
        .iter()
        .map(|s| {
            let user = s.username.as_deref().unwrap_or("—");
            format!(
                "{} (ID {}) · usuário: {} · {}",
                s.session_name, s.session_id, user, s.state
            )
        })
        .collect();
    if details.is_empty() {
        details.push("Nenhuma sessão listada pelo sistema.".to_string());
    }

    if !active_remote.is_empty() {
        AuditCheckResult {
            id: "sessions".to_string(),
            title: "Sessões do sistema".to_string(),
            status: CheckStatus::Alert,
            summary: format!(
                "{} sessão(ões) remota(s) em estado Active agora.",
                active_remote.len()
            ),
            details,
        }
    } else if !other_remote.is_empty() {
        AuditCheckResult {
            id: "sessions".to_string(),
            title: "Sessões do sistema".to_string(),
            status: CheckStatus::Ok,
            summary: "Nenhuma sessão remota ativa (apenas listener ou sessão desconectada)."
                .to_string(),
            details,
        }
    } else {
        AuditCheckResult {
            id: "sessions".to_string(),
            title: "Sessões do sistema".to_string(),
            status: CheckStatus::Ok,
            summary: "Nenhuma sessão remota registrada; apenas sessão local.".to_string(),
            details,
        }
    }
}

fn build_network_check(
    connections: &[RemoteNetworkEntry],
    rdp_port: u16,
    is_custom_port: bool,
    unavailable_reason: Option<&str>,
) -> AuditCheckResult {
    if let Some(reason) = unavailable_reason {
        return AuditCheckResult {
            id: "network_rdp".to_string(),
            title: "Conexões e portas de acesso remoto".to_string(),
            status: CheckStatus::Unavailable,
            summary: format!("Tabela TCP indisponível: {reason}"),
            details: Vec::new(),
        };
    }

    let inbound_rdp: Vec<&RemoteNetworkEntry> = connections
        .iter()
        .filter(|c| c.direction == ConnectionDirection::InboundRdp && !c.is_loopback)
        .collect();
    let agent_established: Vec<&RemoteNetworkEntry> = connections
        .iter()
        .filter(|c| c.direction == ConnectionDirection::RemoteAgentEstablished && !c.is_loopback)
        .collect();
    let listening: Vec<&RemoteNetworkEntry> = connections
        .iter()
        .filter(|c| {
            matches!(
                c.direction,
                ConnectionDirection::ListeningRdp
                    | ConnectionDirection::ListeningRemotePort
                    | ConnectionDirection::RemoteAgentListening
            )
        })
        .collect();

    let details: Vec<String> = connections
        .iter()
        .take(12)
        .map(|c| {
            if let (Some(ref raddr), Some(rport)) = (&c.remote_address, c.remote_port) {
                format!(
                    "{}:{} ↔ {}:{} ({}, PID {}) — {}",
                    c.local_address, c.local_port, raddr, rport, c.process_name, c.pid, c.reason
                )
            } else {
                format!(
                    "{}:{} LISTEN ({}, PID {}) — {}",
                    c.local_address, c.local_port, c.process_name, c.pid, c.reason
                )
            }
        })
        .collect();

    let port_note = if is_custom_port {
        format!("porta RDP customizada {rdp_port} + portas remotas conhecidas")
    } else {
        format!("porta RDP {rdp_port} + portas remotas conhecidas")
    };

    if !inbound_rdp.is_empty() {
        AuditCheckResult {
            id: "network_rdp".to_string(),
            title: "Conexões e portas de acesso remoto".to_string(),
            status: CheckStatus::Alert,
            summary: format!(
                "{} conexão(ões) externa(s) estabelecida(s) na porta RDP {rdp_port}.",
                inbound_rdp.len()
            ),
            details,
        }
    } else if !agent_established.is_empty() || !listening.is_empty() {
        AuditCheckResult {
            id: "network_rdp".to_string(),
            title: "Conexões e portas de acesso remoto".to_string(),
            status: CheckStatus::Attention,
            summary: format!(
                "{} conexão(ões) externa(s) de agente e {} porta(s) remota(s) em escuta ({port_note}).",
                agent_established.len(),
                listening.len()
            ),
            details,
        }
    } else {
        AuditCheckResult {
            id: "network_rdp".to_string(),
            title: "Conexões e portas de acesso remoto".to_string(),
            status: CheckStatus::Ok,
            summary: format!("Nenhuma conexão ou escuta ativa encontrada ({port_note})."),
            details,
        }
    }
}

fn build_processes_check(
    processes: &[RemoteProcessEntry],
    unavailable_reason: Option<&str>,
) -> AuditCheckResult {
    if let Some(reason) = unavailable_reason {
        return AuditCheckResult {
            id: "processes".to_string(),
            title: "Processos de acesso remoto e WSLg".to_string(),
            status: CheckStatus::Unavailable,
            summary: format!("Não foi possível inspecionar processos: {reason}"),
            details: Vec::new(),
        };
    }

    let agents: Vec<&RemoteProcessEntry> = processes
        .iter()
        .filter(|p| p.classification == ProcessClassification::RemoteAgent)
        .collect();
    let wslg: Vec<&RemoteProcessEntry> = processes
        .iter()
        .filter(|p| p.classification == ProcessClassification::WslgInternal)
        .collect();
    let clients: Vec<&RemoteProcessEntry> = processes
        .iter()
        .filter(|p| p.classification == ProcessClassification::RdpClient)
        .collect();

    let details: Vec<String> = processes
        .iter()
        .map(|p| {
            let path = p
                .executable_path
                .as_deref()
                .unwrap_or("caminho protegido pelo sistema");
            format!("{} (PID {}) — {} · {}", p.name, p.pid, p.label, path)
        })
        .collect();

    if !agents.is_empty() {
        AuditCheckResult {
            id: "processes".to_string(),
            title: "Processos de acesso remoto e WSLg".to_string(),
            status: CheckStatus::Attention,
            summary: format!(
                "{} agente(s) de acesso remoto em execução (ex: {}).",
                agents.len(),
                agents[0].name
            ),
            details,
        }
    } else if !clients.is_empty() {
        AuditCheckResult {
            id: "processes".to_string(),
            title: "Processos de acesso remoto e WSLg".to_string(),
            status: CheckStatus::Ok,
            summary: format!(
                "Nenhum agente de entrada; {} cliente(s) RDP de saída em execução.",
                clients.len()
            ),
            details,
        }
    } else if !wslg.is_empty() {
        AuditCheckResult {
            id: "processes".to_string(),
            title: "Processos de acesso remoto e WSLg".to_string(),
            status: CheckStatus::Ok,
            summary: format!(
                "Nenhum agente de terceiros; {} componente(s) interno(s) do WSLg detectado(s).",
                wslg.len()
            ),
            details,
        }
    } else {
        AuditCheckResult {
            id: "processes".to_string(),
            title: "Processos de acesso remoto e WSLg".to_string(),
            status: CheckStatus::Ok,
            summary: "Nenhum agente ou cliente de acesso remoto conhecido em execução.".to_string(),
            details,
        }
    }
}

// ---------------------------------------------------------------------------
// Comando Tauri
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn run_remote_access_audit() -> RemoteAccessAudit {
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(platform::collect_audit).await;

    match result {
        Ok(audit) => {
            let msg = format!(
                "Auditoria de acesso remoto concluída: {:?} ({} sessões, {} conexões, {} processos)",
                audit.verdict,
                audit.sessions.len(),
                audit.connections.len(),
                audit.processes.len()
            );
            debug::record_message(
                "security_remote_access_audit",
                started.elapsed().as_millis(),
                "ok",
                &msg,
            );
            audit
        }
        Err(error) => {
            let detail = error.to_string();
            debug::record_failure(
                "security_remote_access_audit",
                started.elapsed().as_millis(),
                Failure::new("Falha ao executar auditoria de acesso remoto")
                    .stage("spawn_blocking")
                    .detail(detail.clone()),
            );
            build_audit_report(
                RdpConfiguration {
                    enabled: None,
                    configured_port: None,
                    is_custom_port: false,
                    nla_required: None,
                    service_status: None,
                    service_pid: None,
                    unavailable_reason: Some(detail.clone()),
                },
                Vec::new(),
                Some(detail.clone()),
                None,
                Some(detail.clone()),
                None,
                Some(detail),
                false,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Coleta Windows (100% Win32 nativo + qwinsta oculto)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use super::{
        build_audit_report, parse_qwinsta_output, RawProcess, RawTcpSocket, RdpConfiguration,
        RemoteAccessAudit, RemoteSessionEntry,
    };
    use crate::process::hidden_command;
    use std::ffi::OsStr;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE, NO_ERROR,
    };
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
        MIB_TCPTABLE_OWNER_PID, MIB_TCP_STATE_ESTAB, MIB_TCP_STATE_LISTEN, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
        REG_DWORD, REG_VALUE_TYPE,
    };
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_MANAGER_CONNECT,
        SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START_PENDING,
        SERVICE_STATUS_PROCESS, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    const TABLE_SLACK: usize = 64 * 1024;

    pub fn collect_audit() -> RemoteAccessAudit {
        let elevated = is_elevated();
        let rdp_config = read_rdp_config();
        let (sessions, sessions_err) = collect_sessions();
        let (sockets, sockets_err) = match collect_tcp_sockets() {
            Ok(list) => (Some(list), None),
            Err(err) => (None, Some(err)),
        };
        let (processes, processes_err) = match collect_processes() {
            Ok(list) => (Some(list), None),
            Err(err) => (None, Some(err)),
        };

        build_audit_report(
            rdp_config,
            sessions,
            sessions_err,
            sockets,
            sockets_err,
            processes,
            processes_err,
            elevated,
        )
    }

    fn to_wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn read_hklm_dword(subkey: &str, value_name: &str) -> Result<u32, String> {
        let subkey_w = to_wide(subkey);
        let value_w = to_wide(value_name);
        let mut hkey = HKEY::default();
        let open_res = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(subkey_w.as_ptr()),
                Some(0),
                KEY_READ,
                &mut hkey,
            )
        };
        if open_res != ERROR_SUCCESS {
            return Err(format!("RegOpenKeyExW({subkey}) falhou ({})", open_res.0));
        }

        let mut data: u32 = 0;
        let mut data_len = std::mem::size_of::<u32>() as u32;
        let mut reg_type = REG_VALUE_TYPE(0);
        let query_res = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(value_w.as_ptr()),
                None,
                Some(&mut reg_type),
                Some((&mut data as *mut u32).cast::<u8>()),
                Some(&mut data_len),
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }

        if query_res != ERROR_SUCCESS {
            return Err(format!(
                "RegQueryValueExW({value_name}) falhou ({})",
                query_res.0
            ));
        }
        if reg_type != REG_DWORD {
            return Err(format!("Valor {value_name} não é REG_DWORD"));
        }
        Ok(data)
    }

    fn query_term_service() -> (Option<String>, Option<u32>) {
        let Ok(scm) =
            (unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) })
        else {
            return (None, None);
        };
        let svc_name = to_wide("TermService");
        let Ok(service) =
            (unsafe { OpenServiceW(scm, PCWSTR(svc_name.as_ptr()), SERVICE_QUERY_STATUS) })
        else {
            unsafe {
                let _ = CloseServiceHandle(scm);
            }
            return (None, None);
        };

        let mut status = SERVICE_STATUS_PROCESS::default();
        let mut needed: u32 = 0;
        let buf = unsafe {
            std::slice::from_raw_parts_mut(
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
                std::mem::size_of::<SERVICE_STATUS_PROCESS>(),
            )
        };
        let ok = unsafe {
            QueryServiceStatusEx(service, SC_STATUS_PROCESS_INFO, Some(buf), &mut needed)
        }
        .is_ok();

        unsafe {
            let _ = CloseServiceHandle(service);
            let _ = CloseServiceHandle(scm);
        }

        if !ok {
            return (None, None);
        }

        let label = match status.dwCurrentState {
            SERVICE_RUNNING => "Em execução",
            SERVICE_STOPPED => "Parado",
            SERVICE_START_PENDING => "Iniciando",
            SERVICE_STOP_PENDING => "Parando",
            _ => "Outro estado",
        };
        let pid = (status.dwProcessId > 0).then_some(status.dwProcessId);
        (Some(label.to_string()), pid)
    }

    fn read_rdp_config() -> RdpConfiguration {
        let ts_key = r"SYSTEM\CurrentControlSet\Control\Terminal Server";
        let rdp_tcp_key = r"SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp";

        let deny_res = read_hklm_dword(ts_key, "fDenyTSConnections");
        let port_res = read_hklm_dword(rdp_tcp_key, "PortNumber");
        let nla_res = read_hklm_dword(rdp_tcp_key, "UserAuthentication");
        let (service_status, service_pid) = query_term_service();

        match deny_res {
            Ok(deny) => {
                let configured_port = port_res
                    .ok()
                    .and_then(|p| u16::try_from(p).ok())
                    .filter(|p| *p > 0);
                let is_custom_port = configured_port.is_some_and(|p| p != 3389);
                let nla_required = nla_res.ok().map(|v| v != 0);
                RdpConfiguration {
                    enabled: Some(deny == 0),
                    configured_port,
                    is_custom_port,
                    nla_required,
                    service_status,
                    service_pid,
                    unavailable_reason: None,
                }
            }
            Err(err) => RdpConfiguration {
                enabled: None,
                configured_port: port_res.ok().and_then(|p| u16::try_from(p).ok()),
                is_custom_port: false,
                nla_required: None,
                service_status,
                service_pid,
                unavailable_reason: Some(err),
            },
        }
    }

    fn collect_sessions() -> (Vec<RemoteSessionEntry>, Option<String>) {
        match hidden_command("qwinsta.exe").output() {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let reason = if stderr.is_empty() {
                        format!("qwinsta retornou código {:?}", output.status.code())
                    } else {
                        stderr
                    };
                    return (Vec::new(), Some(reason));
                }
                let stdout = String::from_utf8_lossy(&output.stdout);
                (parse_qwinsta_output(&stdout), None)
            }
            Err(err) => (
                Vec::new(),
                Some(format!("qwinsta.exe indisponível neste Windows: {err}")),
            ),
        }
    }

    fn collect_tcp_sockets() -> Result<Vec<RawTcpSocket>, String> {
        let mut out = Vec::new();
        collect_tcp_family(AF_INET, &mut out)?;
        let _ = collect_tcp_family(AF_INET6, &mut out);
        Ok(out)
    }

    fn collect_tcp_family(af: u32, out: &mut Vec<RawTcpSocket>) -> Result<(), String> {
        let mut size: u32 = 0;
        let first =
            unsafe { GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0) };
        if first != ERROR_INSUFFICIENT_BUFFER.0 && first != NO_ERROR.0 {
            return Err(format!("GetExtendedTcpTable retornou {first}"));
        }
        if size == 0 {
            return Ok(());
        }

        let mut buffer = vec![0u8; size as usize + TABLE_SLACK];
        let mut used = buffer.len() as u32;
        loop {
            let status = unsafe {
                GetExtendedTcpTable(
                    Some(buffer.as_mut_ptr().cast()),
                    &mut used,
                    false,
                    af,
                    TCP_TABLE_OWNER_PID_ALL,
                    0,
                )
            };
            if status == ERROR_INSUFFICIENT_BUFFER.0 {
                buffer.resize(used as usize + TABLE_SLACK, 0);
                used = buffer.len() as u32;
                continue;
            }
            if status != NO_ERROR.0 {
                return Err(format!("GetExtendedTcpTable falhou com {status}"));
            }
            break;
        }

        if af == AF_INET {
            let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            let count = unsafe { (*table).dwNumEntries as usize };
            let rows_ptr =
                unsafe { std::ptr::addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>() };
            for row in unsafe { std::slice::from_raw_parts(rows_ptr, count) } {
                let is_listening = if row.dwState == MIB_TCP_STATE_LISTEN.0 as u32 {
                    true
                } else if row.dwState == MIB_TCP_STATE_ESTAB.0 as u32 {
                    false
                } else {
                    continue;
                };
                let local_port = u16::from_be(row.dwLocalPort as u16);
                if local_port == 0 {
                    continue;
                }
                out.push(RawTcpSocket {
                    pid: row.dwOwningPid,
                    is_listening,
                    local_address: Ipv4Addr::from(u32::from_be(row.dwLocalAddr)).to_string(),
                    local_port,
                    remote_address: (!is_listening)
                        .then(|| Ipv4Addr::from(u32::from_be(row.dwRemoteAddr)).to_string()),
                    remote_port: (!is_listening).then(|| u16::from_be(row.dwRemotePort as u16)),
                });
            }
        } else {
            let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            let count = unsafe { (*table).dwNumEntries as usize };
            let rows_ptr =
                unsafe { std::ptr::addr_of!((*table).table).cast::<MIB_TCP6ROW_OWNER_PID>() };
            for row in unsafe { std::slice::from_raw_parts(rows_ptr, count) } {
                let is_listening = if row.dwState == MIB_TCP_STATE_LISTEN.0 as u32 {
                    true
                } else if row.dwState == MIB_TCP_STATE_ESTAB.0 as u32 {
                    false
                } else {
                    continue;
                };
                let local_port = u16::from_be(row.dwLocalPort as u16);
                if local_port == 0 {
                    continue;
                }
                out.push(RawTcpSocket {
                    pid: row.dwOwningPid,
                    is_listening,
                    local_address: Ipv6Addr::from(row.ucLocalAddr).to_string(),
                    local_port,
                    remote_address: (!is_listening)
                        .then(|| Ipv6Addr::from(row.ucRemoteAddr).to_string()),
                    remote_port: (!is_listening).then(|| u16::from_be(row.dwRemotePort as u16)),
                });
            }
        }
        Ok(())
    }

    fn collect_processes() -> Result<Vec<RawProcess>, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|e| format!("CreateToolhelp32Snapshot falhou: {e}"))?;

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        let mut list = Vec::new();
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while ok {
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            let executable_path = query_image_path(entry.th32ProcessID);
            list.push(RawProcess {
                pid: entry.th32ProcessID,
                name,
                executable_path,
            });
            ok = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }

        unsafe {
            let _ = CloseHandle(snapshot);
        }
        Ok(list)
    }

    fn query_image_path(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
        };
        unsafe {
            let _ = CloseHandle(handle);
        }
        if result.is_err() {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }

    fn is_elevated() -> bool {
        let mut token = HANDLE::default();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned: u32 = 0;
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        }
        .is_ok();
        unsafe {
            let _ = CloseHandle(token);
        }
        ok && elevation.TokenIsElevated != 0
    }
}

// ---------------------------------------------------------------------------
// Coleta Linux (sysinfo + /proc/net/tcp + xrdp.ini + who)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod platform {
    use super::{
        build_audit_report, parse_xrdp_ini_port, RawProcess, RawTcpSocket, RdpConfiguration,
        RemoteAccessAudit, RemoteSessionEntry,
    };
    use crate::process::c_locale_command;
    use std::collections::HashMap;
    use std::fs;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    pub fn collect_audit() -> RemoteAccessAudit {
        let elevated = unsafe { libc::geteuid() == 0 };

        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
        );

        let raw_processes: Vec<RawProcess> = sys
            .processes()
            .iter()
            .map(|(pid, proc_item)| RawProcess {
                pid: pid.as_u32(),
                name: proc_item.name().to_string_lossy().into_owned(),
                executable_path: proc_item.exe().map(|p| p.to_string_lossy().into_owned()),
            })
            .collect();

        let xrdp_pid = raw_processes
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("xrdp"))
            .map(|p| p.pid);

        let rdp_config = read_linux_rdp_config(xrdp_pid);
        let (sessions, sessions_err) = collect_linux_sessions();
        let (sockets, sockets_err) = match collect_proc_tcp() {
            Ok(list) => (Some(list), None),
            Err(err) => (None, Some(err)),
        };

        build_audit_report(
            rdp_config,
            sessions,
            sessions_err,
            sockets,
            sockets_err,
            Some(raw_processes),
            None,
            elevated,
        )
    }

    fn read_linux_rdp_config(xrdp_pid: Option<u32>) -> RdpConfiguration {
        let ini_content = fs::read_to_string("/etc/xrdp/xrdp.ini").ok();
        let configured_port = ini_content
            .as_deref()
            .and_then(parse_xrdp_ini_port)
            .or_else(|| xrdp_pid.map(|_| 3389));
        let is_custom_port = configured_port.is_some_and(|p| p != 3389);
        let enabled = xrdp_pid.is_some();

        RdpConfiguration {
            enabled: Some(enabled),
            configured_port,
            is_custom_port,
            nla_required: None,
            service_status: Some(if enabled {
                "xrdp em execução".to_string()
            } else {
                "xrdp não está em execução".to_string()
            }),
            service_pid: xrdp_pid,
            unavailable_reason: None,
        }
    }

    fn collect_linux_sessions() -> (Vec<RemoteSessionEntry>, Option<String>) {
        match c_locale_command("who").output() {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut sessions = Vec::new();
                for (idx, line) in stdout.lines().enumerate() {
                    let tokens: Vec<&str> = line.split_whitespace().collect();
                    if tokens.len() < 2 {
                        continue;
                    }
                    let user = tokens[0].to_string();
                    let tty = tokens[1].to_string();
                    let has_remote_host = line.contains('(') && !line.contains("(:");
                    let is_remote = has_remote_host && tty.starts_with("pts/");
                    sessions.push(RemoteSessionEntry {
                        session_name: tty,
                        username: Some(user),
                        session_id: idx as u32,
                        state: "Active".to_string(),
                        is_remote,
                        is_active_remote: is_remote,
                    });
                }
                (sessions, None)
            }
            Ok(output) => (
                Vec::new(),
                Some(format!("who retornou código {:?}", output.status.code())),
            ),
            Err(err) => (Vec::new(), Some(format!("who indisponível: {err}"))),
        }
    }

    fn collect_proc_tcp() -> Result<Vec<RawTcpSocket>, String> {
        let inode_to_pid = map_socket_inodes();
        let mut sockets = Vec::new();

        if let Ok(v4) = fs::read_to_string("/proc/net/tcp") {
            parse_proc_tcp_table(&v4, false, &inode_to_pid, &mut sockets);
        } else {
            return Err("Não foi possível ler /proc/net/tcp".to_string());
        }
        if let Ok(v6) = fs::read_to_string("/proc/net/tcp6") {
            parse_proc_tcp_table(&v6, true, &inode_to_pid, &mut sockets);
        }
        Ok(sockets)
    }

    fn map_socket_inodes() -> HashMap<u64, u32> {
        let mut map = HashMap::new();
        let Ok(entries) = fs::read_dir("/proc") else {
            return map;
        };
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let fd_dir = entry.path().join("fd");
            let Ok(fds) = fs::read_dir(fd_dir) else {
                continue;
            };
            for fd in fds.flatten() {
                if let Ok(target) = fs::read_link(fd.path()) {
                    let s = target.to_string_lossy();
                    if let Some(rest) = s.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']'))
                    {
                        if let Ok(inode) = rest.parse::<u64>() {
                            map.insert(inode, pid);
                        }
                    }
                }
            }
        }
        map
    }

    fn parse_proc_tcp_table(
        content: &str,
        is_ipv6: bool,
        inode_to_pid: &HashMap<u64, u32>,
        out: &mut Vec<RawTcpSocket>,
    ) {
        for line in content.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 {
                continue;
            }
            // 01 = ESTABLISHED, 0A = LISTEN
            let is_listening = match cols[3] {
                "0A" => true,
                "01" => false,
                _ => continue,
            };
            let Some((local_address, local_port)) = parse_hex_endpoint(cols[1], is_ipv6) else {
                continue;
            };
            let remote = if is_listening {
                None
            } else {
                parse_hex_endpoint(cols[2], is_ipv6)
            };
            let inode = cols[9].parse::<u64>().unwrap_or(0);
            let pid = inode_to_pid.get(&inode).copied().unwrap_or(0);
            out.push(RawTcpSocket {
                pid,
                is_listening,
                local_address,
                local_port,
                remote_address: remote.as_ref().map(|(a, _)| a.clone()),
                remote_port: remote.map(|(_, p)| p),
            });
        }
    }

    fn parse_hex_endpoint(raw: &str, is_ipv6: bool) -> Option<(String, u16)> {
        let (addr_hex, port_hex) = raw.split_once(':')?;
        let port = u16::from_str_radix(port_hex, 16).ok()?;
        if !is_ipv6 {
            let n = u32::from_str_radix(addr_hex, 16).ok()?;
            Some((Ipv4Addr::from(n.to_be()).to_string(), port))
        } else if addr_hex.len() == 32 {
            let mut bytes = [0u8; 16];
            for word in 0..4 {
                let slice = &addr_hex[word * 8..(word + 1) * 8];
                let w = u32::from_str_radix(slice, 16).ok()?;
                bytes[word * 4..(word + 1) * 4].copy_from_slice(&w.to_ne_bytes());
            }
            Some((Ipv6Addr::from(bytes).to_string(), port))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Fallback honesto (macOS / outros)
// ---------------------------------------------------------------------------

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::{build_audit_report, RdpConfiguration, RemoteAccessAudit};

    pub fn collect_audit() -> RemoteAccessAudit {
        let reason =
            "Auditoria determinística de acesso remoto ainda não implementada nesta plataforma."
                .to_string();
        build_audit_report(
            RdpConfiguration {
                enabled: None,
                configured_port: None,
                is_custom_port: false,
                nla_required: None,
                service_status: None,
                service_pid: None,
                unavailable_reason: Some(reason.clone()),
            },
            Vec::new(),
            Some(reason.clone()),
            None,
            Some(reason.clone()),
            None,
            Some(reason),
            false,
        )
    }
}

// ---------------------------------------------------------------------------
// Testes unitários (rodam em Linux e Windows)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn disabled_rdp_cfg(port: u16) -> RdpConfiguration {
        RdpConfiguration {
            enabled: Some(false),
            configured_port: Some(port),
            is_custom_port: port != 3389,
            nla_required: Some(true),
            service_status: Some("Parado".to_string()),
            service_pid: None,
            unavailable_reason: None,
        }
    }

    #[test]
    fn parses_qwinsta_en_and_pt_br() {
        let sample = "\
 SESSIONNAME       USERNAME                 ID  STATE   TYPE        DEVICE
 services                                    0  Disc
>console           lucas                     1  Active
 rdp-tcp#0         suporte                   2  Active  rdpwd
 rdp-tcp                                 65536  Listen
";
        let sessions = parse_qwinsta_output(sample);
        assert_eq!(sessions.len(), 4);
        assert_eq!(sessions[1].session_name, "console");
        assert_eq!(sessions[1].username.as_deref(), Some("lucas"));
        assert!(!sessions[1].is_remote);

        assert_eq!(sessions[2].session_name, "rdp-tcp#0");
        assert_eq!(sessions[2].state, "Active");
        assert!(sessions[2].is_remote);
        assert!(sessions[2].is_active_remote);

        assert_eq!(sessions[3].session_name, "rdp-tcp");
        assert_eq!(sessions[3].state, "Listening");
        assert!(sessions[3].is_remote);
        assert!(!sessions[3].is_active_remote);

        let pt_br = "\
 NOMEDASESSÃO      USUÁRIO                  ID  ESTADO  TIPO        DISPOSITIVO
>console           lucas                     1  Ativo
 rdp-tcp#4         remoto                    3  Ativo   rdpwd
";
        let pt_sessions = parse_qwinsta_output(pt_br);
        assert_eq!(pt_sessions.len(), 2);
        assert!(pt_sessions[1].is_active_remote);
    }

    #[test]
    fn differentiates_wslg_msrdc_from_external_rdp_and_agents() {
        let wslg = RawProcess {
            pid: 100,
            name: "msrdc.exe".to_string(),
            executable_path: Some(r"C:\Program Files\WSL\msrdc.exe".to_string()),
        };
        let (class, _, _) = classify_process(&wslg, None).expect("msrdc deve ser classificado");
        assert_eq!(class, ProcessClassification::WslgInternal);

        let external_msrdc = RawProcess {
            pid: 101,
            name: "msrdc.exe".to_string(),
            executable_path: Some(r"C:\Users\dev\Downloads\msrdc.exe".to_string()),
        };
        let (class, _, _) = classify_process(&external_msrdc, None)
            .expect("msrdc fora do WSL deve ser classificado");
        assert_eq!(class, ProcessClassification::RdpClient);

        let anydesk = RawProcess {
            pid: 102,
            name: "AnyDesk.exe".to_string(),
            executable_path: Some(r"C:\Program Files (x86)\AnyDesk\AnyDesk.exe".to_string()),
        };
        let (class, label, _) =
            classify_process(&anydesk, None).expect("AnyDesk deve ser classificado");
        assert_eq!(class, ProcessClassification::RemoteAgent);
        assert!(label.contains("AnyDesk"));
    }

    #[test]
    fn verdict_is_wslg_only_when_rdp_disabled_and_only_wslg_runs() {
        let audit = build_audit_report(
            disabled_rdp_cfg(3389),
            vec![RemoteSessionEntry {
                session_name: "console".to_string(),
                username: Some("lucas".to_string()),
                session_id: 1,
                state: "Active".to_string(),
                is_remote: false,
                is_active_remote: false,
            }],
            None,
            Some(Vec::new()),
            None,
            Some(vec![
                RawProcess {
                    pid: 500,
                    name: "msrdc.exe".to_string(),
                    executable_path: Some(r"C:\Program Files\WSL\msrdc.exe".to_string()),
                },
                RawProcess {
                    pid: 501,
                    name: "wslservice.exe".to_string(),
                    executable_path: Some(r"C:\Program Files\WSL\wslservice.exe".to_string()),
                },
            ]),
            None,
            false,
        );

        assert_eq!(audit.verdict, AuditVerdict::WslgOnly);
        assert!(audit.checks.iter().all(|c| c.status == CheckStatus::Ok));
        assert_eq!(audit.processes.len(), 2);
    }

    #[test]
    fn detects_active_rdp_connection_on_custom_port() {
        let mut cfg = disabled_rdp_cfg(3390);
        cfg.enabled = Some(true);
        cfg.service_status = Some("Em execução".to_string());
        cfg.service_pid = Some(900);

        let audit = build_audit_report(
            cfg,
            vec![RemoteSessionEntry {
                session_name: "console".to_string(),
                username: Some("lucas".to_string()),
                session_id: 1,
                state: "Active".to_string(),
                is_remote: false,
                is_active_remote: false,
            }],
            None,
            Some(vec![RawTcpSocket {
                pid: 900,
                is_listening: false,
                local_address: "192.168.1.15".to_string(),
                local_port: 3390,
                remote_address: Some("203.0.113.42".to_string()),
                remote_port: Some(51234),
            }]),
            None,
            Some(vec![RawProcess {
                pid: 900,
                name: "svchost.exe".to_string(),
                executable_path: Some(r"C:\Windows\System32\svchost.exe".to_string()),
            }]),
            None,
            true,
        );

        assert_eq!(audit.verdict, AuditVerdict::ActiveRemoteSession);
        assert_eq!(audit.connections.len(), 1);
        assert_eq!(
            audit.connections[0].direction,
            ConnectionDirection::InboundRdp
        );
        assert!(audit.connections[0].reason.contains("3390"));
    }

    #[test]
    fn never_returns_clean_when_a_check_is_unavailable() {
        let audit = build_audit_report(
            disabled_rdp_cfg(3389),
            Vec::new(),
            Some("qwinsta indisponível".to_string()),
            Some(Vec::new()),
            None,
            Some(Vec::new()),
            None,
            false,
        );

        assert_eq!(audit.verdict, AuditVerdict::Partial);
        assert!(audit
            .checks
            .iter()
            .any(|c| c.id == "sessions" && c.status == CheckStatus::Unavailable));
    }

    #[test]
    fn parses_xrdp_ini_standard_and_custom_ports() {
        let standard = "[Globals]\nport=3389\n";
        assert_eq!(parse_xrdp_ini_port(standard), Some(3389));

        let custom = "[Globals]\n; comment\nport=tcp://:3395\n[Logging]\nport=9999\n";
        assert_eq!(parse_xrdp_ini_port(custom), Some(3395));
    }
}
