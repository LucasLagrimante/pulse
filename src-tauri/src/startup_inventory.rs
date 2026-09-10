use serde::{Deserialize, Serialize};
use std::process::Command;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupItem {
    pub name: String,
    pub command: String,
    pub origin: String,
    pub enabled: bool,
    pub can_change: bool,
    pub unavailable_reason: Option<String>,
}

#[tauri::command]
pub fn list_startup_items() -> Result<Vec<StartupItem>, String> {
    let out = Command::new("reg.exe")
        .args(["query", RUN_KEY])
        .output()
        .map_err(|e| format!("Não foi possível ler a inicialização: {e}"))?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_run_entries(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse_run_entries(text: &str) -> Vec<StartupItem> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() < 3 {
                return None;
            }
            Some(StartupItem {
                name: parts[0].trim().to_owned(),
                command: parts[2].trim().to_owned(),
                origin: "HKCU\\Run".into(),
                enabled: true,
                can_change: true,
                unavailable_reason: None,
            })
        })
        .collect()
}

#[tauri::command]
pub fn set_startup_item_enabled(name: String, enabled: bool) -> Result<StartupItem, String> {
    if name == "Pulse" {
        return Err("O autostart do Pulse é gerenciado pela Visão geral".into());
    }
    if name.is_empty() || name.contains('"') {
        return Err("Nome de entrada inválido".into());
    }
    if enabled {
        return Err(
            "Reativação exige preservar o comando original e não é suportada nesta versão".into(),
        );
    }
    let args = vec!["delete", RUN_KEY, "/v", &name, "/f"];
    let out = Command::new("reg.exe")
        .args(args)
        .output()
        .map_err(|e| format!("Não foi possível alterar a inicialização: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    if !enabled {
        return Ok(StartupItem {
            name,
            command: String::new(),
            origin: "HKCU\\Run".into(),
            enabled: false,
            can_change: true,
            unavailable_reason: None,
        });
    }
    list_startup_items()?
        .into_iter()
        .find(|i| i.name == name)
        .ok_or_else(|| "Entrada não encontrada após a alteração".into())
}

#[cfg(test)]
mod tests {
    use super::parse_run_entries;
    #[test]
    fn parses_hkcu_run_entries() {
        let v =
            parse_run_entries("HKEY_CURRENT_USER\\...\n    Foo    REG_SZ    \"C:\\\\Foo.exe\"\n");
        assert_eq!(v[0].name, "Foo");
        assert!(v[0].can_change);
    }
}
