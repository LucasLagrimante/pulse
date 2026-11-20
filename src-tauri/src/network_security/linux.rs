//! Coleta de rede e sinais de postura de segurança no Linux.
//!
//! Princípio: **evidência, nunca rótulo** e **zero dado fictício**.
//! - Conexões vêm direto de `/proc/net/{tcp,tcp6,udp,udp6}` e do mapa
//!   inode → PID em `/proc/<pid>/fd` (sem spawn de subprocesso a cada 2 s).
//! - Sinais de postura usam apenas arquivos do kernel (`/proc`, `/sys`),
//!   configurações do sistema e comandos fixos com `LC_ALL=C` e timeout curto.

use super::{
    format_evidence_line, now_unix_ms, NetworkSnapshot, PostureSignal, ProcessTraffic,
    RemoteEndpoint, SecurityPosture, SignalSeverity, MAX_DESTINATIONS_PER_PROCESS,
};
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

const CLI_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CERT_EVIDENCE: usize = 30;
const CA_ANCHOR_DIRS: &[&str] = &[
    "/usr/local/share/ca-certificates",
    "/etc/ca-certificates/trust-source/anchors",
    "/etc/pki/ca-trust/source/anchors",
];
const PROXY_ENV_VARS: &[&str] = &[
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketKind {
    Established,
    Listening,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableProto {
    Tcp4,
    Tcp6,
    Udp4,
    Udp6,
}

impl TableProto {
    fn is_ipv6(self) -> bool {
        matches!(self, Self::Tcp6 | Self::Udp6)
    }

    fn is_udp(self) -> bool {
        matches!(self, Self::Udp4 | Self::Udp6)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSocket {
    kind: SocketKind,
    local_ip: IpAddr,
    remote_ip: Option<IpAddr>,
    remote_port: u16,
    is_ipv6: bool,
    inode: u64,
}

impl RawSocket {
    fn listens_on_all_interfaces(&self) -> bool {
        self.kind == SocketKind::Listening && self.local_ip.is_unspecified()
    }

    fn remote_is_loopback(&self) -> bool {
        self.remote_ip.map(is_loopback_ip).unwrap_or(true)
    }
}

fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

fn is_elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}

// ---------------------------------------------------------------------------
// Conexões de rede (/proc/net/* + /proc/<pid>/fd)
// ---------------------------------------------------------------------------

pub fn collect_network() -> NetworkSnapshot {
    let tables = [
        ("/proc/net/tcp", TableProto::Tcp4, "TCP IPv4", false),
        ("/proc/net/tcp6", TableProto::Tcp6, "TCP IPv6", true),
        ("/proc/net/udp", TableProto::Udp4, "UDP IPv4", false),
        ("/proc/net/udp6", TableProto::Udp6, "UDP IPv6", true),
    ];

    let mut raw_sockets = Vec::new();
    let mut readable_tables = 0usize;
    let mut failures = Vec::new();

    for (path, proto, label, optional_not_found) in tables {
        match fs::read_to_string(path) {
            Ok(content) => {
                readable_tables += 1;
                parse_proc_net_table(&content, proto, &mut raw_sockets);
            }
            Err(error) => {
                // IPv6 pode estar desabilitado no kernel: só tratamos como
                // erro parcial se não for ausência simples do arquivo IPv6.
                if !(optional_not_found && error.kind() == ErrorKind::NotFound) {
                    failures.push(format!("{label} ({path}): {error}"));
                }
            }
        }
    }

    if readable_tables == 0 {
        let reason = if failures.is_empty() {
            "Nenhuma tabela /proc/net/* disponível para leitura".to_string()
        } else {
            failures.join("; ")
        };
        crate::debug::record_failure(
            "get_network_connections",
            0,
            crate::debug::Failure::new("Falha ao ler /proc/net/*")
                .stage("proc_net")
                .detail(&reason),
        );
        return NetworkSnapshot::unavailable(reason);
    }

    let inode_to_pid = build_inode_pid_map();
    let elevated = is_elevated();
    let mut snapshot = aggregate_sockets(&raw_sockets, &inode_to_pid, resolve_pid_info, elevated);

    if !failures.is_empty() {
        snapshot.unavailable_reason = Some(format!("Coleta parcial: {}", failures.join("; ")));
    }
    snapshot
}

fn parse_proc_ipv4(hex: &str) -> Option<Ipv4Addr> {
    if hex.len() != 8 {
        return None;
    }
    let raw = u32::from_str_radix(hex, 16).ok()?;
    Some(Ipv4Addr::from(raw.to_ne_bytes()))
}

fn parse_proc_ipv6(hex: &str) -> Option<Ipv6Addr> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for idx in 0..4 {
        let start = idx * 8;
        let word = u32::from_str_radix(&hex[start..start + 8], 16).ok()?;
        bytes[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_ne_bytes());
    }
    Some(Ipv6Addr::from(bytes))
}

fn parse_endpoint(token: &str, is_ipv6: bool) -> Option<(IpAddr, u16)> {
    let (addr_hex, port_hex) = token.split_once(':')?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    let ip = if is_ipv6 {
        IpAddr::V6(parse_proc_ipv6(addr_hex)?)
    } else {
        IpAddr::V4(parse_proc_ipv4(addr_hex)?)
    };
    Some((ip, port))
}

fn parse_proc_net_table(content: &str, proto: TableProto, out: &mut Vec<RawSocket>) {
    let is_ipv6 = proto.is_ipv6();
    let is_udp = proto.is_udp();

    for line in content.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let Some(_sl) = cols.next() else { continue };
        let Some(local_tok) = cols.next() else {
            continue;
        };
        let Some(remote_tok) = cols.next() else {
            continue;
        };
        let Some(state_hex) = cols.next() else {
            continue;
        };
        // Pula tx_queue:rx_queue, tr:tm->when, retrnsmt, uid, timeout (5 colunas)
        let Some(inode_str) = cols.nth(5) else {
            continue;
        };

        let kind = if is_udp {
            SocketKind::Udp
        } else {
            match u8::from_str_radix(state_hex, 16).ok() {
                Some(0x01) => SocketKind::Established,
                Some(0x0A) => SocketKind::Listening,
                _ => continue,
            }
        };

        let Some((local_ip, local_port)) = parse_endpoint(local_tok, is_ipv6) else {
            continue;
        };
        if local_port == 0 {
            continue;
        }

        let (remote_ip, remote_port) = if kind == SocketKind::Established {
            let Some((ip, port)) = parse_endpoint(remote_tok, is_ipv6) else {
                continue;
            };
            (Some(ip), port)
        } else {
            (None, 0)
        };

        let inode = inode_str.parse::<u64>().unwrap_or(0);
        out.push(RawSocket {
            kind,
            local_ip,
            remote_ip,
            remote_port,
            is_ipv6,
            inode,
        });
    }
}

fn build_inode_pid_map() -> HashMap<u64, u32> {
    let mut map: HashMap<u64, u32> = HashMap::new();
    let Ok(proc_entries) = fs::read_dir("/proc") else {
        return map;
    };

    for entry in proc_entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Ok(pid) = name_str.parse::<u32>() else {
            continue;
        };

        let fd_dir = entry.path().join("fd");
        let Ok(fds) = fs::read_dir(fd_dir) else {
            continue;
        };

        for fd_entry in fds.flatten() {
            let Ok(target) = fs::read_link(fd_entry.path()) else {
                continue;
            };
            let Some(target_str) = target.to_str() else {
                continue;
            };
            if let Some(inode) = parse_socket_inode(target_str) {
                // Em caso de socket herdado por fork, mantém o menor PID para
                // determinismo (em geral o processo pai dono original).
                map.entry(inode)
                    .and_modify(|existing| {
                        if pid < *existing {
                            *existing = pid;
                        }
                    })
                    .or_insert(pid);
            }
        }
    }

    map
}

fn parse_socket_inode(link_target: &str) -> Option<u64> {
    let rest = link_target.strip_prefix("socket:[")?;
    let digits = rest.strip_suffix(']')?;
    digits.parse::<u64>().ok()
}

fn clean_exe_link(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches(" (deleted)");
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn resolve_pid_info(pid: u32) -> (String, Option<String>) {
    let exe = fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| clean_exe_link(&path.to_string_lossy()));

    let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());

    let name = comm
        .or_else(|| {
            exe.as_deref()
                .and_then(|path| path.rsplit('/').next())
                .filter(|base| !base.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("Processo (PID {pid})"));

    (name, exe)
}

fn aggregate_sockets(
    sockets: &[RawSocket],
    inode_to_pid: &HashMap<u64, u32>,
    resolve_pid: impl Fn(u32) -> (String, Option<String>),
    elevated: bool,
) -> NetworkSnapshot {
    let mut by_pid: HashMap<u32, ProcessTraffic> = HashMap::new();
    let mut established_total = 0usize;
    let mut established_external = 0usize;
    let mut established_loopback = 0usize;
    let mut udp_total = 0usize;
    let mut listening_total = 0usize;
    let mut listening_public_total = 0usize;

    for row in sockets {
        let pid = if row.inode == 0 {
            0
        } else {
            inode_to_pid.get(&row.inode).copied().unwrap_or(0)
        };

        let entry = by_pid.entry(pid).or_insert_with(|| ProcessTraffic {
            pid,
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
                    let address = row.remote_ip.map(|ip| ip.to_string()).unwrap_or_default();
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

    for (pid, traffic) in by_pid.iter_mut() {
        if *pid == 0 {
            traffic.process_name = if elevated {
                "Sistema ou processo encerrado (sem PID)".to_string()
            } else {
                "Outro usuário (sem permissão para identificar)".to_string()
            };
            traffic.executable_path = None;
            continue;
        }
        let (name, exe) = resolve_pid(*pid);
        traffic.process_name = name;
        traffic.executable_path = exe;
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
            .then_with(|| {
                (right.listening_count + right.udp_socket_count)
                    .cmp(&(left.listening_count + left.udp_socket_count))
            })
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
        unavailable_reason: None,
        collected_at_unix_ms: now_unix_ms(),
    }
}

// ---------------------------------------------------------------------------
// Sinais de postura
// ---------------------------------------------------------------------------

pub fn collect_posture() -> SecurityPosture {
    let elevated = is_elevated();
    let signals = vec![
        collect_firewall_signal(),
        collect_proxy_signal(),
        collect_ca_roots_signal(),
        collect_kernel_security_signal(),
        collect_audit_visibility_signal(),
    ];

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

fn unavailable_signal(id: &'static str, title: &'static str, reason: String) -> PostureSignal {
    PostureSignal {
        id,
        title,
        severity: SignalSeverity::Unavailable,
        summary: format!("Não foi possível medir: {reason}"),
        rationale: "Sem medição não há como afirmar que este ponto está em ordem.",
        evidence: Vec::new(),
    }
}

fn run_command(program: &str, args: &[&str]) -> Result<String, String> {
    let mut child = crate::process::hidden_command(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("falha ao iniciar {program}: {error}"))?;

    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{program} excedeu o tempo limite"));
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(error) => return Err(format!("falha ao aguardar {program}: {error}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("falha ao ler saída de {program}: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UnitStatus {
    installed: bool,
    active: bool,
    active_state: String,
}

fn query_systemd_unit(unit: &str) -> Result<UnitStatus, String> {
    let stdout = run_command("systemctl", &["show", "-p", "LoadState,ActiveState", unit])?;
    parse_systemctl_show(&stdout)
        .ok_or_else(|| format!("resposta inesperada do systemctl para {unit}"))
}

fn parse_systemctl_show(output: &str) -> Option<UnitStatus> {
    let mut load_state: Option<&str> = None;
    let mut active_state: Option<&str> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("LoadState=") {
            load_state = Some(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("ActiveState=") {
            active_state = Some(value.trim());
        }
    }

    let load = load_state?;
    let active = active_state?;
    let installed = load != "not-found";
    Some(UnitStatus {
        installed,
        active: installed && active == "active",
        active_state: active.to_string(),
    })
}

// -- 1) Firewall do sistema --------------------------------------------------

fn parse_ufw_conf(content: &str) -> Option<bool> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            if key.trim().eq_ignore_ascii_case("ENABLED") {
                let normalized = value.trim().trim_matches('"').trim_matches('\'');
                if normalized.eq_ignore_ascii_case("yes")
                    || normalized.eq_ignore_ascii_case("true")
                    || normalized == "1"
                {
                    return Some(true);
                }
                if normalized.eq_ignore_ascii_case("no")
                    || normalized.eq_ignore_ascii_case("false")
                    || normalized == "0"
                {
                    return Some(false);
                }
            }
        }
    }
    None
}

fn collect_firewall_signal() -> PostureSignal {
    let ufw_conf = fs::read_to_string("/etc/ufw/ufw.conf")
        .ok()
        .and_then(|content| parse_ufw_conf(&content));
    let ufw_unit = query_systemd_unit("ufw").ok();
    let firewalld_unit = query_systemd_unit("firewalld").ok();
    let nftables_unit = query_systemd_unit("nftables").ok();

    evaluate_firewall(
        ufw_conf,
        ufw_unit.as_ref(),
        firewalld_unit.as_ref(),
        nftables_unit.as_ref(),
    )
}

fn evaluate_firewall(
    ufw_conf_enabled: Option<bool>,
    ufw_unit: Option<&UnitStatus>,
    firewalld_unit: Option<&UnitStatus>,
    nftables_unit: Option<&UnitStatus>,
) -> PostureSignal {
    let rationale = "Um firewall ativo bloqueia conexões de entrada não solicitadas na rede local.";
    let mut active_names: Vec<&str> = Vec::new();
    let mut installed_any = false;
    let mut evidence: Vec<String> = Vec::new();

    // UFW só está efetivamente ativo se ENABLED=yes em /etc/ufw/ufw.conf E serviço active.
    let ufw_present = ufw_conf_enabled.is_some() || ufw_unit.is_some_and(|u| u.installed);
    if ufw_present {
        installed_any = true;
        let service_active = ufw_unit.is_some_and(|u| u.active);
        match (ufw_conf_enabled, service_active) {
            (Some(true), true) => {
                active_names.push("ufw");
                evidence.push(format_evidence_line(
                    "ufw",
                    "ativo (ENABLED=yes em /etc/ufw/ufw.conf e serviço active)",
                ));
            }
            (Some(false), _) => {
                evidence.push(format_evidence_line(
                    "ufw",
                    "desativado em /etc/ufw/ufw.conf (ENABLED=no)",
                ));
            }
            (Some(true), false) => {
                let state = ufw_unit
                    .map(|u| u.active_state.as_str())
                    .unwrap_or("desconhecido");
                evidence.push(format_evidence_line(
                    "ufw",
                    &format!("ENABLED=yes, mas serviço {state}"),
                ));
            }
            (None, active) => {
                let state = if active { "active" } else { "inactive" };
                evidence.push(format_evidence_line(
                    "ufw",
                    &format!("serviço {state} (/etc/ufw/ufw.conf não legível)"),
                ));
            }
        }
    }

    for (name, unit) in [("firewalld", firewalld_unit), ("nftables", nftables_unit)] {
        if let Some(status) = unit.filter(|u| u.installed) {
            installed_any = true;
            if status.active {
                active_names.push(name);
                evidence.push(format_evidence_line(name, "serviço active"));
            } else {
                evidence.push(format_evidence_line(
                    name,
                    &format!("serviço {}", status.active_state),
                ));
            }
        }
    }

    if !active_names.is_empty() {
        PostureSignal {
            id: "firewall",
            title: "Firewall do sistema",
            severity: SignalSeverity::Ok,
            summary: format!("Firewall ativo: {}", active_names.join(", ")),
            rationale,
            evidence,
        }
    } else if installed_any {
        PostureSignal {
            id: "firewall",
            title: "Firewall do sistema",
            severity: SignalSeverity::Attention,
            summary: "Nenhum firewall ativo entre os instalados".to_string(),
            rationale,
            evidence,
        }
    } else {
        unavailable_signal(
            "firewall",
            "Firewall do sistema",
            "nenhum serviço ufw, firewalld ou nftables encontrado ou systemctl indisponível".into(),
        )
    }
}

// -- 2) Proxy do sistema -----------------------------------------------------

fn collect_proxy_signal() -> PostureSignal {
    let env_entries: Vec<(String, String)> = PROXY_ENV_VARS
        .iter()
        .filter_map(|var| {
            let value = std::env::var(var).ok()?;
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(((*var).to_string(), trimmed.to_string()))
            }
        })
        .collect();

    let gnome_evidence = collect_gnome_proxy();
    let kde_evidence = home_dir()
        .map(|home| home.join(".config/kioslaverc"))
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|content| parse_kioslaverc_proxy(&content))
        .unwrap_or_default();

    evaluate_proxy(env_entries, gnome_evidence, kde_evidence)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn collect_gnome_proxy() -> Vec<String> {
    let Ok(mode_raw) = run_command("gsettings", &["get", "org.gnome.system.proxy", "mode"]) else {
        return Vec::new();
    };
    let mode = mode_raw.trim().trim_matches('\'');
    match mode {
        "manual" => {
            let mut items = Vec::new();
            for proto in ["http", "https", "socks"] {
                let schema = format!("org.gnome.system.proxy.{proto}");
                let host = run_command("gsettings", &["get", &schema, "host"])
                    .ok()
                    .map(|s| s.trim().trim_matches('\'').to_string())
                    .unwrap_or_default();
                let port = run_command("gsettings", &["get", &schema, "port"])
                    .ok()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if !host.is_empty() {
                    items.push(format_evidence_line(
                        &format!("GNOME {proto}"),
                        &format!("{host}:{port}"),
                    ));
                }
            }
            if items.is_empty() {
                items.push(format_evidence_line("GNOME proxy", "modo manual"));
            }
            items
        }
        "auto" => {
            let url = run_command(
                "gsettings",
                &["get", "org.gnome.system.proxy", "autoconfig-url"],
            )
            .ok()
            .map(|s| s.trim().trim_matches('\'').to_string())
            .unwrap_or_default();
            vec![format_evidence_line(
                "GNOME proxy automático (PAC)",
                if url.is_empty() { "(sem URL)" } else { &url },
            )]
        }
        _ => Vec::new(),
    }
}

fn parse_kioslaverc_proxy(content: &str) -> Vec<String> {
    let mut in_section = false;
    let mut proxy_type: Option<i32> = None;
    let mut details: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == "[Proxy Settings]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "ProxyType" {
            proxy_type = value.parse::<i32>().ok();
        } else if !value.is_empty()
            && matches!(
                key,
                "httpProxy" | "httpsProxy" | "socksProxy" | "Proxy Config Script"
            )
        {
            details.push(format_evidence_line(&format!("KDE {key}"), value));
        }
    }

    match proxy_type {
        Some(1) => {
            if details.is_empty() {
                vec![format_evidence_line(
                    "KDE kioslaverc",
                    "ProxyType=1 (manual)",
                )]
            } else {
                details
            }
        }
        Some(2) => {
            if details.is_empty() {
                vec![format_evidence_line("KDE kioslaverc", "ProxyType=2 (PAC)")]
            } else {
                details
            }
        }
        Some(3) => vec![format_evidence_line("KDE kioslaverc", "ProxyType=3 (WPAD)")],
        Some(4) => vec![format_evidence_line(
            "KDE kioslaverc",
            "ProxyType=4 (variáveis de ambiente)",
        )],
        _ => Vec::new(),
    }
}

fn evaluate_proxy(
    env_entries: Vec<(String, String)>,
    gnome_evidence: Vec<String>,
    kde_evidence: Vec<String>,
) -> PostureSignal {
    let rationale = "Um proxy não declarado é o caminho mais curto para interceptar HTTPS.";
    let mut evidence: Vec<String> = env_entries
        .iter()
        .map(|(key, value)| format_evidence_line(key, value))
        .collect();
    evidence.extend(gnome_evidence);
    evidence.extend(kde_evidence);

    if evidence.is_empty() {
        PostureSignal {
            id: "system_proxy",
            title: "Proxy do sistema",
            severity: SignalSeverity::Ok,
            summary: "Sem proxy configurado no ambiente nem na sessão desktop".to_string(),
            rationale,
            evidence,
        }
    } else {
        PostureSignal {
            id: "system_proxy",
            title: "Proxy do sistema",
            severity: SignalSeverity::Attention,
            summary: format!("Proxy detectado ({} configuração(ões))", evidence.len()),
            rationale,
            evidence,
        }
    }
}

// -- 3) Certificados raiz adicionados ----------------------------------------

fn collect_ca_roots_signal() -> PostureSignal {
    let mut checked_dirs = Vec::new();
    let mut certs = Vec::new();

    for dir_str in CA_ANCHOR_DIRS {
        let dir = Path::new(dir_str);
        if !dir.is_dir() {
            continue;
        }
        if let Ok(found) = list_ca_files(dir, 0) {
            checked_dirs.push((*dir_str).to_string());
            certs.extend(found);
        }
    }

    certs.sort();
    evaluate_ca_roots(checked_dirs, certs)
}

fn list_ca_files(dir: &Path, depth: usize) -> std::io::Result<Vec<String>> {
    let mut results = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if depth < 2 {
                if let Ok(nested) = list_ca_files(&path, depth + 1) {
                    results.extend(nested);
                }
            }
            continue;
        }
        let is_cert = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("crt") || ext.eq_ignore_ascii_case("pem"));
        if is_cert {
            results.push(path.to_string_lossy().into_owned());
        }
    }
    Ok(results)
}

fn evaluate_ca_roots(checked_dirs: Vec<String>, certs: Vec<String>) -> PostureSignal {
    let rationale = "Uma raiz instalada permite assinar certificados aceitos sem aviso.";

    if checked_dirs.is_empty() {
        return unavailable_signal(
            "custom_ca_roots",
            "Certificados raiz adicionados",
            "nenhum diretório padrão de âncoras de CA locais foi encontrado".into(),
        );
    }

    if certs.is_empty() {
        PostureSignal {
            id: "custom_ca_roots",
            title: "Certificados raiz adicionados",
            severity: SignalSeverity::Ok,
            summary: "Nenhum certificado raiz extra nas pastas de confiança locais".to_string(),
            rationale,
            evidence: checked_dirs
                .into_iter()
                .map(|dir| format_evidence_line("Pasta verificada", &dir))
                .collect(),
        }
    } else {
        let total = certs.len();
        let mut evidence: Vec<String> = certs.into_iter().take(MAX_CERT_EVIDENCE).collect();
        if total > MAX_CERT_EVIDENCE {
            evidence.push(format!(
                "… e mais {} certificado(s) não listados",
                total - MAX_CERT_EVIDENCE
            ));
        }
        PostureSignal {
            id: "custom_ca_roots",
            title: "Certificados raiz adicionados",
            severity: SignalSeverity::Attention,
            summary: format!(
                "{total} certificado(s) raiz adicional(is) nas pastas de confiança locais"
            ),
            rationale,
            evidence,
        }
    }
}

// -- 4) Proteção do kernel (LSM / ptrace) ------------------------------------

fn collect_kernel_security_signal() -> PostureSignal {
    let apparmor = fs::read_to_string("/sys/module/apparmor/parameters/enabled").ok();
    let selinux = fs::read_to_string("/sys/fs/selinux/enforce").ok();
    let ptrace = fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok();

    evaluate_kernel_security(apparmor.as_deref(), selinux.as_deref(), ptrace.as_deref())
}

fn evaluate_kernel_security(
    apparmor_raw: Option<&str>,
    selinux_raw: Option<&str>,
    ptrace_raw: Option<&str>,
) -> PostureSignal {
    let rationale =
        "AppArmor/SELinux confinam processos e ptrace_scope impede que um programa leia a memória de outro.";

    let apparmor_enabled = apparmor_raw.map(|raw| raw.trim().eq_ignore_ascii_case("Y"));
    let selinux_enforce = selinux_raw.and_then(|raw| raw.trim().parse::<u8>().ok());
    let ptrace_scope = ptrace_raw.and_then(|raw| raw.trim().parse::<u8>().ok());

    if apparmor_enabled.is_none() && selinux_enforce.is_none() && ptrace_scope.is_none() {
        return unavailable_signal(
            "kernel_security",
            "Proteção do kernel (LSM / ptrace)",
            "não foi possível ler AppArmor, SELinux nem yama/ptrace_scope".into(),
        );
    }

    let mut evidence = Vec::new();
    let mut active_lsms = Vec::new();

    if let Some(enabled) = apparmor_enabled {
        if enabled {
            active_lsms.push("AppArmor");
            evidence.push(format_evidence_line("AppArmor", "ativo (Y)"));
        } else {
            evidence.push(format_evidence_line("AppArmor", "inativo (N)"));
        }
    }

    if let Some(enforce) = selinux_enforce {
        if enforce == 1 {
            active_lsms.push("SELinux");
            evidence.push(format_evidence_line("SELinux", "enforcing (1)"));
        } else {
            evidence.push(format_evidence_line("SELinux", "permissive (0)"));
        }
    }

    let ptrace_restricted = match ptrace_scope {
        Some(0) => {
            evidence.push(format_evidence_line(
                "yama/ptrace_scope",
                "0 (qualquer processo do usuário pode inspecionar outro)",
            ));
            Some(false)
        }
        Some(1) => {
            evidence.push(format_evidence_line(
                "yama/ptrace_scope",
                "1 (restrito a processos filhos)",
            ));
            Some(true)
        }
        Some(2) => {
            evidence.push(format_evidence_line(
                "yama/ptrace_scope",
                "2 (somente administrador)",
            ));
            Some(true)
        }
        Some(level) => {
            evidence.push(format_evidence_line(
                "yama/ptrace_scope",
                &format!("{level} (ptrace bloqueado)"),
            ));
            Some(true)
        }
        None => {
            evidence.push(format_evidence_line(
                "yama/ptrace_scope",
                "não exposto pelo kernel",
            ));
            None
        }
    };

    let lsm_active = !active_lsms.is_empty();
    let ptrace_ok = ptrace_restricted == Some(true);

    if lsm_active && ptrace_ok {
        PostureSignal {
            id: "kernel_security",
            title: "Proteção do kernel (LSM / ptrace)",
            severity: SignalSeverity::Ok,
            summary: format!("{} ativo e ptrace restrito", active_lsms.join(" + ")),
            rationale,
            evidence,
        }
    } else {
        let mut issues = Vec::new();
        if !lsm_active {
            issues.push("nenhum LSM (AppArmor/SELinux) ativo");
        }
        match ptrace_restricted {
            Some(false) => issues.push("ptrace_scope sem restrição (0)"),
            None => issues.push("yama/ptrace_scope indisponível"),
            Some(true) => {}
        }
        PostureSignal {
            id: "kernel_security",
            title: "Proteção do kernel (LSM / ptrace)",
            severity: SignalSeverity::Attention,
            summary: format!("Proteção parcial: {}", issues.join("; ")),
            rationale,
            evidence,
        }
    }
}

// -- 5) Visibilidade de auditoria --------------------------------------------

fn dir_has_entries(path: &Path) -> bool {
    fs::read_dir(path)
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some()
}

fn collect_audit_visibility_signal() -> PostureSignal {
    let persistent_journal = dir_has_entries(Path::new("/var/log/journal"));
    let volatile_journal = Path::new("/run/log/journal").is_dir();
    let auditd_unit = query_systemd_unit("auditd").ok();

    evaluate_audit_visibility(persistent_journal, volatile_journal, auditd_unit.as_ref())
}

fn evaluate_audit_visibility(
    persistent_journal: bool,
    volatile_journal: bool,
    auditd_unit: Option<&UnitStatus>,
) -> PostureSignal {
    let rationale =
        "Sem registro persistente de eventos, ações anteriores ao último boot somem do histórico.";

    if !persistent_journal && !volatile_journal && auditd_unit.is_none() {
        return unavailable_signal(
            "audit_visibility",
            "Visibilidade de auditoria",
            "não foi possível inspecionar systemd-journald nem auditd".into(),
        );
    }

    let mut evidence = Vec::new();
    if persistent_journal {
        evidence.push(format_evidence_line(
            "systemd-journald",
            "persistente em disco (/var/log/journal)",
        ));
    } else if volatile_journal {
        evidence.push(format_evidence_line(
            "systemd-journald",
            "apenas volátil (/run/log/journal — apagado ao reiniciar)",
        ));
    } else {
        evidence.push(format_evidence_line(
            "systemd-journald",
            "nenhum diretório de journal encontrado",
        ));
    }

    let auditd_active = match auditd_unit {
        Some(status) if status.active => {
            evidence.push(format_evidence_line("auditd", "serviço active"));
            true
        }
        Some(status) if status.installed => {
            evidence.push(format_evidence_line(
                "auditd",
                &format!("instalado, mas {}", status.active_state),
            ));
            false
        }
        Some(_) => {
            evidence.push(format_evidence_line("auditd", "não instalado"));
            false
        }
        None => {
            evidence.push(format_evidence_line("auditd", "systemctl indisponível"));
            false
        }
    };

    if persistent_journal && auditd_active {
        PostureSignal {
            id: "audit_visibility",
            title: "Visibilidade de auditoria",
            severity: SignalSeverity::Ok,
            summary: "Journal persistente em disco e auditd ativo".to_string(),
            rationale,
            evidence,
        }
    } else if persistent_journal {
        PostureSignal {
            id: "audit_visibility",
            title: "Visibilidade de auditoria",
            severity: SignalSeverity::Ok,
            summary: "Journal persistente em disco (auditd inativo)".to_string(),
            rationale,
            evidence,
        }
    } else if auditd_active {
        PostureSignal {
            id: "audit_visibility",
            title: "Visibilidade de auditoria",
            severity: SignalSeverity::Attention,
            summary: "auditd ativo, mas o journal do systemd é volátil".to_string(),
            rationale,
            evidence,
        }
    } else {
        PostureSignal {
            id: "audit_visibility",
            title: "Visibilidade de auditoria",
            severity: SignalSeverity::Attention,
            summary: "Journal apenas volátil (ou ausente) e auditd inativo".to_string(),
            rationale,
            evidence,
        }
    }
}

// ---------------------------------------------------------------------------
// Testes unitários e de coleta real no Linux
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parseia_ipv4_e_ipv6_little_endian_de_proc_net() {
        assert_eq!(
            parse_proc_ipv4("0100007F"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(parse_proc_ipv4("00000000"), Some(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(
            parse_proc_ipv4("0A7100CB"),
            Some(Ipv4Addr::new(203, 0, 113, 10))
        );
        assert_eq!(parse_proc_ipv4("1234"), None);

        assert_eq!(
            parse_proc_ipv6("00000000000000000000000000000000"),
            Some(Ipv6Addr::UNSPECIFIED)
        );
        assert_eq!(
            parse_proc_ipv6("00000000000000000000000001000000"),
            Some(Ipv6Addr::LOCALHOST)
        );
        // ::ffff:127.0.0.1
        let mapped_lo = parse_proc_ipv6("0000000000000000FFFF00000100007F").unwrap();
        assert!(is_loopback_ip(IpAddr::V6(mapped_lo)));
        // 2001:db8::1
        let doc_v6 = parse_proc_ipv6("B80D0120000000000000000001000000").unwrap();
        assert_eq!(doc_v6.to_string(), "2001:db8::1");
        assert!(!is_loopback_ip(IpAddr::V6(doc_v6)));
    }

    #[test]
    fn parseia_tabelas_tcp_tcp6_e_udp() {
        let tcp_fixture = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1001 1 0 100 0 0 10 0
   1: 0100007F:9C40 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 1002 1 0 100 0 0 10 0
   2: 9B02A8C0:C350 0A7100CB:01BB 01 00000000:00000000 00:00000000 00000000  1000        0 1003 1 0 100 0 0 10 0
   3: 9B02A8C0:C351 0A7100CB:01BB 06 00000000:00000000 00:00000000 00000000  1000        0 1004 1 0 100 0 0 10 0
";
        let mut sockets = Vec::new();
        parse_proc_net_table(tcp_fixture, TableProto::Tcp4, &mut sockets);
        assert_eq!(sockets.len(), 3); // linha 3 (TIME_WAIT = 06) ignorada
        assert_eq!(sockets[0].kind, SocketKind::Listening);
        assert!(sockets[0].listens_on_all_interfaces());
        assert_eq!(sockets[0].inode, 1001);

        assert_eq!(sockets[1].kind, SocketKind::Established);
        assert!(sockets[1].remote_is_loopback());
        assert_eq!(sockets[1].remote_port, 8080);

        assert_eq!(sockets[2].kind, SocketKind::Established);
        assert!(!sockets[2].remote_is_loopback());
        assert_eq!(sockets[2].remote_port, 443);

        let tcp6_fixture = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:01BB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2001 1 0 100 0 0 10 0
   1: 00000000000000000000000001000000:9999 0000000000000000FFFF00000100007F:0050 01 00000000:00000000 00:00000000 00000000  1000        0 2002 1 0 100 0 0 10 0
";
        let mut sockets6 = Vec::new();
        parse_proc_net_table(tcp6_fixture, TableProto::Tcp6, &mut sockets6);
        assert_eq!(sockets6.len(), 2);
        assert!(sockets6[0].listens_on_all_interfaces());
        assert!(sockets6[1].remote_is_loopback());

        let udp_fixture = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  10: 3500007F:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000   991        0 3001 2 0 0
  11: 00000000:0000 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 3002 2 0 0
";
        let mut udp_sockets = Vec::new();
        parse_proc_net_table(udp_fixture, TableProto::Udp4, &mut udp_sockets);
        assert_eq!(udp_sockets.len(), 1); // porta 0 ignorada
        assert_eq!(udp_sockets[0].kind, SocketKind::Udp);
        assert_eq!(udp_sockets[0].inode, 3001);
    }

    #[test]
    fn agrega_com_teto_de_destinos_dedupe_e_outro_usuario_honesto() {
        let mut sockets = Vec::new();
        // 30 destinos distintos + 1 duplicado para o PID 42
        for idx in 1..=30u8 {
            sockets.push(RawSocket {
                kind: SocketKind::Established,
                local_ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
                remote_ip: Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, idx))),
                remote_port: 443,
                is_ipv6: false,
                inode: 500,
            });
        }
        // Duplicado do primeiro destino
        sockets.push(RawSocket {
            kind: SocketKind::Established,
            local_ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            remote_ip: Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))),
            remote_port: 443,
            is_ipv6: false,
            inode: 500,
        });
        // Loopback do PID 42
        sockets.push(RawSocket {
            kind: SocketKind::Established,
            local_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            remote_ip: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            remote_port: 5432,
            is_ipv6: false,
            inode: 500,
        });
        // Socket em escuta pública de outro usuário (inode ausente do mapa)
        sockets.push(RawSocket {
            kind: SocketKind::Listening,
            local_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            remote_ip: None,
            remote_port: 0,
            is_ipv6: false,
            inode: 9999,
        });

        let mut map = HashMap::new();
        map.insert(500u64, 42u32);

        let snap = aggregate_sockets(
            &sockets,
            &map,
            |pid| (format!("app-{pid}"), Some(format!("/usr/bin/app-{pid}"))),
            false,
        );

        assert_eq!(snap.established_total, 32);
        assert_eq!(snap.established_external, 31);
        assert_eq!(snap.established_loopback, 1);
        assert_eq!(snap.listening_total, 1);
        assert_eq!(snap.listening_public_total, 1);
        assert_eq!(snap.processes.len(), 2);

        let app = &snap.processes[0];
        assert_eq!(app.pid, 42);
        assert_eq!(app.external_total, 31);
        assert_eq!(app.destinations.len(), MAX_DESTINATIONS_PER_PROCESS);

        let other = &snap.processes[1];
        assert_eq!(other.pid, 0);
        assert!(other.executable_path.is_none());
        assert!(other.process_name.contains("Outro usuário"));
        assert_eq!(other.listening_count, 1);
        assert!(other.listens_on_all_interfaces);
    }

    #[test]
    fn parseia_socket_inode_e_fixtures_de_postura() {
        assert_eq!(parse_socket_inode("socket:[123456]"), Some(123456));
        assert_eq!(parse_socket_inode("pipe:[123456]"), None);
        assert_eq!(parse_socket_inode("/dev/null"), None);

        assert_eq!(
            parse_ufw_conf("# comment\nENABLED=yes\nLOGLEVEL=low\n"),
            Some(true)
        );
        assert_eq!(parse_ufw_conf("ENABLED=no\n"), Some(false));
        assert_eq!(parse_ufw_conf("# nada"), None);

        let unit_active = parse_systemctl_show("LoadState=loaded\nActiveState=active\n").unwrap();
        assert!(unit_active.installed && unit_active.active);
        let unit_missing =
            parse_systemctl_show("LoadState=not-found\nActiveState=inactive\n").unwrap();
        assert!(!unit_missing.installed && !unit_missing.active);

        // UFW com serviço active mas ENABLED=no deve ser Attention (não falso Ok!)
        let fw_off = evaluate_firewall(Some(false), Some(&unit_active), None, None);
        assert_eq!(fw_off.severity, SignalSeverity::Attention);

        // UFW ENABLED=yes + serviço active -> Ok
        let fw_on = evaluate_firewall(Some(true), Some(&unit_active), None, None);
        assert_eq!(fw_on.severity, SignalSeverity::Ok);

        // Nenhum firewall instalado -> Unavailable
        let fw_none = evaluate_firewall(None, Some(&unit_missing), Some(&unit_missing), None);
        assert_eq!(fw_none.severity, SignalSeverity::Unavailable);

        // KDE kioslaverc
        let kde_manual = "\
[Proxy Settings]
ProxyType=1
httpProxy=http://127.0.0.1:8080
";
        let kde_ev = parse_kioslaverc_proxy(kde_manual);
        assert_eq!(kde_ev.len(), 1);
        assert!(kde_ev[0].contains("127.0.0.1:8080"));
        assert!(parse_kioslaverc_proxy("[Proxy Settings]\nProxyType=0\n").is_empty());

        // Proxy
        assert_eq!(
            evaluate_proxy(vec![], vec![], vec![]).severity,
            SignalSeverity::Ok
        );
        assert_eq!(
            evaluate_proxy(
                vec![("HTTP_PROXY".into(), "http://proxy:3128".into())],
                vec![],
                vec![]
            )
            .severity,
            SignalSeverity::Attention
        );

        // CA roots
        assert_eq!(
            evaluate_ca_roots(vec![], vec![]).severity,
            SignalSeverity::Unavailable
        );
        assert_eq!(
            evaluate_ca_roots(vec!["/usr/local/share/ca-certificates".into()], vec![]).severity,
            SignalSeverity::Ok
        );
        assert_eq!(
            evaluate_ca_roots(
                vec!["/usr/local/share/ca-certificates".into()],
                vec!["/usr/local/share/ca-certificates/corp.crt".into()]
            )
            .severity,
            SignalSeverity::Attention
        );

        // Kernel security
        assert_eq!(
            evaluate_kernel_security(Some("Y\n"), None, Some("1\n")).severity,
            SignalSeverity::Ok
        );
        assert_eq!(
            evaluate_kernel_security(Some("N\n"), None, Some("1\n")).severity,
            SignalSeverity::Attention
        );
        assert_eq!(
            evaluate_kernel_security(Some("Y\n"), None, Some("0\n")).severity,
            SignalSeverity::Attention
        );
        assert_eq!(
            evaluate_kernel_security(None, None, None).severity,
            SignalSeverity::Unavailable
        );

        // Audit visibility
        assert_eq!(
            evaluate_audit_visibility(true, false, Some(&unit_missing)).severity,
            SignalSeverity::Ok
        );
        assert_eq!(
            evaluate_audit_visibility(false, true, Some(&unit_missing)).severity,
            SignalSeverity::Attention
        );
        assert_eq!(
            evaluate_audit_visibility(false, false, None).severity,
            SignalSeverity::Unavailable
        );
    }

    #[test]
    fn coleta_real_de_rede_e_postura_no_linux_funciona() {
        let net = collect_network();
        assert!(
            net.unavailable_reason.is_none(),
            "esperava coleta de /proc/net sem falha: {:?}",
            net.unavailable_reason
        );
        assert!(net.collected_at_unix_ms > 0);
        // Consistência entre totais e soma dos processos
        let sum_est: usize = net.processes.iter().map(|p| p.established_count).sum();
        let sum_ext: usize = net.processes.iter().map(|p| p.external_total).sum();
        let sum_udp: usize = net.processes.iter().map(|p| p.udp_socket_count).sum();
        let sum_listen: usize = net.processes.iter().map(|p| p.listening_count).sum();
        assert_eq!(sum_est, net.established_total);
        assert_eq!(sum_ext, net.established_external);
        assert_eq!(
            net.established_external + net.established_loopback,
            net.established_total
        );
        assert_eq!(sum_udp, net.udp_total);
        assert_eq!(sum_listen, net.listening_total);

        let posture = collect_posture();
        assert_eq!(posture.signals.len(), 5);
        assert_eq!(
            posture.attention_count,
            posture
                .signals
                .iter()
                .filter(|s| s.severity == SignalSeverity::Attention)
                .count()
        );
        assert_eq!(
            posture.unavailable_count,
            posture
                .signals
                .iter()
                .filter(|s| s.severity == SignalSeverity::Unavailable)
                .count()
        );
        assert!(posture.collected_at_unix_ms > 0);
    }

    #[test]
    fn limpa_sufixo_deleted_de_exe() {
        assert_eq!(
            clean_exe_link("/opt/google/chrome/chrome (deleted)").as_deref(),
            Some("/opt/google/chrome/chrome")
        );
        assert_eq!(
            clean_exe_link("/usr/bin/firefox").as_deref(),
            Some("/usr/bin/firefox")
        );
        assert_eq!(clean_exe_link("   "), None);
    }
}
