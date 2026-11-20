//! Atualizações pendentes no macOS via Homebrew (`brew outdated --json=v2`).
//!
//! Somente leitura: lista formulae e casks desatualizados quando o Homebrew
//! está instalado; sem Homebrew declara indisponibilidade com motivo. A
//! atualização automática fica indisponível até validação em hardware.
//!
//! TODO(macos): validar em hardware.

use super::{
    now_unix_ms, unavailable, BatchUpgradeReport, PendingUpdate, UpdatesSnapshot, UpgradeFailure,
};
use crate::services::command;
use std::time::Duration;

const LIST_TIMEOUT: Duration = Duration::from_secs(90);

const UPGRADE_UNAVAILABLE: &str =
    "Indisponível no macOS por enquanto: a atualização de pacotes via Homebrew ainda não foi validada";

fn brew_program() -> Option<String> {
    let path = command::resolve_program("brew", &["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]);
    if std::path::Path::new(&path).is_file() || command::program_exists("brew") {
        Some(path)
    } else {
        None
    }
}

fn format_installed_version(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .last()
            .map(str::to_string)
            .unwrap_or_else(|| "Indisponível".to_string()),
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => text.trim().to_string(),
        _ => "Indisponível".to_string(),
    }
}

/// Interpreta a saída JSON de `brew outdated --json=v2`.
pub fn parse_brew_outdated_json(text: &str) -> Result<Vec<PendingUpdate>, String> {
    let root: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("Não foi possível ler o JSON do Homebrew: {error}"))?;
    let mut updates = Vec::new();

    for (section, prefix, source) in [
        ("formulae", "brew:formula", "Homebrew (formula)"),
        ("casks", "brew:cask", "Homebrew (cask)"),
    ] {
        let Some(entries) = root.get(section).and_then(|v| v.as_array()) else {
            continue;
        };
        for entry in entries {
            let Some(name) = entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let available = entry
                .get("current_version")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Indisponível");
            let current = format_installed_version(entry.get("installed_versions"));
            updates.push(PendingUpdate {
                id: format!("{prefix}:{name}"),
                name: name.to_string(),
                current_version: current,
                available_version: available.to_string(),
                source: source.to_string(),
                icon: None,
            });
        }
    }

    Ok(updates)
}

pub fn collect() -> UpdatesSnapshot {
    let started = std::time::Instant::now();
    let Some(brew) = brew_program() else {
        return unavailable(
            "Homebrew (brew) não encontrado neste sistema para verificar atualizações",
        );
    };

    let mut cmd = command::c_locale_command(&brew);
    cmd.args(["outdated", "--json=v2"]);
    let output = match command::run(cmd, LIST_TIMEOUT) {
        Ok(output) => output,
        Err(error) => {
            crate::debug::record_failure(
                "updates_list_macos",
                started.elapsed().as_millis(),
                crate::debug::Failure::new("Falha ao consultar o Homebrew")
                    .stage("brew outdated")
                    .detail(&error),
            );
            return unavailable(error);
        }
    };
    if !output.success() {
        let detail = output
            .first_error_line()
            .unwrap_or_else(|| format!("código {:?}", output.code));
        crate::debug::record_failure(
            "updates_list_macos",
            started.elapsed().as_millis(),
            crate::debug::Failure::new("O Homebrew retornou erro")
                .stage("brew outdated")
                .detail(&detail),
        );
        return unavailable(format!(
            "Não foi possível consultar atualizações no Homebrew: {detail}"
        ));
    }

    match parse_brew_outdated_json(&output.stdout) {
        Ok(updates) => {
            let total = updates.len();
            UpdatesSnapshot {
                updates,
                total_count: total,
                unavailable_reason: None,
                collected_at_unix_ms: now_unix_ms(),
            }
        }
        Err(error) => unavailable(error),
    }
}

pub fn upgrade() -> Result<String, UpgradeFailure> {
    Err(UpgradeFailure::plain(UPGRADE_UNAVAILABLE))
}

pub fn upgrade_all() -> Result<BatchUpgradeReport, UpgradeFailure> {
    Err(UpgradeFailure::plain(UPGRADE_UNAVAILABLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_brew_outdated_v2_json() {
        let json = r#"{
            "formulae": [
                {
                    "name": "git",
                    "installed_versions": ["2.46.0", "2.47.0"],
                    "current_version": "2.47.1"
                }
            ],
            "casks": [
                {
                    "name": "visual-studio-code",
                    "installed_versions": ["1.95.0"],
                    "current_version": "1.96.0"
                }
            ]
        }"#;
        let updates = parse_brew_outdated_json(json).expect("JSON válido");
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].id, "brew:formula:git");
        assert_eq!(updates[0].current_version, "2.47.0");
        assert_eq!(updates[0].available_version, "2.47.1");
        assert_eq!(updates[0].source, "Homebrew (formula)");
        assert_eq!(updates[1].id, "brew:cask:visual-studio-code");
        assert_eq!(updates[1].current_version, "1.95.0");
        assert_eq!(updates[1].available_version, "1.96.0");
        assert_eq!(updates[1].source, "Homebrew (cask)");
    }
}
