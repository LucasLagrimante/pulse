//! Coleta de portas em escuta no Linux lendo `/proc/net/tcp{,6}` e mapeando o
//! inode de cada socket para o processo dono via `/proc/<pid>/fd`.
//!
//! Sem root só os processos do próprio usuário são legíveis: portas de outros
//! usuários (root, docker-proxy, postgres do sistema…) aparecem com PID 0 e
//! dono "Indisponível", nunca com um processo deduzido.

use super::unix::{build_entry, is_unix_noise_process, unresolved_entry, ProcessInfo};
use super::DevPortEntry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

pub(super) const ENVIRONMENT: &str = "linux";
/// Teto da varredura de `/proc/*/fd` (máquinas com navegadores abertos têm
/// dezenas de milhares de descritores).
const FD_SCAN_BUDGET: Duration = Duration::from_millis(1500);
const MAX_CMDLINE_CHARS: usize = 400;
const TCP_LISTEN_STATE: &str = "0A";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProcSocket {
    pub inode: u64,
    pub uid: u32,
    pub port: u16,
    pub bind_address: String,
}

fn decode_ipv4(hex: &str) -> Option<String> {
    let value = u32::from_str_radix(hex, 16).ok()?;
    // O kernel imprime o endereço (ordem de rede) como inteiro nativo.
    Some(Ipv4Addr::from(value.to_ne_bytes()).to_string())
}

fn decode_ipv6(hex: &str) -> Option<String> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (chunk, word) in bytes.chunks_mut(4).zip(0..4) {
        let part = u32::from_str_radix(&hex[word * 8..word * 8 + 8], 16).ok()?;
        chunk.copy_from_slice(&part.to_ne_bytes());
    }
    let address = Ipv6Addr::from(bytes);
    Some(match address.to_ipv4_mapped() {
        Some(v4) => v4.to_string(),
        None => address.to_string(),
    })
}

/// Linhas em `LISTEN` de `/proc/net/tcp` (ou `tcp6` com `ipv6 = true`).
pub(super) fn parse_proc_net_tcp(text: &str, ipv6: bool) -> Vec<ProcSocket> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10 || fields[3] != TCP_LISTEN_STATE {
                return None;
            }
            let (address_hex, port_hex) = fields[1].split_once(':')?;
            let port = u16::from_str_radix(port_hex, 16).ok().filter(|p| *p != 0)?;
            let bind_address = if ipv6 {
                decode_ipv6(address_hex)?
            } else {
                decode_ipv4(address_hex)?
            };
            Some(ProcSocket {
                inode: fields[9].parse().ok().filter(|inode| *inode != 0)?,
                uid: fields[7].parse().ok()?,
                port,
                bind_address,
            })
        })
        .collect()
}

/// Início da faixa de portas efêmeras (`ip_local_port_range`, padrão 32768).
pub(super) fn parse_port_range_low(text: &str) -> Option<u16> {
    text.split_whitespace().next()?.parse().ok()
}

fn ephemeral_low() -> u16 {
    fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .as_deref()
        .and_then(parse_port_range_low)
        .unwrap_or(32768)
}

pub(super) fn parse_socket_link(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// inode → menor PID que tem o socket aberto (o pai, em servidores com workers).
fn socket_owners(wanted: &HashSet<u64>) -> HashMap<u64, u32> {
    let mut owners: HashMap<u64, u32> = HashMap::new();
    if wanted.is_empty() {
        return owners;
    }
    let started = Instant::now();
    let Ok(entries) = fs::read_dir("/proc") else {
        return owners;
    };
    let mut pids: Vec<u32> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .collect();
    pids.sort_unstable();
    for (scanned, pid) in pids.into_iter().enumerate() {
        if started.elapsed() > FD_SCAN_BUDGET || owners.len() == wanted.len() {
            break;
        }
        if scanned % 15 == 0 {
            std::thread::yield_now();
        }
        // Processos de outros usuários: sem permissão, ignorados.
        let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.filter_map(Result::ok) {
            let Ok(link) = fs::read_link(fd.path()) else {
                continue;
            };
            if let Some(inode) = link.to_str().and_then(parse_socket_link) {
                if wanted.contains(&inode) {
                    owners.entry(inode).or_insert(pid);
                }
            }
        }
    }
    owners
}

fn read_link_string(path: String) -> Option<String> {
    let target = fs::read_link(path).ok()?;
    let text = target.to_string_lossy().into_owned();
    let text = text.trim_end_matches(" (deleted)").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

pub(super) fn cmdline_from_raw(raw: &[u8]) -> Option<String> {
    let text: String = String::from_utf8_lossy(raw)
        .split('\0')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_CMDLINE_CHARS)
        .collect();
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn process_info(pid: u32) -> ProcessInfo {
    ProcessInfo {
        comm: fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim().to_owned())
            .unwrap_or_default(),
        exe: read_link_string(format!("/proc/{pid}/exe")),
        cwd: read_link_string(format!("/proc/{pid}/cwd")),
        cmdline: fs::read(format!("/proc/{pid}/cmdline"))
            .ok()
            .and_then(|raw| cmdline_from_raw(&raw)),
    }
}

pub(super) fn collect() -> Result<Vec<DevPortEntry>, crate::debug::Failure> {
    let tcp4 = fs::read_to_string("/proc/net/tcp").map_err(|error| {
        crate::debug::Failure::new("Não foi possível ler a tabela TCP do Linux")
            .stage("/proc/net/tcp")
            .detail(error.to_string())
    })?;
    let mut sockets = parse_proc_net_tcp(&tcp4, false);
    // IPv6 pode estar desativado no kernel: ausência do arquivo não é erro.
    if let Ok(tcp6) = fs::read_to_string("/proc/net/tcp6") {
        sockets.extend(parse_proc_net_tcp(&tcp6, true));
    }

    let low = ephemeral_low();
    let own_pid = std::process::id();
    let wanted: HashSet<u64> = sockets.iter().map(|socket| socket.inode).collect();
    let owners = socket_owners(&wanted);
    let mut infos: HashMap<u32, ProcessInfo> = HashMap::new();
    let mut entries = Vec::new();

    for socket in sockets {
        match owners.get(&socket.inode).copied() {
            Some(pid) if pid == own_pid => {}
            Some(pid) => {
                let info = infos.entry(pid).or_insert_with(|| process_info(pid));
                if is_unix_noise_process(
                    &info.comm,
                    info.cmdline.as_deref(),
                    info.exe.as_deref(),
                    socket.port,
                    low,
                ) {
                    continue;
                }
                entries.push(build_entry(
                    ENVIRONMENT,
                    pid,
                    socket.port,
                    &socket.bind_address,
                    info,
                ));
            }
            None => {
                // Sem dono legível: só portas fora de serviços do sistema e
                // fora da faixa efêmera (IPC) entram, com o dono Indisponível.
                if is_unix_noise_process("?", None, None, socket.port, low) {
                    continue;
                }
                entries.push(unresolved_entry(
                    ENVIRONMENT,
                    &format!("uid{}", socket.uid),
                    socket.port,
                    &socket.bind_address,
                ));
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP4: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 249347 1 0000000000000000 100 0 0 10 0\n   1: 0100007F:30E6 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 20946268 1 0000000000000000 100 0 0 10 0\n   2: 0100007F:A1B2 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 555 1 0000000000000000 100 0 0 10 0\n";

    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000000000000000000000000000:1435 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 777 1 0000000000000000 100 0 0 10 0\n   1: 00000000000000000000000001000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 778 1 0000000000000000 100 0 0 10 0\n   2: 0000000000000000FFFF00000100007F:1F40 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 779 1 0000000000000000 100 0 0 10 0\n";

    #[test]
    fn parses_ipv4_listeners_only() {
        let sockets = parse_proc_net_tcp(TCP4, false);
        assert_eq!(sockets.len(), 2, "conexão estabelecida (01) não entra");
        assert_eq!(
            sockets[0],
            ProcSocket {
                inode: 249347,
                uid: 0,
                port: 80,
                bind_address: "0.0.0.0".into()
            }
        );
        assert_eq!(sockets[1].bind_address, "127.0.0.1");
        assert_eq!(sockets[1].port, 12518);
        assert_eq!(sockets[1].uid, 1000);
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn parses_ipv6_any_loopback_and_mapped_v4() {
        let sockets = parse_proc_net_tcp(TCP6, true);
        assert_eq!(sockets.len(), 3);
        assert_eq!(
            (sockets[0].bind_address.as_str(), sockets[0].port),
            ("::", 5173)
        );
        assert_eq!(
            (sockets[1].bind_address.as_str(), sockets[1].port),
            ("::1", 3000)
        );
        assert_eq!(
            (sockets[2].bind_address.as_str(), sockets[2].port),
            ("127.0.0.1", 8000)
        );
    }

    #[test]
    fn parses_helpers() {
        assert_eq!(parse_port_range_low("32768\t60999\n"), Some(32768));
        assert_eq!(parse_socket_link("socket:[20946268]"), Some(20946268));
        assert_eq!(parse_socket_link("pipe:[1]"), None);
        assert_eq!(
            cmdline_from_raw(b"node\0server.js\0--port\x003000\0"),
            Some("node server.js --port 3000".into())
        );
        assert_eq!(cmdline_from_raw(b""), None);
    }

    /// Coleta real: abre uma porta neste processo de teste e confirma que a
    /// varredura de /proc encontra o socket e o PID certo (sem filtrar, pois o
    /// binário de teste não é runtime de dev).
    #[test]
    fn live_proc_scan_finds_own_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local");
        let port = listener.local_addr().unwrap().port();
        let tcp4 = fs::read_to_string("/proc/net/tcp").expect("/proc/net/tcp legível");
        let sockets = parse_proc_net_tcp(&tcp4, false);
        let mine = sockets
            .iter()
            .find(|socket| socket.port == port)
            .expect("socket do teste listado");
        let owners = socket_owners(&HashSet::from([mine.inode]));
        assert_eq!(owners.get(&mine.inode), Some(&std::process::id()));
        let entries = collect().expect("coleta real");
        for entry in &entries {
            eprintln!(
                "porta {} {} pid {} {} ({}) projeto {}",
                entry.port,
                entry.bind_address,
                entry.pid,
                entry.process_name,
                entry.runtime,
                entry.project_name
            );
        }
        assert!(entries.iter().all(|e| e.port != 0 && !e.url.is_empty()));
        assert!(entries.iter().all(|e| e.pid != std::process::id()));
        assert!(entries
            .iter()
            .filter(|e| e.pid == 0)
            .all(|e| e.process_name == "Indisponível" && e.command_line.is_none()));
    }
}
