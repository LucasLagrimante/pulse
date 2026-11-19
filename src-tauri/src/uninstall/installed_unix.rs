//! Inventário de programas fora do Windows (Linux e macOS).
//!
//! Não existe registro: cada origem de instalação tem a própria fonte da
//! verdade e o próprio desinstalador.
//!
//! - Linux (`packages_linux.rs`): pacotes `deb` que instalam um `.desktop`
//!   visível, apps Flatpak, Snaps com atalho e AppImages na pasta pessoal;
//! - macOS (`apps_macos.rs`): pacotes `.app` em `/Applications` e
//!   `~/Applications`.
//!
//! O `id` segue o formato `<origem>|<escopo>|<chave>` (`deb|system|firefox`,
//! `flatpak|user|app/org.x.Y/x86_64/stable`, `appimage|user|/home/u/X.AppImage`)
//! e é a única coisa que o front-end devolve: o comando de remoção é sempre
//! montado aqui, a partir do inventário, nunca a partir de texto vindo do IPC.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::installed::{InstalledApp, InstalledAppsSnapshot};

pub const SOURCE_DEB: &str = "deb";
pub const SOURCE_FLATPAK: &str = "flatpak";
pub const SOURCE_SNAP: &str = "snap";
pub const SOURCE_APPIMAGE: &str = "appimage";
pub const SOURCE_MAC_APP: &str = "macapp";

const KNOWN_SOURCES: &[&str] = &[
    SOURCE_DEB,
    SOURCE_FLATPAK,
    SOURCE_SNAP,
    SOURCE_APPIMAGE,
    SOURCE_MAC_APP,
];

/// Teto de bytes guardados de cada saída de CLI. `dpkg-query -W` com todos os
/// pacotes fica perto de 1 MB; acima disso algo está errado.
const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixAppId {
    pub source: String,
    pub scope: String,
    pub key: String,
}

pub fn build_id(source: &str, scope: &str, key: &str) -> String {
    format!("{source}|{scope}|{key}")
}

/// `splitn(3)`: a chave do AppImage/macOS é um caminho e pode conter `|`.
pub fn parse_id(id: &str) -> Option<UnixAppId> {
    let mut parts = id.splitn(3, '|');
    let source = parts.next()?.trim();
    let scope = parts.next()?.trim();
    let key = parts.next()?;
    if !KNOWN_SOURCES.contains(&source) || scope.is_empty() || key.trim().is_empty() {
        return None;
    }
    Some(UnixAppId {
        source: source.to_string(),
        scope: scope.to_string(),
        key: key.to_string(),
    })
}

/// Esqueleto de um app fora do Windows. `hive`/`view`/`registryPath` não
/// existem aqui: carregam origem, escopo e uma descrição legível da chave,
/// para o front continuar recebendo os mesmos campos.
pub fn base_app(
    source: &str,
    scope: &str,
    key: &str,
    display_name: String,
    location_label: String,
) -> InstalledApp {
    InstalledApp {
        id: build_id(source, scope, key),
        hive: source.to_string(),
        view: scope.to_string(),
        registry_path: location_label,
        display_name,
        publisher: None,
        version: None,
        install_date: None,
        install_location: None,
        estimated_size_kb: None,
        uninstall_string: None,
        is_msi: false,
        icon: None,
        source: Some(source.to_string()),
        protected_reason: None,
        match_names: Vec::new(),
    }
}

/// `YYYYMMDD` local, o mesmo formato do `InstallDate` do Windows que a UI já
/// interpreta. `None` quando o sistema de arquivos não informa a data.
pub fn install_date_from_mtime(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let local: chrono::DateTime<chrono::Local> = modified.into();
    Some(local.format("%Y%m%d").to_string())
}

#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0) && !self.timed_out
    }

    /// Últimas linhas não vazias de stderr (ou stdout, se stderr vazio): é o
    /// que explica uma falha sem despejar o log inteiro na tela.
    pub fn tail(&self, lines: usize) -> Vec<String> {
        let source = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let collected: Vec<String> = source
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| line.chars().take(300).collect())
            .collect();
        let skip = collected.len().saturating_sub(lines);
        collected.into_iter().skip(skip).collect()
    }
}

fn spawn_reader<R: Read + Send + 'static>(mut reader: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    // Continua drenando acima do teto: parar de ler travaria o
                    // processo filho com o pipe cheio.
                    if buffer.len() < MAX_CAPTURE_BYTES {
                        buffer.extend_from_slice(&chunk[..read]);
                    }
                }
            }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    })
}

/// Executa um programa com argumentos fixos (nunca via shell), `LC_ALL=C`
/// para a saída ser parseável, stdin fechado e prazo.
///
/// `kill_on_timeout`: consultas de leitura são encerradas no prazo; um
/// desinstalador **não** (matar `apt`/`flatpak` no meio deixa o sistema
/// inconsistente) — aí o Pulse só para de aguardar.
pub fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
    kill_on_timeout: bool,
) -> std::io::Result<CommandOutput> {
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().map(spawn_reader);
    let stderr = child.stderr.take().map(spawn_reader);
    let started = Instant::now();
    let mut status = None;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = exit.code();
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            if kill_on_timeout {
                let _ = child.kill();
                let _ = child.wait();
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    // Sem `kill`, o processo continua vivo e segura os pipes: não esperamos
    // os leitores para não pendurar a thread junto.
    let join = |handle: Option<std::thread::JoinHandle<String>>| {
        if timed_out && !kill_on_timeout {
            return String::new();
        }
        handle
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    };
    Ok(CommandOutput {
        status,
        stdout: join(stdout),
        stderr: join(stderr),
        timed_out,
    })
}

/// Procura um executável no `PATH` (sem executar nada).
pub fn find_in_path(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .chain(
            [
                "/usr/bin",
                "/usr/sbin",
                "/bin",
                "/usr/local/bin",
                "/snap/bin",
            ]
            .iter()
            .map(std::path::PathBuf::from),
        )
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Inventário completo. Uma origem ilegível vira linha em `partial_reason`;
/// origem ausente (sem Flatpak/Snap instalado) não é problema.
pub fn collect() -> InstalledAppsSnapshot {
    #[cfg(target_os = "linux")]
    let (mut apps, problems, scanned_roots) = super::packages_linux::collect_all();
    #[cfg(target_os = "macos")]
    let (mut apps, problems, scanned_roots) = super::apps_macos::collect_all();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let (mut apps, problems, scanned_roots): (Vec<InstalledApp>, Vec<String>, usize) = (
        Vec::new(),
        vec!["O inventário de programas ainda não é suportado neste sistema".to_string()],
        0,
    );

    apps.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    apps.dedup_by(|right, left| right.id == left.id);

    let total_count = apps.len();
    InstalledAppsSnapshot {
        apps,
        total_count,
        partial_reason: if problems.is_empty() {
            None
        } else {
            Some(problems.join("; "))
        },
        scanned_roots,
        collected_at_unix_ms: now_unix_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_unix_ida_e_volta_com_caminho_contendo_barra_vertical() {
        let id = build_id(SOURCE_APPIMAGE, "user", "/home/u/Applications/a|b.AppImage");
        let parsed = parse_id(&id).expect("id gerado precisa ser legível");
        assert_eq!(parsed.source, SOURCE_APPIMAGE);
        assert_eq!(parsed.scope, "user");
        assert_eq!(parsed.key, "/home/u/Applications/a|b.AppImage");
    }

    #[test]
    fn id_unix_invalido_e_recusado() {
        assert!(parse_id("lixo").is_none());
        assert!(parse_id("deb||firefox").is_none());
        assert!(parse_id("deb|system|").is_none());
        assert!(parse_id("HKLM|SOFTWARE\\X|64").is_none());
        assert!(parse_id("rpm|system|firefox").is_none());
    }

    #[test]
    fn comando_com_prazo_e_saida_capturada() {
        let output = run_command(
            "sh",
            &["-c", "echo ok; echo falha >&2; exit 3"],
            Duration::from_secs(5),
            true,
        )
        .expect("sh existe em qualquer unix");
        assert_eq!(output.status, Some(3));
        assert_eq!(output.stdout.trim(), "ok");
        assert_eq!(output.tail(1), vec!["falha".to_string()]);
        assert!(!output.success());
    }

    #[test]
    fn comando_que_estoura_o_prazo_e_encerrado() {
        let output =
            run_command("sleep", &["5"], Duration::from_millis(200), true).expect("sleep existe");
        assert!(output.timed_out);
        assert!(!output.success());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inventario_real_respeita_invariantes() {
        let snapshot = collect();
        assert_eq!(snapshot.total_count, snapshot.apps.len());
        assert!(snapshot
            .apps
            .iter()
            .all(|app| !app.display_name.trim().is_empty()));
        assert!(snapshot.apps.iter().all(|app| parse_id(&app.id).is_some()));
        assert!(snapshot.apps.iter().all(|app| app.source.is_some()));
        let mut ids: Vec<&str> = snapshot.apps.iter().map(|app| app.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), snapshot.apps.len(), "ids precisam ser únicos");
    }
}
