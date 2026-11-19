//! Itens de inicialização no macOS: plists do launchd (somente leitura).
//!
//! `~/Library/LaunchAgents`, `/Library/LaunchAgents` e `/Library/LaunchDaemons`.
//! Os plists podem ser binários; `plutil -convert json` normaliza os dois
//! formatos. Ativar/desativar fica indisponível até validação em hardware.
//!
//! TODO(macos): validar em hardware.

use super::StartupItem;
use crate::services::command;
use std::path::{Path, PathBuf};
use std::time::Duration;

const READ_ONLY_REASON: &str =
    "Indisponível no macOS por enquanto: alterar itens do launchd ainda não foi validado";

/// Label, comando e estado de um plist já convertido para JSON.
pub fn parse_plist_json(text: &str) -> Option<(String, String, bool)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let label = value.get("Label")?.as_str()?.to_string();
    let command = value
        .get("ProgramArguments")
        .and_then(|args| args.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|cmd| !cmd.is_empty())
        .or_else(|| {
            value
                .get("Program")
                .and_then(|p| p.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let disabled = value
        .get("Disabled")
        .and_then(|d| d.as_bool())
        .unwrap_or(false);
    Some((label, command, !disabled))
}

fn read_dir(dir: &Path, origin: &str) -> Vec<StartupItem> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().is_none_or(|ext| ext != "plist") {
            continue;
        }
        let mut convert = command::c_locale_command("plutil");
        convert
            .args(["-convert", "json", "-o", "-", "--"])
            .arg(&path);
        let Ok(output) = command::run(convert, Duration::from_secs(5)) else {
            continue;
        };
        let Some((label, command_line, enabled)) = output
            .success()
            .then(|| parse_plist_json(&output.stdout))
            .flatten()
        else {
            continue;
        };
        items.push(StartupItem {
            name: label,
            command: command_line,
            origin: origin.to_string(),
            enabled,
            can_change: false,
            unavailable_reason: Some(READ_ONLY_REASON.to_string()),
            impact: "Indisponível".to_string(),
            startup_time_ms: None,
            icon: None,
        });
    }
    items
}

pub fn collect() -> Vec<StartupItem> {
    let mut dirs: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push((home.join("Library/LaunchAgents"), "LaunchAgents (usuário)"));
    }
    dirs.push((
        PathBuf::from("/Library/LaunchAgents"),
        "LaunchAgents (sistema)",
    ));
    dirs.push((
        PathBuf::from("/Library/LaunchDaemons"),
        "LaunchDaemons (sistema)",
    ));
    dirs.iter()
        .flat_map(|(dir, origin)| read_dir(dir, origin))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plist_json() {
        let text = r#"{"Label":"com.example.agent","ProgramArguments":["/usr/local/bin/x","--y"],"Disabled":true}"#;
        assert_eq!(
            parse_plist_json(text),
            Some((
                "com.example.agent".into(),
                "/usr/local/bin/x --y".into(),
                false
            ))
        );
    }
}
