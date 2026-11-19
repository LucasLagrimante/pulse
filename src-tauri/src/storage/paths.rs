//! Pastas do Pulse por plataforma.
//!
//! No Windows os caminhos são exatamente os de sempre (`%LOCALAPPDATA%\Pulse`,
//! com a pasta temporária como último recurso). Fora do Windows seguem o XDG
//! (Linux: `~/.local/share/Pulse`) ou o padrão do macOS
//! (`~/Library/Application Support/Pulse`).

use std::{env, path::PathBuf};

/// Base dos dados locais, sem o sufixo `Pulse`.
pub fn local_data_base() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
    }
    #[cfg(not(windows))]
    {
        dirs::data_local_dir().unwrap_or_else(env::temp_dir)
    }
}

/// Pasta de dados locais do Pulse (`%LOCALAPPDATA%\Pulse`, `~/.local/share/Pulse`…).
pub fn pulse_local_dir() -> PathBuf {
    local_data_base().join("Pulse")
}

/// Pasta pessoal do usuário (`%USERPROFILE%` no Windows, `$HOME` fora dele).
pub fn user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::{local_data_base, pulse_local_dir, user_home};

    #[test]
    fn pulse_dir_is_absolute_and_named_pulse() {
        let dir = pulse_local_dir();
        assert!(dir.is_absolute());
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("Pulse"));
        assert_eq!(dir.parent(), Some(local_data_base().as_path()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_paths_follow_xdg_and_home() {
        let home = user_home().expect("HOME definido nos testes");
        assert!(home.is_absolute());
        if std::env::var_os("XDG_DATA_HOME").is_none() {
            assert_eq!(local_data_base(), home.join(".local").join("share"));
        }
    }
}
