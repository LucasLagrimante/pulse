//! Coleta de portas em escuta no macOS via `lsof -nP -iTCP -sTCP:LISTEN -F pcn`.
//! TODO(macos): validar em hardware (formato do lsof e permissões do sandbox).
//!
//! O parser é puro e testado em qualquer plataforma; a coleta só compila no macOS.

/// Um socket em escuta lido do `lsof -F pcn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LsofListener {
    pub pid: u32,
    pub command: String,
    pub bind_address: String,
    pub port: u16,
}

/// Formato de campos do lsof: `p<pid>` abre um processo, `c<comando>` dá o
/// nome e cada `n<endereço>:<porta>` é um socket (`f<fd>` é ignorado).
pub(super) fn parse_lsof_listen(output: &str) -> Vec<LsofListener> {
    let mut listeners = Vec::new();
    let mut pid: Option<u32> = None;
    let mut command = String::new();
    for line in output.lines() {
        let Some(tag) = line.chars().next() else {
            continue;
        };
        let value = &line[tag.len_utf8()..];
        match tag {
            'p' => {
                pid = value.trim().parse().ok();
                command.clear();
            }
            'c' => command = value.trim().to_owned(),
            'n' => {
                let Some(pid) = pid else {
                    continue;
                };
                // Ex.: `*:3000`, `127.0.0.1:5173`, `[::1]:8080`.
                let Some((host, port)) = value.trim().rsplit_once(':') else {
                    continue;
                };
                let Ok(port) = port.parse::<u16>() else {
                    continue;
                };
                if port == 0 {
                    continue;
                }
                let host = host.trim_start_matches('[').trim_end_matches(']');
                let bind_address = if host.is_empty() || host == "*" {
                    "0.0.0.0".to_owned()
                } else {
                    host.to_owned()
                };
                listeners.push(LsofListener {
                    pid,
                    command: command.clone(),
                    bind_address,
                    port,
                });
            }
            _ => {}
        }
    }
    listeners
}

#[cfg(target_os = "macos")]
pub(super) const ENVIRONMENT: &str = "macos";

#[cfg(target_os = "macos")]
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "macos")]
fn process_info(pid: u32, command: &str) -> super::unix::ProcessInfo {
    let pid_arg = pid.to_string();
    let cmdline = command_output("ps", &["-o", "command=", "-p", &pid_arg])
        .map(|text| text.trim().chars().take(400).collect::<String>())
        .filter(|text| !text.is_empty());
    let exe = command_output("ps", &["-o", "comm=", "-p", &pid_arg])
        .map(|text| text.trim().to_owned())
        .filter(|text| text.starts_with('/'));
    let cwd =
        command_output("lsof", &["-a", "-p", &pid_arg, "-d", "cwd", "-Fn"]).and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix('n'))
                .map(str::to_owned)
        });
    super::unix::ProcessInfo {
        comm: command.to_owned(),
        exe,
        cwd,
        cmdline,
    }
}

#[cfg(target_os = "macos")]
pub(super) fn collect() -> Result<Vec<super::DevPortEntry>, crate::debug::Failure> {
    use std::collections::HashMap;
    let output = command_output("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
        .ok_or_else(|| {
            crate::debug::Failure::new("Não foi possível listar as portas com o lsof").stage("lsof")
        })?;
    let own_pid = std::process::id();
    // macOS: faixa efêmera padrão começa em 49152 (net.inet.ip.portrange.first).
    let ephemeral_low = 49152;
    let mut infos: HashMap<u32, super::unix::ProcessInfo> = HashMap::new();
    let mut entries = Vec::new();
    for listener in parse_lsof_listen(&output) {
        if listener.pid == own_pid {
            continue;
        }
        let info = infos
            .entry(listener.pid)
            .or_insert_with(|| process_info(listener.pid, &listener.command));
        if super::unix::is_unix_noise_process(
            &info.comm,
            info.cmdline.as_deref(),
            info.exe.as_deref(),
            listener.port,
            ephemeral_low,
        ) {
            continue;
        }
        entries.push(super::unix::build_entry(
            ENVIRONMENT,
            listener.pid,
            listener.port,
            &listener.bind_address,
            info,
        ));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{parse_lsof_listen, LsofListener};

    #[test]
    fn parses_lsof_field_output() {
        let output = "p501\ncnode\nf23\nn*:3000\nf24\nn[::1]:5173\np88\ncControlCe\nf7\nn*:7000\np9\ncbroken\nnsem-porta\n";
        let listeners = parse_lsof_listen(output);
        assert_eq!(
            listeners,
            vec![
                LsofListener {
                    pid: 501,
                    command: "node".into(),
                    bind_address: "0.0.0.0".into(),
                    port: 3000
                },
                LsofListener {
                    pid: 501,
                    command: "node".into(),
                    bind_address: "::1".into(),
                    port: 5173
                },
                LsofListener {
                    pid: 88,
                    command: "ControlCe".into(),
                    bind_address: "0.0.0.0".into(),
                    port: 7000
                },
            ]
        );
    }
}
