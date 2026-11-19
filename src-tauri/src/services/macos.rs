//! Serviços no macOS: jobs do launchd visíveis ao usuário (`launchctl list`).
//!
//! Somente leitura. `launchctl list` sem root mostra o domínio do usuário
//! (LaunchAgents); daemons do sistema exigiriam `sudo`. Controle fica
//! indisponível até ser validado em hardware.
//!
//! TODO(macos): validar em hardware.

use super::command;
use super::WindowsService;
use std::time::Duration;

const LIST_TIMEOUT: Duration = Duration::from_secs(20);

/// Linhas `PID\tStatus\tLabel` do `launchctl list` (cabeçalho ignorado).
/// PID `-` = job carregado mas sem processo.
pub fn parse_launchctl_list(output: &str) -> Vec<WindowsService> {
    output
        .lines()
        .skip_while(|line| line.starts_with("PID"))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let pid = fields.next()?.trim();
            let _status = fields.next()?;
            let label = fields.next()?.trim();
            if label.is_empty() {
                return None;
            }
            let process_id = pid.parse::<u32>().ok().filter(|pid| *pid > 0);
            let (state, state_code) = if process_id.is_some() {
                ("Em execução".to_string(), 4)
            } else {
                ("Parado".to_string(), 1)
            };
            Some(WindowsService {
                name: label.to_string(),
                display_name: label.to_string(),
                state,
                state_code,
                start_type: "Carregado no launchd".to_string(),
                start_type_code: 3,
                process_id,
                executable_path: None,
                icon: None,
                start_name: None,
                is_user_service: false,
                category: "unknown",
                publisher: None,
                suspicion_reasons: Vec::new(),
            })
        })
        .collect()
}

pub fn enumerate() -> Result<Vec<WindowsService>, String> {
    let mut list = command::c_locale_command("launchctl");
    list.arg("list");
    let output = command::run(list, LIST_TIMEOUT)?;
    if !output.success() {
        return Err(format!(
            "Falha ao listar os serviços do launchd: {}",
            output
                .first_error_line()
                .unwrap_or_else(|| "código de saída desconhecido".to_string())
        ));
    }
    let services = parse_launchctl_list(&output.stdout);
    if services.is_empty() {
        return Err("O launchd não retornou nenhum serviço".to_string());
    }
    Ok(services)
}

pub fn control(_name: &str, _action: &str) -> Result<(), String> {
    Err("Indisponível no macOS por enquanto: o controle de serviços do launchd ainda não foi validado".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launchctl_list() {
        let text = "PID\tStatus\tLabel\n123\t0\tcom.apple.Finder\n-\t0\tcom.example.agent\n";
        let services = parse_launchctl_list(text);
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].process_id, Some(123));
        assert_eq!(services[1].state_code, 1);
    }
}
