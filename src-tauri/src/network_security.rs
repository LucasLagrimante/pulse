//! Rede e Segurança: o que sai desta máquina, por qual processo, e os sinais de
//! postura que **não** dependem de um antivírus próprio.
//!
//! Princípio herdado do AGENTS.md: **evidência, nunca rótulo**. Tudo aqui é
//! verificável (`GetExtendedTcpTable`, store de certificados, registro, WMI).
//! Nada é classificado como malware; quando um sinal é ambíguo, a interface
//! mostra o fato e o usuário conclui.
//!
//! Custo medido da coleta de conexões (tcp4 + tcp6 + udp4 + resolução de
//! caminho pelo kernel): **~1,1 ms** em Rust/MSVC. Por isso a tela atualiza a
//! cada 2 s sem custo perceptível.
//!
//! Os sinais de postura são caros (WMI, store de certificados, PowerShell), por
//! isso ficam atrás de um cache de 2 minutos e nunca entram no caminho crítico
//! da abertura do app.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Validade do snapshot de conexões.
const CONNECTIONS_TTL: Duration = Duration::from_secs(2);
/// Validade dos sinais de postura (caros: WMI/PowerShell/certificados).
const POSTURE_TTL: Duration = Duration::from_secs(120);
/// Teto de destinos listados por processo — a tela não é um log.
const MAX_DESTINATIONS_PER_PROCESS: usize = 24;
/// Um PowerShell travado não pode segurar a coleta para sempre.
const POSTURE_COMMAND_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEndpoint {
    pub address: String,
    pub port: u16,
    pub is_ipv6: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTraffic {
    pub pid: u32,
    /// Caminho completo. `None` quando o processo é protegido (elevado/PPL) e o
    /// Pulse não está elevado — nunca inventamos um nome.
    pub executable_path: Option<String>,
    pub process_name: String,
    pub established_count: usize,
    pub udp_socket_count: usize,
    pub listening_count: usize,
    /// `true` quando alguma porta escuta em `0.0.0.0`/`::`, ou seja, alcançável
    /// pela LAN — não só pelo próprio micro.
    pub listens_on_all_interfaces: bool,
    /// Destinos externos (fora de loopback), sem repetição, limitados ao teto.
    pub destinations: Vec<RemoteEndpoint>,
    /// Total de conexões externas, incluindo as que excederam o teto.
    pub external_total: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSnapshot {
    pub processes: Vec<ProcessTraffic>,
    pub established_total: usize,
    pub established_external: usize,
    pub established_loopback: usize,
    pub udp_total: usize,
    pub listening_total: usize,
    /// Portas escutando em todas as interfaces.
    pub listening_public_total: usize,
    /// Preenchido quando a tabela não pôde ser lida. Ausência de dado é
    /// declarada, nunca apresentada como "nenhuma conexão".
    pub unavailable_reason: Option<String>,
    pub collected_at_unix_ms: u64,
}

impl NetworkSnapshot {
    fn unavailable(reason: impl Into<String>) -> Self {
        NetworkSnapshot {
            processes: Vec::new(),
            established_total: 0,
            established_external: 0,
            established_loopback: 0,
            udp_total: 0,
            listening_total: 0,
            listening_public_total: 0,
            unavailable_reason: Some(reason.into()),
            collected_at_unix_ms: now_unix_ms(),
        }
    }
}

/// `Unavailable` existe para que "não deu para medir" nunca vire "está tudo bem".
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SignalSeverity {
    Ok,
    Attention,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostureSignal {
    /// Identificador estável, para a interface agrupar.
    pub id: &'static str,
    pub title: &'static str,
    pub severity: SignalSeverity,
    /// Valor medido, em uma frase.
    pub summary: String,
    /// Por que isso importa, em uma frase.
    pub rationale: &'static str,
    /// O que foi encontrado, item a item.
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityPosture {
    pub signals: Vec<PostureSignal>,
    pub attention_count: usize,
    pub unavailable_count: usize,
    pub collected_at_unix_ms: u64,
    /// `true` quando o processo do Pulse tem token elevado. Sinais que exigem
    /// administrador são marcados como indisponíveis sem ele.
    pub elevated: bool,
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

static CONNECTIONS_CACHE: Mutex<Option<(NetworkSnapshot, Instant)>> = Mutex::new(None);
static POSTURE_CACHE: Mutex<Option<(SecurityPosture, Instant)>> = Mutex::new(None);

pub fn network_snapshot(force: bool) -> NetworkSnapshot {
    if !force {
        if let Ok(guard) = CONNECTIONS_CACHE.lock() {
            if let Some((snapshot, fetched_at)) = guard.as_ref() {
                if fetched_at.elapsed() < CONNECTIONS_TTL {
                    return snapshot.clone();
                }
            }
        }
    }

    let snapshot = platform::collect_network();

    if let Ok(mut guard) = CONNECTIONS_CACHE.lock() {
        *guard = Some((snapshot.clone(), Instant::now()));
    }
    snapshot
}

pub fn security_posture(force: bool) -> SecurityPosture {
    if !force {
        if let Ok(guard) = POSTURE_CACHE.lock() {
            if let Some((posture, fetched_at)) = guard.as_ref() {
                if fetched_at.elapsed() < POSTURE_TTL {
                    return posture.clone();
                }
            }
        }
    }

    let posture = platform::collect_posture();

    if let Ok(mut guard) = POSTURE_CACHE.lock() {
        *guard = Some((posture.clone(), Instant::now()));
    }
    posture
}

// ---------------------------------------------------------------------------
// Comandos Tauri
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_network_connections(force_refresh: Option<bool>) -> NetworkSnapshot {
    let force = force_refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || network_snapshot(force))
        .await
        .unwrap_or_else(|error| {
            NetworkSnapshot::unavailable(format!("Falha ao coletar a rede: {error}"))
        })
}

#[tauri::command]
pub async fn get_security_posture(force_refresh: Option<bool>) -> SecurityPosture {
    let force = force_refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || security_posture(force))
        .await
        .unwrap_or_else(|error| SecurityPosture {
            signals: vec![PostureSignal {
                id: "collection",
                title: "Sinais de postura",
                severity: SignalSeverity::Unavailable,
                summary: format!("Falha ao coletar: {error}"),
                rationale: "Sem coleta não há como afirmar que o sistema está em ordem.",
                evidence: Vec::new(),
            }],
            attention_count: 0,
            unavailable_count: 1,
            collected_at_unix_ms: now_unix_ms(),
            elevated: false,
        })
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use super::{
        format_evidence_line, now_unix_ms, run_powershell, NetworkSnapshot, PostureSignal,
        ProcessTraffic, RemoteEndpoint, SecurityPosture, SignalSeverity,
        MAX_DESTINATIONS_PER_PROCESS,
    };
    use std::collections::HashMap;
    use std::net::{Ipv4Addr, Ipv6Addr};

    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID,
        MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, MIB_TCP_STATE_ESTAB, MIB_TCP_STATE_LISTEN,
        MIB_UDP6ROW_OWNER_PID, MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW_OWNER_PID,
        MIB_UDPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    /// Folga entre a chamada de sondagem e a de leitura: a tabela cresce se um
    /// processo abre socket no meio das duas.
    const TABLE_SLACK: usize = 64 * 1024;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum SocketKind {
        Established,
        Listening,
        Udp,
    }

    struct SocketRow {
        kind: SocketKind,
        pid: u32,
        local_addr: String,
        remote_addr: Option<String>,
        remote_port: u16,
        is_ipv6: bool,
    }

    impl SocketRow {
        fn listens_on_all_interfaces(&self) -> bool {
            self.kind == SocketKind::Listening
                && (self.local_addr == "0.0.0.0" || self.local_addr == "::")
        }

        fn remote_is_loopback(&self) -> bool {
            self.remote_addr
                .as_deref()
                .map(|address| {
                    address.starts_with("127.") || address == "::1" || address == "0:0:0:0:0:0:0:1"
                })
                .unwrap_or(true)
        }
    }

    pub fn collect_network() -> NetworkSnapshot {
        let mut rows = Vec::new();
        let mut failures = Vec::new();

        for (af, label) in [(AF_INET, "IPv4"), (AF_INET6, "IPv6")] {
            if let Err(error) = collect_tcp(af, &mut rows) {
                failures.push(format!("TCP {label}: {error}"));
            }
        }
        for (af, label) in [(AF_INET, "IPv4"), (AF_INET6, "IPv6")] {
            if let Err(error) = collect_udp(af, &mut rows) {
                failures.push(format!("UDP {label}: {error}"));
            }
        }

        // Nada lido e houve falha: a tabela inteira está indisponível. Não é
        // "zero conexões".
        if rows.is_empty() && !failures.is_empty() {
            return NetworkSnapshot::unavailable(failures.join("; "));
        }

        let mut by_pid: HashMap<u32, ProcessTraffic> = HashMap::new();
        let mut established_total = 0usize;
        let mut established_external = 0usize;
        let mut established_loopback = 0usize;
        let mut udp_total = 0usize;
        let mut listening_total = 0usize;
        let mut listening_public_total = 0usize;

        for row in &rows {
            let entry = by_pid.entry(row.pid).or_insert_with(|| ProcessTraffic {
                pid: row.pid,
                executable_path: None,
                process_name: String::new(),
                established_count: 0,
                udp_socket_count: 0,
                listening_count: 0,
                listens_on_all_interfaces: false,
                destinations: Vec::new(),
                external_total: 0,
            });

            match row.kind {
                SocketKind::Established => {
                    established_total += 1;
                    entry.established_count += 1;
                    if row.remote_is_loopback() {
                        established_loopback += 1;
                        continue;
                    }
                    established_external += 1;
                    entry.external_total += 1;
                    if entry.destinations.len() < MAX_DESTINATIONS_PER_PROCESS {
                        let address = row.remote_addr.clone().unwrap_or_default();
                        let duplicate = entry.destinations.iter().any(|existing| {
                            existing.address == address && existing.port == row.remote_port
                        });
                        if !duplicate {
                            entry.destinations.push(RemoteEndpoint {
                                address,
                                port: row.remote_port,
                                is_ipv6: row.is_ipv6,
                            });
                        }
                    }
                }
                SocketKind::Listening => {
                    listening_total += 1;
                    entry.listening_count += 1;
                    if row.listens_on_all_interfaces() {
                        listening_public_total += 1;
                        entry.listens_on_all_interfaces = true;
                    }
                }
                SocketKind::Udp => {
                    udp_total += 1;
                    entry.udp_socket_count += 1;
                }
            }
        }

        // Resolve o executável de cada PID uma única vez.
        let mut resolved: HashMap<u32, Option<String>> = HashMap::new();
        for (pid, traffic) in by_pid.iter_mut() {
            if *pid == 0 {
                traffic.process_name = "Sistema inativo (PID 0)".to_string();
                continue;
            }
            let path = resolved
                .entry(*pid)
                .or_insert_with(|| image_path(*pid))
                .clone();
            match path {
                Some(path) => {
                    traffic.process_name =
                        path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
                    traffic.executable_path = Some(path);
                }
                None => {
                    // Honestidade: Pulse não elevado ou processo PPL.
                    traffic.process_name = format!("Processo protegido (PID {pid})");
                }
            }
        }

        let mut processes: Vec<ProcessTraffic> = by_pid
            .into_values()
            .filter(|traffic| {
                traffic.established_count > 0
                    || traffic.listening_count > 0
                    || traffic.udp_socket_count > 0
            })
            .collect();
        processes.sort_by(|left, right| {
            right
                .external_total
                .cmp(&left.external_total)
                .then_with(|| right.established_count.cmp(&left.established_count))
                .then_with(|| right.log_total().cmp(&left.log_total()))
                .then_with(|| left.process_name.cmp(&right.process_name))
        });
        for traffic in processes.iter_mut() {
            traffic.destinations.sort_by(|left, right| {
                left.address
                    .cmp(&right.address)
                    .then_with(|| left.port.cmp(&right.port))
            });
        }

        NetworkSnapshot {
            processes,
            established_total,
            established_external,
            established_loopback,
            udp_total,
            listening_total,
            listening_public_total,
            unavailable_reason: (!failures.is_empty())
                .then(|| format!("Coleta parcial: {}", failures.join("; "))),
            collected_at_unix_ms: now_unix_ms(),
        }
    }

    impl ProcessTraffic {
        fn log_total(&self) -> usize {
            self.listening_count + self.udp_socket_count
        }
    }

    fn collect_tcp(af: u32, out: &mut Vec<SocketRow>) -> Result<(), String> {
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
                let Some(kind) = tcp_kind(row.dwState) else {
                    continue;
                };
                let local_port = u16::from_be(row.dwLocalPort as u16);
                if local_port == 0 {
                    continue;
                }
                out.push(SocketRow {
                    kind,
                    pid: row.dwOwningPid,
                    local_addr: Ipv4Addr::from(u32::from_be(row.dwLocalAddr)).to_string(),
                    remote_addr: (kind == SocketKind::Established)
                        .then(|| Ipv4Addr::from(u32::from_be(row.dwRemoteAddr)).to_string()),
                    remote_port: u16::from_be(row.dwRemotePort as u16),
                    is_ipv6: false,
                });
            }
        } else {
            let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            let count = unsafe { (*table).dwNumEntries as usize };
            let rows_ptr =
                unsafe { std::ptr::addr_of!((*table).table).cast::<MIB_TCP6ROW_OWNER_PID>() };
            for row in unsafe { std::slice::from_raw_parts(rows_ptr, count) } {
                let Some(kind) = tcp_kind(row.dwState) else {
                    continue;
                };
                let local_port = u16::from_be(row.dwLocalPort as u16);
                if local_port == 0 {
                    continue;
                }
                out.push(SocketRow {
                    kind,
                    pid: row.dwOwningPid,
                    local_addr: Ipv6Addr::from(row.ucLocalAddr).to_string(),
                    remote_addr: (kind == SocketKind::Established)
                        .then(|| Ipv6Addr::from(row.ucRemoteAddr).to_string()),
                    remote_port: u16::from_be(row.dwRemotePort as u16),
                    is_ipv6: true,
                });
            }
        }
        Ok(())
    }

    fn tcp_kind(state: u32) -> Option<SocketKind> {
        if state == MIB_TCP_STATE_ESTAB.0 as u32 {
            Some(SocketKind::Established)
        } else if state == MIB_TCP_STATE_LISTEN.0 as u32 {
            Some(SocketKind::Listening)
        } else {
            None
        }
    }

    fn collect_udp(af: u32, out: &mut Vec<SocketRow>) -> Result<(), String> {
        let mut size: u32 = 0;
        let first =
            unsafe { GetExtendedUdpTable(None, &mut size, false, af, UDP_TABLE_OWNER_PID, 0) };
        if first != ERROR_INSUFFICIENT_BUFFER.0 && first != NO_ERROR.0 {
            return Err(format!("GetExtendedUdpTable retornou {first}"));
        }
        if size == 0 {
            return Ok(());
        }

        let mut buffer = vec![0u8; size as usize + TABLE_SLACK];
        let mut used = buffer.len() as u32;
        loop {
            let status = unsafe {
                GetExtendedUdpTable(
                    Some(buffer.as_mut_ptr().cast()),
                    &mut used,
                    false,
                    af,
                    UDP_TABLE_OWNER_PID,
                    0,
                )
            };
            if status == ERROR_INSUFFICIENT_BUFFER.0 {
                buffer.resize(used as usize + TABLE_SLACK, 0);
                used = buffer.len() as u32;
                continue;
            }
            if status != NO_ERROR.0 {
                return Err(format!("GetExtendedUdpTable falhou com {status}"));
            }
            break;
        }

        if af == AF_INET {
            let table = buffer.as_ptr().cast::<MIB_UDPTABLE_OWNER_PID>();
            let count = unsafe { (*table).dwNumEntries as usize };
            let rows_ptr =
                unsafe { std::ptr::addr_of!((*table).table).cast::<MIB_UDPROW_OWNER_PID>() };
            for row in unsafe { std::slice::from_raw_parts(rows_ptr, count) } {
                let local_port = u16::from_be(row.dwLocalPort as u16);
                if local_port == 0 {
                    continue;
                }
                out.push(SocketRow {
                    kind: SocketKind::Udp,
                    pid: row.dwOwningPid,
                    local_addr: Ipv4Addr::from(u32::from_be(row.dwLocalAddr)).to_string(),
                    remote_addr: None,
                    remote_port: 0,
                    is_ipv6: false,
                });
            }
        } else {
            let table = buffer.as_ptr().cast::<MIB_UDP6TABLE_OWNER_PID>();
            let count = unsafe { (*table).dwNumEntries as usize };
            let rows_ptr =
                unsafe { std::ptr::addr_of!((*table).table).cast::<MIB_UDP6ROW_OWNER_PID>() };
            for row in unsafe { std::slice::from_raw_parts(rows_ptr, count) } {
                let local_port = u16::from_be(row.dwLocalPort as u16);
                if local_port == 0 {
                    continue;
                }
                out.push(SocketRow {
                    kind: SocketKind::Udp,
                    pid: row.dwOwningPid,
                    local_addr: Ipv6Addr::from(row.ucLocalAddr).to_string(),
                    remote_addr: None,
                    remote_port: 0,
                    is_ipv6: true,
                });
            }
        }
        Ok(())
    }

    fn image_path(pid: u32) -> Option<String> {
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

    // -- Postura -------------------------------------------------------------

    /// Um único PowerShell devolve todos os sinais caros. Separar em chamadas
    /// custaria ~200 ms cada (inicialização do runtime); juntos custam uma só.
    const POSTURE_COMMAND: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$out = New-Object System.Collections.ArrayList

# 1) Filtros da classe de teclado (keylogger de driver)
$kb = 'SYSTEM\CurrentControlSet\Control\Class\{4d36e96b-e325-11ce-bfc1-08002be10318}'
try {
  $upper = (Get-ItemProperty -Path ("HKLM:\" + $kb) -Name UpperFilters -ErrorAction Stop).UpperFilters
  if ($null -eq $upper) { $upper = @() }
  [void]$out.Add(('KEYBOARD|' + ($upper -join ',')))
} catch { [void]$out.Add('KEYBOARD|ERR') }

# 2) Proxy do usuario
try {
  $ie = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction Stop
  [void]$out.Add(('PROXY|' + [string]$ie.ProxyEnable + '|' + [string]$ie.ProxyServer + '|' + [string]$ie.AutoConfigURL))
} catch { [void]$out.Add('PROXY|ERR') }

# 3) Raizes nao-Microsoft instaladas desde 2025
try {
  $since = [datetime]'2025-01-01'
  $roots = @(Get-ChildItem Cert:\LocalMachine\Root -ErrorAction Stop | Where-Object { $_.NotBefore -ge $since -and $_.Subject -notlike '*Microsoft*' })
  $total = @(Get-ChildItem Cert:\LocalMachine\Root -ErrorAction Stop | Where-Object { $_.Subject -notlike '*Microsoft*' }).Count
  $items = ($roots | ForEach-Object { ($_.Subject -replace '\|',' ') + ' (em ' + $_.NotBefore.ToString('yyyy-MM-dd') + ')' }) -join ' ;; '
  [void]$out.Add(('ROOTS|' + $total + '|' + $roots.Count + '|' + $items))
} catch { [void]$out.Add('ROOTS|ERR') }

# 4) Defender
try {
  $d = Get-MpComputerStatus -ErrorAction Stop
  [void]$out.Add(('DEFENDER|' + [string]$d.RealTimeProtectionEnabled + '|' + [string]$d.AntivirusSignatureAge + '|' + [string]$d.AntivirusSignatureVersion))
} catch { [void]$out.Add('DEFENDER|ERR') }

# 5) Visibilidade de auditoria: existe Sysmon? O log de Seguranca tem registros?
try {
  $sysmon = [bool](Get-Service -Name 'Sysmon*' -ErrorAction SilentlyContinue)
  $secCount = 0
  try { $secCount = [int]@(Get-WinEvent -ListLog 'Security' -ErrorAction Stop).RecordCount } catch { $secCount = -1 }
  [void]$out.Add(('AUDIT|' + $sysmon + '|' + $secCount))
} catch { [void]$out.Add('AUDIT|ERR') }

$out -join "`n"
"#;

    pub fn collect_posture() -> SecurityPosture {
        let elevated = is_elevated();
        let mut signals = Vec::new();

        let raw = run_powershell(POSTURE_COMMAND);
        let mut lines: HashMap<&str, Vec<String>> = HashMap::new();
        match raw {
            Err(error) => {
                signals.push(unavailable(
                    "posture_collection",
                    "Sinais de postura",
                    error,
                ));
            }
            Ok(output) => {
                for line in output.lines() {
                    let mut parts = line.trim().split('|');
                    let Some(tag) = parts.next() else { continue };
                    let rest: Vec<String> = parts.map(str::to_string).collect();
                    lines.insert(
                        match tag {
                            "KEYBOARD" => "KEYBOARD",
                            "PROXY" => "PROXY",
                            "ROOTS" => "ROOTS",
                            "DEFENDER" => "DEFENDER",
                            "AUDIT" => "AUDIT",
                            _ => continue,
                        },
                        rest,
                    );
                }

                signals.push(keyboard_signal(lines.get("KEYBOARD")));
                signals.push(proxy_signal(lines.get("PROXY")));
                signals.push(root_signal(lines.get("ROOTS")));
                signals.push(defender_signal(lines.get("DEFENDER")));
                signals.push(audit_signal(lines.get("AUDIT"), elevated));
            }
        }

        let attention_count = signals
            .iter()
            .filter(|signal| signal.severity == SignalSeverity::Attention)
            .count();
        let unavailable_count = signals
            .iter()
            .filter(|signal| signal.severity == SignalSeverity::Unavailable)
            .count();

        SecurityPosture {
            signals,
            attention_count,
            unavailable_count,
            collected_at_unix_ms: now_unix_ms(),
            elevated,
        }
    }

    fn unavailable(id: &'static str, title: &'static str, reason: String) -> PostureSignal {
        PostureSignal {
            id,
            title,
            severity: SignalSeverity::Unavailable,
            summary: format!("Não foi possível medir: {reason}"),
            rationale: "Sem medição não há como afirmar que este ponto está em ordem.",
            evidence: Vec::new(),
        }
    }

    fn keyboard_signal(values: Option<&Vec<String>>) -> PostureSignal {
        let rationale = "Todo driver nessa classe vê cada tecla antes do sistema.";
        let Some(values) = values else {
            return unavailable("keyboard_filters", "Filtros de teclado", "sem dados".into());
        };
        let Some(raw) = values.first() else {
            return unavailable(
                "keyboard_filters",
                "Filtros de teclado",
                "resposta vazia".into(),
            );
        };
        if raw == "ERR" {
            return unavailable(
                "keyboard_filters",
                "Filtros de teclado",
                "leitura do registro negada".into(),
            );
        }

        let filters: Vec<&str> = raw
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect();
        let suspicious: Vec<&&str> = filters
            .iter()
            .filter(|filter| !filter.eq_ignore_ascii_case("kbdclass"))
            .collect();

        if suspicious.is_empty() {
            PostureSignal {
                id: "keyboard_filters",
                title: "Filtros de teclado",
                severity: SignalSeverity::Ok,
                summary: if filters.is_empty() {
                    "Nenhum filtro declarado na classe de teclado".to_string()
                } else {
                    "Somente o driver padrão do Windows (kbdclass)".to_string()
                },
                rationale,
                evidence: vec![format_evidence_line("UpperFilters", raw)],
            }
        } else {
            PostureSignal {
                id: "keyboard_filters",
                title: "Filtros de teclado",
                severity: SignalSeverity::Attention,
                summary: format!(
                    "Filtro de terceiro na classe de teclado: {}",
                    suspicious
                        .iter()
                        .map(|item| item.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                rationale,
                evidence: vec![format_evidence_line("UpperFilters", raw)],
            }
        }
    }

    fn proxy_signal(values: Option<&Vec<String>>) -> PostureSignal {
        let rationale = "Um proxy não declarado é o caminho mais curto para interceptar HTTPS.";
        let Some(values) = values else {
            return unavailable("system_proxy", "Proxy do sistema", "sem dados".into());
        };
        if values.first().map(String::as_str) == Some("ERR") {
            return unavailable(
                "system_proxy",
                "Proxy do sistema",
                "leitura da configuração falhou".into(),
            );
        }
        let enabled = values.first().map(String::as_str) == Some("1");
        let server = values.get(1).cloned().unwrap_or_default();
        let auto = values.get(2).cloned().unwrap_or_default();
        let server = server
            .trim()
            .trim_start_matches(|c: char| c == '0' && server.trim().len() == 1)
            .to_string();
        let server = if server.is_empty() {
            String::new()
        } else {
            server
        };
        let auto = if auto.trim().is_empty() {
            String::new()
        } else {
            auto
        };

        if enabled && !server.is_empty() {
            PostureSignal {
                id: "system_proxy",
                title: "Proxy do sistema",
                severity: SignalSeverity::Attention,
                summary: format!("Proxy ativo: {}", server.trim()),
                rationale,
                evidence: vec!["Confirme se foi você quem configurou esse proxy.".to_string()],
            }
        } else if !auto.is_empty() {
            PostureSignal {
                id: "system_proxy",
                title: "Proxy do sistema",
                severity: SignalSeverity::Attention,
                summary: format!("Script de configuração automática: {}", auto.trim()),
                rationale,
                evidence: vec![
                    "Um PAC remoto pode redirecionar o tráfego sem aparecer como proxy."
                        .to_string(),
                ],
            }
        } else {
            PostureSignal {
                id: "system_proxy",
                title: "Proxy do sistema",
                severity: SignalSeverity::Ok,
                summary: "Sem proxy no perfil do usuário".to_string(),
                rationale,
                evidence: Vec::new(),
            }
        }
    }

    fn root_signal(values: Option<&Vec<String>>) -> PostureSignal {
        let rationale = "Uma raiz instalada permite assinar certificados aceitos sem aviso.";
        let Some(values) = values else {
            return unavailable(
                "root_certificates",
                "Raízes de confiança",
                "sem dados".into(),
            );
        };
        if values.first().map(String::as_str) == Some("ERR") {
            return unavailable(
                "root_certificates",
                "Raízes de confiança",
                "enumeração do store falhou".into(),
            );
        }
        let total: usize = values
            .first()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let recent: usize = values
            .get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let items: Vec<String> = values
            .get(2)
            .map(|value| {
                value
                    .split(" ;; ")
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        if recent == 0 {
            PostureSignal {
                id: "root_certificates",
                title: "Raízes de confiança",
                severity: SignalSeverity::Ok,
                summary: format!(
                    "{total} raízes de terceiros no store da máquina, nenhuma instalada desde 2025"
                ),
                rationale,
                evidence: Vec::new(),
            }
        } else {
            PostureSignal {
                id: "root_certificates",
                title: "Raízes de confiança",
                severity: SignalSeverity::Attention,
                summary: format!("{total} raízes de terceiros; {recent} instaladas desde 2025"),
                rationale,
                evidence: items,
            }
        }
    }

    fn defender_signal(values: Option<&Vec<String>>) -> PostureSignal {
        let rationale = "É a proteção ativa da máquina; o Pulse não substitui esse papel.";
        let Some(values) = values else {
            return unavailable("defender", "Microsoft Defender", "sem dados".into());
        };
        if values.first().map(String::as_str) == Some("ERR") {
            return unavailable(
                "defender",
                "Microsoft Defender",
                "consulta ao Defender falhou (pode estar desativado ou substituído)".into(),
            );
        }
        let real_time = values.first().map(String::as_str) == Some("True");
        let age: i64 = values
            .get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(-1);
        let version = values.get(2).cloned().unwrap_or_default();

        if !real_time {
            PostureSignal {
                id: "defender",
                title: "Microsoft Defender",
                severity: SignalSeverity::Attention,
                summary: "Proteção em tempo real desligada".to_string(),
                rationale,
                evidence: vec![format_evidence_line("Versão de assinaturas", &version)],
            }
        } else if age < 0 {
            PostureSignal {
                id: "defender",
                title: "Microsoft Defender",
                severity: SignalSeverity::Unavailable,
                summary: "Proteção ativa, idade das assinaturas desconhecida".to_string(),
                rationale,
                evidence: Vec::new(),
            }
        } else if age > 7 {
            PostureSignal {
                id: "defender",
                title: "Microsoft Defender",
                severity: SignalSeverity::Attention,
                summary: format!("Proteção ativa, mas assinaturas com {age} dias"),
                rationale,
                evidence: vec![format_evidence_line("Versão de assinaturas", &version)],
            }
        } else {
            PostureSignal {
                id: "defender",
                title: "Microsoft Defender",
                severity: SignalSeverity::Ok,
                summary: format!("Proteção ativa, assinaturas com {age} dia(s)"),
                rationale,
                evidence: Vec::new(),
            }
        }
    }

    fn audit_signal(values: Option<&Vec<String>>, elevated: bool) -> PostureSignal {
        let rationale = "Sem esse log, um processo ou serviço instalado ontem é invisível hoje.";
        let Some(values) = values else {
            return unavailable(
                "audit_visibility",
                "Visibilidade de auditoria",
                "sem dados".into(),
            );
        };
        if values.first().map(String::as_str) == Some("ERR") {
            return unavailable(
                "audit_visibility",
                "Visibilidade de auditoria",
                "consulta ao log de eventos falhou".into(),
            );
        }
        let has_sysmon = values.first().map(String::as_str) == Some("True");
        let records: i64 = values
            .get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(-1);

        // `-1` = o canal existe mas a leitura foi negada (não elevado).
        if records == 0 && !has_sysmon {
            PostureSignal {
                id: "audit_visibility",
                title: "Visibilidade de auditoria",
                severity: SignalSeverity::Attention,
                summary: "Log de Segurança vazio e sem Sysmon: o passado não é investigável".to_string(),
                rationale,
                evidence: vec![
                    "Ligar exige administrador: auditoria de criação de processo (4688) e de instalação de serviço (4697/7045).".to_string(),
                ],
            }
        } else if records < 0 {
            PostureSignal {
                id: "audit_visibility",
                title: "Visibilidade de auditoria",
                severity: if elevated {
                    SignalSeverity::Attention
                } else {
                    SignalSeverity::Unavailable
                },
                summary: if elevated {
                    "Log de Segurança não pôde ser lido mesmo com privilégio elevado".to_string()
                } else {
                    "Sem privilégio elevado não é possível confirmar a auditoria".to_string()
                },
                rationale,
                evidence: vec![
                    "Reinicie o Pulse como administrador para medir este item.".to_string()
                ],
            }
        } else {
            PostureSignal {
                id: "audit_visibility",
                title: "Visibilidade de auditoria",
                severity: SignalSeverity::Ok,
                summary: if has_sysmon {
                    "Sysmon instalado; o histórico de processos é registrado".to_string()
                } else {
                    format!("Log de Segurança com {records} registros")
                },
                rationale,
                evidence: Vec::new(),
            }
        }
    }

    fn is_elevated() -> bool {
        crate::privileges::is_elevated_cached()
    }
}

/// Linha de evidência padronizada (`Rótulo: valor`), para a interface renderizar
/// como lista sem precisar parsear.
fn format_evidence_line(label: &str, value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        format!("{label}: (vazio)")
    } else {
        format!("{label}: {value}")
    }
}

/// Executa um script PowerShell fixo (nunca interpolado com dado do IPC) sem
/// janela de console e com timeout. Usa o helper do projeto para manter a regra
/// de "nenhum subprocesso cria janela".
fn run_powershell(script: &str) -> Result<String, String> {
    use std::process::Stdio;

    let mut child = crate::process::hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Não foi possível iniciar o PowerShell: {error}"))?;

    let deadline = Instant::now() + POSTURE_COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("O Windows demorou demais para responder".to_string());
                }
                std::thread::sleep(Duration::from_millis(60));
            }
            Err(error) => return Err(format!("Falha ao acompanhar a consulta: {error}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("Falha ao ler a saída: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("saída vazia");
        return Err(detail.to_string());
    }
    Ok(stdout)
}
