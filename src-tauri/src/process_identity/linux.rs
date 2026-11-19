//! Identidade de processos no Linux: nome do aplicativo (`.desktop`) e
//! origem do software (Flatpak, Snap ou pacote `dpkg` dono do executável).

use super::lacks_display_name;
use crate::system_overview::TopProcess;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// Pacote dono de cada executável. O dono só muda em reinstalação, então o
/// cache vale pela vida do processo (inclusive "sem dono").
static PACKAGE_OWNERS: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

pub fn enrich(processes: &mut [TopProcess]) {
    for process in processes.iter_mut() {
        if lacks_display_name(process) {
            if let Some(app) = crate::app_icons::linux::desktop_app_for_process(
                process.id,
                process.path.as_deref(),
                &process.name,
            ) {
                process.display_name = Some(app.name);
            }
        }
        if process.publisher.is_none() {
            process.publisher = sandbox_origin(process);
        }
    }

    let pending: Vec<String> = processes
        .iter()
        .filter(|process| process.publisher.is_none())
        .filter_map(|process| process.path.clone())
        .collect();
    let owners = package_owners(&pending);
    for process in processes.iter_mut() {
        if process.publisher.is_some() {
            continue;
        }
        if let Some(package) = process
            .path
            .as_ref()
            .and_then(|path| owners.get(path))
            .cloned()
            .flatten()
        {
            process.publisher = Some(format!("Pacote {package}"));
        }
    }
}

fn sandbox_origin(process: &TopProcess) -> Option<String> {
    if let Some(app) = crate::app_icons::linux::flatpak_app_id(process.id) {
        return Some(format!("Flatpak · {app}"));
    }
    let snap = crate::app_icons::linux::snap_name(process.path.as_deref()?)?;
    Some(format!("Snap · {snap}"))
}

/// Em sistemas com `/usr` mesclado o `dpkg` pode registrar `/bin/x` enquanto
/// o kernel reporta `/usr/bin/x`: consulta as duas formas.
fn query_variants(path: &str) -> Vec<String> {
    let mut variants = vec![path.to_owned()];
    for prefix in ["/usr/bin/", "/usr/sbin/", "/usr/lib/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            variants.push(format!("/{}{rest}", &prefix[5..]));
        }
    }
    variants
}

/// Interpreta `dpkg-query -S`: `pacote[:arq][, outro]: /caminho`.
pub fn parse_dpkg_search(text: &str) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    for line in text.lines() {
        if line.starts_with("diversion ") {
            continue;
        }
        let Some((packages, path)) = line.split_once(": ") else {
            continue;
        };
        let Some(first) = packages.split(',').next().map(str::trim) else {
            continue;
        };
        let package = first.split(':').next().unwrap_or(first).trim();
        let path = path.trim();
        if !package.is_empty() && path.starts_with('/') {
            owners
                .entry(path.to_owned())
                .or_insert_with(|| package.to_owned());
        }
    }
    owners
}

fn package_owners(paths: &[String]) -> HashMap<String, Option<String>> {
    let Ok(mut guard) = PACKAGE_OWNERS.lock() else {
        return HashMap::new();
    };
    let cache = guard.get_or_insert_with(HashMap::new);

    let mut unknown: Vec<&String> = paths
        .iter()
        .filter(|path| !cache.contains_key(*path))
        .collect();
    unknown.sort();
    unknown.dedup();
    let queryable: Vec<&String> = unknown
        .iter()
        .copied()
        .filter(|path| path.starts_with('/') && Path::new(path).is_file())
        .collect();

    if !queryable.is_empty() {
        let args: Vec<String> = queryable
            .iter()
            .flat_map(|path| query_variants(path))
            .collect();
        // Caminhos vêm do kernel (`/proc/<pid>/exe`), passados como argumentos
        // separados, nunca por shell. Saída com código ≠ 0 é normal quando
        // algum caminho não tem dono: o stdout continua válido.
        let found = crate::process::c_locale_command("dpkg-query")
            .arg("-S")
            .arg("--")
            .args(&args)
            .output()
            .ok()
            .map(|output| parse_dpkg_search(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default();
        for path in &queryable {
            let owner = query_variants(path)
                .iter()
                .find_map(|variant| found.get(variant).cloned());
            cache.insert((*path).clone(), owner);
        }
    }
    for path in unknown {
        cache.entry(path.clone()).or_insert(None);
    }

    paths
        .iter()
        .map(|path| (path.clone(), cache.get(path).cloned().flatten()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreta_busca_do_dpkg() {
        let text = "bash: /bin/bash\nlibc6:amd64, libc6-dev:amd64: /usr/lib/x86_64-linux-gnu/libc.so.6\ndiversion by dash from: /bin/sh\nfirefox: /usr/lib/firefox/firefox\n";
        let owners = parse_dpkg_search(text);
        assert_eq!(owners.get("/bin/bash").map(String::as_str), Some("bash"));
        assert_eq!(
            owners
                .get("/usr/lib/x86_64-linux-gnu/libc.so.6")
                .map(String::as_str),
            Some("libc6")
        );
        assert_eq!(
            owners.get("/usr/lib/firefox/firefox").map(String::as_str),
            Some("firefox")
        );
        assert_eq!(owners.len(), 3);
    }

    #[test]
    fn consulta_forma_sem_usr_mesclado() {
        assert_eq!(
            query_variants("/usr/bin/bash"),
            vec!["/usr/bin/bash", "/bin/bash"]
        );
        assert_eq!(query_variants("/opt/app/app"), vec!["/opt/app/app"]);
    }

    #[test]
    fn coleta_real_encontra_dono_do_shell() {
        // Em Debian/Ubuntu/Mint o `sh` pertence a um pacote; sem dpkg, None.
        let has_dpkg = Path::new("/var/lib/dpkg/status").exists();
        let target = std::fs::canonicalize("/bin/sh")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "/bin/sh".into());
        let owners = package_owners(std::slice::from_ref(&target));
        if has_dpkg {
            assert!(owners[&target].is_some(), "{target} sem pacote dono");
        }
    }
}
