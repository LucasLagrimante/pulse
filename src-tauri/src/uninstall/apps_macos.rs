//! Inventário de aplicativos no macOS: pacotes `.app` em `/Applications` e
//! `~/Applications` (e uma subpasta abaixo deles, onde fabricantes agrupam
//! apps). O `Info.plist` é lido com `plutil -convert json` — sem dependência
//! nova para plist binário.
//!
//! TODO(macos): validar em hardware (nomes, ícones `.icns` ainda não
//! convertidos, apps do App Store dentro de `/Applications`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::installed::InstalledApp;
use super::installed_unix::{base_app, install_date_from_mtime, run_command, SOURCE_MAC_APP};

const PLUTIL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_APPS_PER_ROOT: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BundleInfo {
    pub bundle_id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
}

/// Campos do `Info.plist` já convertido para JSON.
pub fn parse_info_plist_json(text: &str) -> Option<BundleInfo> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
    };
    Some(BundleInfo {
        bundle_id: field("CFBundleIdentifier"),
        name: field("CFBundleDisplayName").or_else(|| field("CFBundleName")),
        version: field("CFBundleShortVersionString").or_else(|| field("CFBundleVersion")),
    })
}

pub fn mac_protected_reason(bundle_id: Option<&str>) -> Option<String> {
    bundle_id
        .filter(|id| id.starts_with("com.apple."))
        .map(|_| "Aplicativo da Apple que acompanha o macOS".to_string())
}

/// Pastas de aplicativos com o escopo de cada uma.
pub fn app_roots(home: Option<&Path>) -> Vec<(PathBuf, &'static str)> {
    let mut roots = vec![(PathBuf::from("/Applications"), "system")];
    if let Some(home) = home {
        roots.push((home.join("Applications"), "user"));
    }
    roots
}

fn is_app_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "app")
        && fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
}

fn bundles_in(root: &Path) -> Vec<PathBuf> {
    let Ok(iter) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in iter.flatten().take(MAX_APPS_PER_ROOT) {
        let path = entry.path();
        if is_app_bundle(&path) {
            found.push(path);
        } else if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_dir()) {
            // Um nível abaixo: `/Applications/Fabricante/App.app`.
            if let Ok(inner) = fs::read_dir(&path) {
                found.extend(
                    inner
                        .flatten()
                        .take(64)
                        .map(|item| item.path())
                        .filter(|item| is_app_bundle(item)),
                );
            }
        }
    }
    found
}

fn read_bundle(path: &Path) -> Option<BundleInfo> {
    let plist = path.join("Contents/Info.plist");
    let plist_text = plist.to_string_lossy().to_string();
    let output = run_command(
        "/usr/bin/plutil",
        &["-convert", "json", "-o", "-", &plist_text],
        PLUTIL_TIMEOUT,
        true,
    )
    .ok()?;
    if !output.success() {
        return None;
    }
    parse_info_plist_json(&output.stdout)
}

pub fn collect_all() -> (Vec<InstalledApp>, Vec<String>, usize) {
    let home = dirs::home_dir();
    let mut apps = Vec::new();
    let mut problems = Vec::new();
    let mut scanned = 0usize;
    for (root, scope) in app_roots(home.as_deref()) {
        if !root.is_dir() {
            continue;
        }
        scanned += 1;
        for bundle in bundles_in(&root) {
            let path_text = bundle.to_string_lossy().to_string();
            let info = read_bundle(&bundle).unwrap_or_default();
            let stem = bundle
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| path_text.clone());
            let mut app = base_app(
                SOURCE_MAC_APP,
                scope,
                &path_text,
                info.name.clone().unwrap_or_else(|| stem.clone()),
                path_text.clone(),
            );
            app.version = info.version.clone();
            app.install_date = install_date_from_mtime(&bundle);
            app.install_location = Some(path_text.clone());
            app.uninstall_string = Some(format!("Mover para a Lixeira: {path_text}"));
            app.protected_reason = mac_protected_reason(info.bundle_id.as_deref());
            let mut names = Vec::new();
            if let Some(bundle_id) = &info.bundle_id {
                names.push(bundle_id.clone());
            }
            names.push(info.name.clone().unwrap_or(stem));
            app.match_names = names;
            apps.push(app);
        }
    }
    if scanned == 0 {
        problems.push("Nenhuma pasta de aplicativos pôde ser lida".to_string());
    }
    (apps, problems, scanned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_plist_em_json() {
        let info = parse_info_plist_json(
            r#"{"CFBundleIdentifier":"com.example.Foto","CFBundleName":"Foto","CFBundleShortVersionString":"3.1"}"#,
        )
        .unwrap();
        assert_eq!(info.bundle_id.as_deref(), Some("com.example.Foto"));
        assert_eq!(info.name.as_deref(), Some("Foto"));
        assert_eq!(info.version.as_deref(), Some("3.1"));
        assert!(parse_info_plist_json("não é json").is_none());
    }

    #[test]
    fn apps_da_apple_sao_protegidos() {
        assert!(mac_protected_reason(Some("com.apple.Safari")).is_some());
        assert!(mac_protected_reason(Some("com.example.Foto")).is_none());
        assert!(mac_protected_reason(None).is_none());
    }
}
