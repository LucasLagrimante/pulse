//! Partes comuns a Linux e macOS do inspetor de portas: filtro de ruído,
//! montagem das entradas e encerramento por sinal POSIX.

use super::{
    format_listen_url, infer_project_info, infer_runtime, is_dev_build_binary,
    is_dev_runtime_binary, DevPortEntry,
};
use std::path::Path;
use std::time::Duration;

/// Portas de serviços do sistema que nunca são servidores de desenvolvimento.
const UNIX_SYSTEM_PORTS: &[u16] = &[
    22, 25, 53, 67, 68, 111, 123, 137, 138, 139, 323, 445, 548, 631, 1716, 5353,
];

/// Daemons do sistema e apps desktop que abrem portas locais para IPC.
const UNIX_NOISE_PROCESSES: &[&str] = &[
    "systemd-resolve",
    "systemd-resolved",
    "systemd-journal",
    "systemd",
    "dbus-daemon",
    "rpcbind",
    "dhclient",
    "cupsd",
    "cups-browsed",
    "sshd",
    "avahi-daemon",
    "chronyd",
    "dnsmasq",
    "exim4",
    "kdeconnectd",
    "code",
    "code-insiders",
    "codium",
    "cursor",
    "steam",
    "steamwebhelper",
    "spotify",
    "discord",
    "slack",
    "chrome",
    "chromium",
    "firefox",
    "brave",
    "msedge",
    "teams",
    "zoom",
    "ciscocollabhost",
    "webex",
    "pulse",
    "pulse-windows",
    // macOS (lsof corta o nome em 9 caracteres)
    "controlce",
    "controlcenter",
    "rapportd",
    "sharingd",
    "launchd",
];

/// Dados do processo dono do socket (todos opcionais: nunca inventados).
#[derive(Debug, Clone, Default)]
pub(super) struct ProcessInfo {
    pub comm: String,
    pub exe: Option<String>,
    pub cwd: Option<String>,
    pub cmdline: Option<String>,
}

pub(super) fn is_unix_noise_process(
    comm: &str,
    command_line: Option<&str>,
    exe: Option<&str>,
    port: u16,
    ephemeral_low: u16,
) -> bool {
    if UNIX_SYSTEM_PORTS.contains(&port) {
        return true;
    }
    let lower = comm.trim().to_ascii_lowercase();
    if lower.is_empty() || UNIX_NOISE_PROCESSES.contains(&lower.as_str()) {
        return true;
    }
    if port >= ephemeral_low
        && !is_dev_runtime_binary(&lower)
        && !is_dev_build_binary(exe)
        && !is_dev_build_binary(command_line)
    {
        // Portas da faixa efêmera costumam ser IPC de apps; só ficam se a
        // linha de comando indicar um servidor de desenvolvimento.
        let cmd_lower = command_line.unwrap_or("").to_ascii_lowercase();
        let looks_like_server = [
            "serve", "dev", "listen", "uvicorn", "vite", "next", "wrangler",
        ]
        .iter()
        .any(|token| cmd_lower.contains(token));
        if !looks_like_server {
            return true;
        }
    }
    false
}

pub(super) fn build_entry(
    environment: &str,
    pid: u32,
    port: u16,
    bind_address: &str,
    info: &ProcessInfo,
) -> DevPortEntry {
    let process_name = if info.comm.trim().is_empty() {
        "processo".to_owned()
    } else {
        info.comm.trim().to_owned()
    };
    let runtime = infer_runtime(&process_name, info.cmdline.as_deref(), info.exe.as_deref());
    let (project_name, project_path) = infer_project_info(
        &process_name,
        info.exe.as_deref(),
        info.cwd.as_deref(),
        info.cmdline.as_deref(),
        &runtime,
    );
    let icon = info
        .exe
        .as_deref()
        .and_then(|path| crate::app_icons::icon_for_path(Path::new(path)));
    DevPortEntry {
        id: format!("{environment}:{pid}:{port}"),
        port,
        bind_address: bind_address.to_owned(),
        url: format_listen_url(bind_address, port),
        pid,
        process_name,
        runtime,
        project_name,
        project_path,
        command_line: info.cmdline.clone(),
        environment: environment.to_owned(),
        wsl_distro: None,
        icon,
    }
}

/// Porta em escuta cujo dono não pode ser lido (processo de outro usuário,
/// tipicamente root/docker-proxy). PID 0 = desconhecido: o encerramento é
/// recusado e nada além da porta é afirmado.
pub(super) fn unresolved_entry(
    environment: &str,
    key: &str,
    port: u16,
    bind_address: &str,
) -> DevPortEntry {
    DevPortEntry {
        id: format!("{environment}:{key}:{port}"),
        port,
        bind_address: bind_address.to_owned(),
        url: format_listen_url(bind_address, port),
        pid: 0,
        process_name: "Indisponível".to_owned(),
        runtime: "Processo de outro usuário".to_owned(),
        project_name: format!("Porta {port}"),
        project_path: None,
        command_line: None,
        environment: environment.to_owned(),
        wsl_distro: None,
        icon: None,
    }
}

fn send_signal(pid: u32, signal: libc::c_int) -> Result<(), std::io::Error> {
    let pid = libc::pid_t::try_from(pid)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if pid <= 1 {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    // SAFETY: kill(2) só recebe inteiros; PID validado acima (nunca 0, -1 ou 1).
    if unsafe { libc::kill(pid, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn describe_kill_error(error: &std::io::Error, process_name: &str, pid: u32) -> String {
    match error.raw_os_error() {
        Some(libc::EPERM) => format!(
            "Sem permissão para encerrar {process_name} (PID {pid}): o processo pertence a outro usuário."
        ),
        _ => format!("Não foi possível encerrar {process_name} (PID {pid}): {error}"),
    }
}

/// SIGTERM, espera curta e SIGKILL se o processo ainda escuta a porta
/// (mesma sequência do `taskkill` sem/com `/F` no Windows).
pub(super) fn terminate_listener(
    port: u16,
    pid: u32,
    environment: &str,
    process_name: &str,
) -> Result<(), String> {
    match send_signal(pid, libc::SIGTERM) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => return Ok(()),
        Err(error) => return Err(describe_kill_error(&error, process_name, pid)),
    }
    std::thread::sleep(Duration::from_millis(250));
    let still_alive = super::collect_dev_ports_sync(true)?
        .ports
        .iter()
        .any(|entry| entry.port == port && entry.pid == pid && entry.environment == environment);
    if still_alive {
        match send_signal(pid, libc::SIGKILL) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
            Err(error) => return Err(describe_kill_error(&error, process_name, pid)),
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_entry, is_unix_noise_process, unresolved_entry, ProcessInfo};

    #[test]
    fn filters_system_daemons_and_desktop_ipc_ports() {
        assert!(is_unix_noise_process("sshd", None, None, 22, 32768));
        assert!(is_unix_noise_process("vite", None, None, 53, 32768));
        assert!(is_unix_noise_process("code", None, None, 3000, 32768));
        assert!(is_unix_noise_process(
            "electron",
            Some("/opt/app --type=renderer"),
            None,
            45547,
            32768
        ));
        assert!(!is_unix_noise_process(
            "node",
            Some("node server.js"),
            None,
            45547,
            32768
        ));
        assert!(!is_unix_noise_process(
            "myapp",
            None,
            Some("/home/u/p/target/debug/myapp"),
            40000,
            32768
        ));
        assert!(!is_unix_noise_process(
            "python3",
            Some("python3 -m http.server"),
            None,
            8000,
            32768
        ));
    }

    #[test]
    fn builds_entries_with_project_info_and_honest_unknown_owner() {
        let info = ProcessInfo {
            comm: "node".into(),
            exe: Some("/usr/bin/node".into()),
            cwd: Some("/home/u/projetos/loja".into()),
            cmdline: Some("node node_modules/.bin/vite".into()),
        };
        let entry = build_entry("linux", 42, 5173, "127.0.0.1", &info);
        assert_eq!(entry.id, "linux:42:5173");
        assert_eq!(entry.runtime, "Vite");
        assert_eq!(entry.project_name, "loja");
        assert_eq!(entry.url, "http://localhost:5173");

        let unknown = unresolved_entry("linux", "inode9", 80, "0.0.0.0");
        assert_eq!(unknown.pid, 0);
        assert_eq!(unknown.process_name, "Indisponível");
        assert!(unknown.command_line.is_none() && unknown.project_path.is_none());
    }
}
