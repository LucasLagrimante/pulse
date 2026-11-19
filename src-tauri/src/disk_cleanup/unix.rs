//! Categorias de limpeza e regras de preservação fora do Windows.
//!
//! Mesmos ids do Windows onde o conceito é o mesmo (`user-temp`,
//! `browser-cache`, `shader-cache`, `nvidia-shader-cache`, `crash-dumps`,
//! `dev-package-caches`, `recycle-bin`), mais `thumbnails` (cache de miniaturas
//! freedesktop, sem equivalente separado no Windows).
//!
//! Decisão: nada aqui pede senha. Áreas do sistema (cache do APT, journal do
//! systemd, runtimes Flatpak sem uso) viram "Recuperação guiada": o Pulse mede e
//! mostra o comando oficial (`apt-get clean`, `journalctl --vacuum-time`,
//! `flatpak uninstall --unused`) para o usuário executar, igual ao DISM no
//! Windows. Assim a limpeza do Pulse continua sem ações privilegiadas e sem
//! apagar à mão arquivos que têm ferramenta própria.
//!
//! Proteções extras (o Linux não bloqueia arquivo aberto como o Windows):
//! - só entra o que pertence ao usuário (`st_uid`) e é arquivo ou pasta comum
//!   (sockets, FIFOs e dispositivos nunca são removidos);
//! - não atravessa para outro sistema de arquivos (`st_dev` da raiz);
//! - na limpeza, arquivos abertos por algum processo do usuário (`/proc/*/fd`)
//!   são preservados e contados como "em uso";
//! - em `/tmp`, sockets e credenciais de sessão (X11, ssh-agent, tmux,
//!   Kerberos, `XAUTHORITY`) ficam de fora pelo nome.

use super::{CleanupDefinition, GuidedInsight};
use std::{
    collections::HashSet,
    env, fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

// ---------------------------------------------------------------------------
// Regras de preservação
// ---------------------------------------------------------------------------

/// Arquivos e pastas abertos agora por processos do usuário, como
/// `(st_dev, st_ino)`. Tirado uma vez no início da limpeza.
pub(super) struct InUseFiles {
    inodes: HashSet<(u64, u64)>,
}

impl InUseFiles {
    /// Linux: descritores abertos e diretório atual de cada processo legível em
    /// `/proc` (os do próprio usuário). Mapeamentos `mmap` sem descritor aberto
    /// não entram.
    #[cfg(target_os = "linux")]
    pub(super) fn snapshot() -> Self {
        let mut inodes = HashSet::new();
        let Ok(processes) = fs::read_dir("/proc") else {
            return Self { inodes };
        };
        let mut insert = |path: &Path| {
            if let Ok(meta) = fs::metadata(path) {
                if meta.is_file() || meta.is_dir() {
                    inodes.insert((meta.dev(), meta.ino()));
                }
            }
        };
        for process in processes.flatten() {
            let is_pid = process
                .file_name()
                .to_str()
                .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()));
            if !is_pid {
                continue;
            }
            let base = process.path();
            insert(&base.join("cwd"));
            if let Ok(descriptors) = fs::read_dir(base.join("fd")) {
                for descriptor in descriptors.flatten() {
                    insert(&descriptor.path());
                }
            }
        }
        Self { inodes }
    }

    /// TODO(macos): validar em hardware. Sem `/proc`, só um `lsof` completo
    /// diria o que está aberto; por ora o macOS não oferece `user-temp`, a
    /// categoria em que isso mais pesa.
    #[cfg(not(target_os = "linux"))]
    pub(super) fn snapshot() -> Self {
        Self {
            inodes: HashSet::new(),
        }
    }

    fn contains(&self, metadata: &fs::Metadata) -> bool {
        self.inodes.contains(&(metadata.dev(), metadata.ino()))
    }
}

/// Prefixos de nomes em `/tmp` que são sockets, travas ou credenciais da
/// sessão. Apagar quebra o login gráfico, o ssh-agent, o tmux ou o Kerberos
/// mesmo quando a data é antiga (são criados no login e não mudam).
const PROTECTED_TEMP_PREFIXES: &[&str] = &[
    ".X",
    ".ICE-unix",
    ".font-unix",
    ".Test-unix",
    "ssh-",
    "tmux-",
    "screen",
    "krb5cc",
    "xauth",
    ".xauth",
    "sddm",
    "lightdm",
    "gdm",
    "systemd-private-",
    "snap-private-tmp",
    "kde-",
    "kdeinit",
    "runtime-",
    "dbus-",
    "keyring-",
    ".mount_",
];

pub(super) fn is_protected_temp_name(name: &str) -> bool {
    PROTECTED_TEMP_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

pub(super) struct EntryGuard<'a> {
    uid: u32,
    device: Option<u64>,
    temp_rules: bool,
    xauthority: Option<PathBuf>,
    in_use: Option<&'a InUseFiles>,
}

impl<'a> EntryGuard<'a> {
    pub(super) fn new(id: &str, root: &Path, in_use: Option<&'a InUseFiles>) -> Self {
        Self {
            // SAFETY: sem parâmetros nem efeitos colaterais.
            uid: unsafe { libc::geteuid() },
            device: fs::metadata(root).ok().map(|meta| meta.dev()),
            temp_rules: id == "user-temp",
            xauthority: env::var_os("XAUTHORITY").map(PathBuf::from),
            in_use,
        }
    }

    /// Fora da análise e da limpeza: de outro usuário, de outro volume, tipo
    /// especial (socket, FIFO, dispositivo) ou item de sessão protegido.
    pub(super) fn excludes(&self, path: &Path, metadata: &fs::Metadata) -> bool {
        let file_type = metadata.file_type();
        if !(file_type.is_file() || file_type.is_dir()) {
            return true;
        }
        if metadata.uid() != self.uid {
            return true;
        }
        if self.device.is_some_and(|device| device != metadata.dev()) {
            return true;
        }
        if self.temp_rules {
            let protected_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_protected_temp_name);
            if protected_name || self.xauthority.as_deref() == Some(path) {
                return true;
            }
        }
        false
    }

    /// Aberto por algum processo no momento da limpeza.
    pub(super) fn in_use(&self, _path: &Path, metadata: &fs::Metadata) -> bool {
        self.in_use.is_some_and(|files| files.contains(metadata))
    }
}

// ---------------------------------------------------------------------------
// Categorias
// ---------------------------------------------------------------------------

/// Sem categorias que exijam administrador: as áreas do sistema viram
/// recuperação guiada (ver o topo do módulo).
pub(super) fn requires_admin(_id: &str) -> bool {
    false
}

fn existing_children(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// `$TMPDIR` e `/tmp`, sem repetir quando são a mesma pasta.
#[cfg(target_os = "linux")]
fn temp_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for candidate in [env::temp_dir(), PathBuf::from("/tmp")] {
        let canonical = fs::canonicalize(&candidate).unwrap_or(candidate);
        if !roots.contains(&canonical) {
            roots.push(canonical);
        }
    }
    roots
}

/// Pastas de cache de navegadores no Linux: Chromium e derivados em
/// `~/.cache/<navegador>`, Firefox por perfil em `cache2`, e as variantes
/// Flatpak (`~/.var/app/<id>/cache`) e Snap (`~/snap/<nome>/common/.cache`).
#[cfg(target_os = "linux")]
pub(super) fn linux_browser_roots(home: &Path, cache: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = [
        "google-chrome",
        "google-chrome-beta",
        "google-chrome-unstable",
        "chromium",
        "microsoft-edge",
        "microsoft-edge-beta",
        "microsoft-edge-dev",
        "BraveSoftware",
        "vivaldi",
        "opera",
    ]
    .iter()
    .map(|name| cache.join(name))
    .collect();
    roots.extend(
        existing_children(&cache.join("mozilla").join("firefox"))
            .into_iter()
            .map(|profile| profile.join("cache2")),
    );
    for app in [
        "com.google.Chrome",
        "org.chromium.Chromium",
        "com.microsoft.Edge",
        "com.brave.Browser",
        "com.vivaldi.Vivaldi",
        "com.opera.Opera",
        "org.mozilla.firefox",
        "io.gitlab.librewolf-community",
    ] {
        roots.push(home.join(".var").join("app").join(app).join("cache"));
    }
    for snap in ["firefox", "chromium"] {
        roots.push(home.join("snap").join(snap).join("common").join(".cache"));
    }
    roots
}

#[cfg(target_os = "linux")]
pub(super) fn definitions() -> Vec<CleanupDefinition> {
    let home = dirs::home_dir().unwrap_or_else(env::temp_dir);
    let cache = dirs::cache_dir().unwrap_or_else(|| home.join(".cache"));
    vec![
        CleanupDefinition {
            id: "user-temp",
            name: "Arquivos temporários do usuário",
            description: "Sobras temporárias suas em /tmp. Arquivos abertos por programas, das últimas 24 horas, de outros usuários, sockets e credenciais da sessão (X11, ssh-agent, tmux) são preservados.",
            recommended: true,
            roots: temp_roots(),
        },
        CleanupDefinition {
            id: "browser-cache",
            name: "Cache de navegadores",
            description: "Cache local do Chrome, Chromium, Edge, Brave, Vivaldi, Opera e Firefox (inclusive Flatpak e Snap). Sites podem carregar mais devagar na primeira visita; arquivos abertos pelo navegador são preservados.",
            recommended: true,
            roots: linux_browser_roots(&home, &cache),
        },
        CleanupDefinition {
            id: "shader-cache",
            name: "Cache de shaders (Mesa)",
            description: "Shaders compilados pelos drivers Mesa (AMD, Intel e Nouveau) que jogos e aplicativos recriam quando necessário.",
            recommended: true,
            roots: vec![
                cache.join("mesa_shader_cache"),
                cache.join("mesa_shader_cache_db"),
                cache.join("radv_builtin_shaders"),
            ],
        },
        CleanupDefinition {
            id: "nvidia-shader-cache",
            name: "Cache de shaders NVIDIA",
            description: "Shaders compilados pelo driver NVIDIA. Jogos recompilam na primeira abertura, com leve travada inicial.",
            recommended: true,
            roots: vec![
                cache.join("nvidia").join("GLCache"),
                home.join(".nv").join("GLCache"),
                home.join(".nv").join("ComputeCache"),
            ],
        },
        CleanupDefinition {
            id: "thumbnails",
            name: "Miniaturas de arquivos",
            description: "Prévias de imagens e vídeos geradas pelo gerenciador de arquivos. São recriadas ao abrir as pastas de novo.",
            recommended: true,
            roots: vec![cache.join("thumbnails"), home.join(".thumbnails")],
        },
        CleanupDefinition {
            id: "crash-dumps",
            name: "Relatórios de falha",
            description: "Relatórios do Apport em /var/crash gerados pelos seus programas. Desmarque se pretende investigar ou enviar uma falha recente.",
            recommended: false,
            roots: vec![PathBuf::from("/var/crash")],
        },
        CleanupDefinition {
            id: "dev-package-caches",
            // Ficam de fora: ~/.m2 (guarda artefatos do `mvn install` que não
            // são baixados de novo), o store do pnpm (ligado por hard link aos
            // node_modules), o módulo cache do Go (somente leitura) e o
            // Chrome do Puppeteer (não é baixado de novo sozinho).
            description: "Pacotes baixados por npm, pip, uv, Yarn, Gradle e Cargo e o cache de build do Go. São baixados de novo no próximo build, que fica mais lento uma vez. Feche builds em andamento antes; só roda na limpeza manual.",
            name: "Caches de ferramentas de desenvolvimento",
            recommended: false,
            roots: vec![
                home.join(".npm").join("_cacache"),
                cache.join("pip"),
                cache.join("uv"),
                cache.join("yarn"),
                home.join(".gradle").join("caches"),
                home.join(".cargo").join("registry").join("cache"),
                cache.join("go-build"),
            ],
        },
        CleanupDefinition {
            id: "recycle-bin",
            name: "Lixeira",
            description: "Arquivos excluídos que ainda podem ser restaurados (pasta pessoal e outros discos montados). Ao limpar, a exclusão se torna permanente.",
            recommended: false,
            roots: Vec::new(),
        },
    ]
}

/// TODO(macos): validar em hardware. Pastas de `~/Library` protegidas pelo TCC
/// (Safari, Mail) ficam de fora; sem acesso, a categoria aparece como pasta não
/// encontrada em vez de inventar tamanho.
#[cfg(target_os = "macos")]
pub(super) fn definitions() -> Vec<CleanupDefinition> {
    let home = dirs::home_dir().unwrap_or_else(env::temp_dir);
    let caches = home.join("Library").join("Caches");
    let mut browser_roots: Vec<PathBuf> = [
        caches.join("Google").join("Chrome"),
        caches.join("Chromium"),
        caches.join("Microsoft Edge"),
        caches.join("BraveSoftware"),
    ]
    .into_iter()
    .collect();
    browser_roots.extend(
        existing_children(&caches.join("Firefox").join("Profiles"))
            .into_iter()
            .map(|profile| profile.join("cache2")),
    );
    vec![
        CleanupDefinition {
            id: "browser-cache",
            name: "Cache de navegadores",
            description: "Cache local do Chrome, Chromium, Edge, Brave e Firefox. Sites podem carregar mais devagar na primeira visita.",
            recommended: true,
            roots: browser_roots,
        },
        CleanupDefinition {
            id: "crash-dumps",
            name: "Relatórios de falha",
            description: "Relatórios de diagnóstico dos seus aplicativos. Desmarque se pretende investigar uma falha recente.",
            recommended: false,
            roots: vec![home.join("Library").join("Logs").join("DiagnosticReports")],
        },
        CleanupDefinition {
            id: "dev-package-caches",
            name: "Caches de ferramentas de desenvolvimento",
            description: "Pacotes baixados por npm, pip, uv, Yarn, Gradle e Cargo. São baixados de novo no próximo build, que fica mais lento uma vez. Feche builds em andamento antes; só roda na limpeza manual.",
            recommended: false,
            roots: vec![
                home.join(".npm").join("_cacache"),
                caches.join("pip"),
                home.join(".cache").join("uv"),
                caches.join("Yarn"),
                home.join(".gradle").join("caches"),
                home.join(".cargo").join("registry").join("cache"),
            ],
        },
        CleanupDefinition {
            id: "recycle-bin",
            name: "Lixeira",
            description: "Arquivos excluídos que ainda podem ser restaurados. Ao limpar, a exclusão se torna permanente.",
            recommended: false,
            roots: Vec::new(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Recuperação guiada: o Pulse mede e mostra o comando; o usuário executa.
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
const MAX_INSIGHT_FILES: usize = 20_000;

/// Soma o tamanho dos arquivos comuns sob `dir` (até 3 níveis, com teto de
/// arquivos). `None` se a pasta não pôde ser lida.
#[cfg(target_os = "linux")]
fn sum_regular_files(dir: &Path) -> Option<u64> {
    fn walk(dir: &Path, depth: usize, total: &mut u64, files: &mut usize) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            if *files >= MAX_INSIGHT_FILES {
                break;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_file() {
                *files += 1;
                *total = total.saturating_add(meta.len());
                if (*files).is_multiple_of(15) {
                    std::thread::yield_now();
                }
            } else if meta.is_dir() && depth < 3 {
                walk(&entry.path(), depth + 1, total, files);
            }
        }
        true
    }
    let mut total = 0;
    let mut files = 0;
    walk(dir, 0, &mut total, &mut files).then_some(total)
}

#[cfg(target_os = "linux")]
fn command_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

#[cfg(target_os = "linux")]
pub(super) fn guided_insights() -> Vec<GuidedInsight> {
    let mut insights = Vec::new();

    let apt_archives = Path::new("/var/cache/apt/archives");
    if apt_archives.is_dir() {
        let size = sum_regular_files(apt_archives);
        insights.push(GuidedInsight {
            id: "apt-cache".into(),
            title: "Pacotes baixados pelo APT".into(),
            detail: "Cópias .deb de atualizações e instalações já aplicadas. O APT baixa de novo só se precisar reinstalar uma versão.".into(),
            path: Some(apt_archives.to_string_lossy().into_owned()),
            unavailable_reason: size
                .is_none()
                .then(|| "A pasta do cache do APT não pôde ser lida".into()),
            size_bytes: size,
            warning: "Não apague os arquivos à mão: o apt-get clean também limpa as listas parciais com segurança.".into(),
            command: "# Terminal (pede a senha de administrador)\nsudo apt-get clean".into(),
        });
    }

    let journal = Path::new("/var/log/journal");
    if journal.is_dir() {
        let size = sum_regular_files(journal);
        insights.push(GuidedInsight {
            id: "systemd-journal".into(),
            title: "Logs do sistema (journal)".into(),
            detail: "Registros antigos do systemd-journald. Manter as últimas duas semanas costuma bastar para investigar problemas recentes.".into(),
            path: Some(journal.to_string_lossy().into_owned()),
            unavailable_reason: size
                .is_none()
                .then(|| "Os arquivos do journal não puderam ser lidos".into()),
            size_bytes: size,
            warning: "Nunca apague arquivos .journal à mão; use o próprio journalctl. Logs removidos não podem ser recuperados.".into(),
            command: "# Terminal (pede a senha de administrador)\nsudo journalctl --vacuum-time=2weeks".into(),
        });
    }

    if command_on_path("flatpak") {
        insights.push(GuidedInsight {
            id: "flatpak-unused".into(),
            title: "Runtimes Flatpak sem uso".into(),
            detail: "Bibliotecas e runtimes que nenhum aplicativo Flatpak instalado usa mais, deixados por atualizações e desinstalações.".into(),
            path: None,
            size_bytes: None,
            unavailable_reason: Some(
                "Não medido: runtimes são compartilhados entre aplicativos".into(),
            ),
            warning: "O comando lista o que vai remover e pede confirmação; aplicativos instalados não são afetados.".into(),
            command: "# Terminal\nflatpak uninstall --unused".into(),
        });
    }

    insights
}

/// TODO(macos): snapshots locais do Time Machine (`tmutil`) e caches do Xcode
/// são candidatos naturais; ficam para quando houver hardware para validar.
#[cfg(target_os = "macos")]
pub(super) fn guided_insights() -> Vec<GuidedInsight> {
    Vec::new()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn session_items_in_tmp_are_protected_by_name() {
        for name in [
            ".X11-unix",
            ".X0-lock",
            ".ICE-unix",
            "ssh-XXXXabc",
            "tmux-1000",
            "krb5cc_1000",
            "xauth_AbCd",
            "systemd-private-abc-colord.service-x",
            ".mount_AppXyz",
        ] {
            assert!(is_protected_temp_name(name), "{name}");
        }
        for name in [
            "pip-build-123",
            "tmpabc123",
            "claude-1000",
            ".org.chromium.Chromium.x",
        ] {
            assert!(!is_protected_temp_name(name), "{name}");
        }
    }

    #[test]
    fn guard_excludes_sockets_and_keeps_regular_user_files() {
        let dir = env::temp_dir().join(format!("pulse-guard-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("velho.tmp");
        fs::write(&file, b"x").unwrap();
        let socket_path = dir.join("agent.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let xauth = dir.join("serverauth.1234");
        fs::write(&xauth, b"x").unwrap();

        let guard = EntryGuard {
            xauthority: Some(xauth.clone()),
            ..EntryGuard::new("user-temp", &dir, None)
        };
        let meta = |path: &Path| fs::symlink_metadata(path).unwrap();
        assert!(!guard.excludes(&file, &meta(&file)));
        assert!(guard.excludes(&socket_path, &meta(&socket_path)));
        assert!(guard.excludes(&xauth, &meta(&xauth)));
        // Fora de `user-temp` a regra de nomes de sessão não vale.
        let cache_guard = EntryGuard::new("browser-cache", &dir, None);
        assert!(!cache_guard.excludes(&xauth, &meta(&xauth)));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Coleta real: um arquivo mantido aberto por este processo aparece em
    /// `/proc/self/fd` e é preservado pela limpeza.
    #[test]
    fn open_files_snapshot_sees_files_held_by_processes() {
        let path = env::temp_dir().join(format!("pulse-in-use-{}.tmp", std::process::id()));
        let handle = fs::File::create(&path).unwrap();
        let closed = env::temp_dir().join(format!("pulse-closed-{}.tmp", std::process::id()));
        fs::write(&closed, b"x").unwrap();

        let snapshot = InUseFiles::snapshot();
        let guard = EntryGuard::new("user-temp", &env::temp_dir(), Some(&snapshot));
        assert!(guard.in_use(&path, &fs::metadata(&path).unwrap()));
        assert!(!guard.in_use(&closed, &fs::metadata(&closed).unwrap()));

        drop(handle);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&closed);
    }

    #[test]
    fn linux_definitions_reuse_windows_ids_and_skip_risky_caches() {
        let definitions = definitions();
        let ids: Vec<&str> = definitions.iter().map(|d| d.id).collect();
        for id in [
            "user-temp",
            "browser-cache",
            "dev-package-caches",
            "recycle-bin",
            "thumbnails",
        ] {
            assert!(ids.contains(&id), "{id}");
        }
        let all_roots: Vec<String> = definitions
            .iter()
            .flat_map(|d| d.roots.iter().map(|r| r.to_string_lossy().into_owned()))
            .collect();
        for risky in ["/.m2", "puppeteer", "pnpm", "/go/pkg/mod"] {
            assert!(
                !all_roots.iter().any(|root| root.contains(risky)),
                "{risky}"
            );
        }
    }

    #[test]
    fn browser_roots_cover_native_flatpak_and_firefox_profiles() {
        let home = env::temp_dir().join(format!("pulse-browser-home-{}", std::process::id()));
        let cache = home.join(".cache");
        fs::create_dir_all(cache.join("mozilla/firefox/abc.default-release")).unwrap();
        let roots = linux_browser_roots(&home, &cache);
        assert!(roots.contains(&cache.join("google-chrome")));
        assert!(roots.contains(&cache.join("mozilla/firefox/abc.default-release/cache2")));
        assert!(roots.contains(&home.join(".var/app/org.mozilla.firefox/cache")));
        fs::remove_dir_all(&home).unwrap();
    }

    /// Coleta real só de leitura: a análise de cada categoria desta máquina
    /// termina, e categorias com raiz existente nunca inventam motivo.
    #[test]
    fn real_analysis_of_linux_categories_is_sane() {
        for definition in definitions() {
            let id = definition.id;
            let any_root = definition.roots.iter().any(|root| root.is_dir());
            let category = super::super::analyze_definition(definition);
            eprintln!(
                "{id}: {} bytes, {} itens, parcial={}, motivo={:?}",
                category.available_bytes,
                category.item_count,
                category.is_partial,
                category.unavailable_reason
            );
            if id == "recycle-bin" {
                assert!(category.unavailable_reason.is_none(), "{:?}", category);
            } else {
                assert_eq!(category.unavailable_reason.is_none(), any_root, "{id}");
            }
            assert!(category.largest_items.len() <= 12);
        }
    }

    /// Coleta real: nesta máquina (Mint, apt + systemd) o cache do APT e o
    /// journal são medidos e o comando nunca é executado pelo Pulse.
    #[test]
    fn real_guided_insights_measure_system_areas() {
        let insights = guided_insights();
        for insight in &insights {
            eprintln!(
                "{}: {:?} {:?}",
                insight.id, insight.size_bytes, insight.unavailable_reason
            );
            assert!(insight.size_bytes.is_some() != insight.unavailable_reason.is_some());
            assert!(insight.command.starts_with("# Terminal"));
        }
        if Path::new("/var/cache/apt/archives").is_dir() {
            assert!(insights.iter().any(|i| i.id == "apt-cache"));
        }
    }

    /// A limpeza de `user-temp` num diretório falso preserva o recente, o
    /// aberto e o socket, e remove só o antigo.
    #[test]
    fn temp_cleanup_on_fake_root_respects_age_and_in_use() {
        use super::super::{remove_entry, CleanupResult};
        let root = env::temp_dir().join(format!("pulse-temp-clean-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("velha")).unwrap();
        let old_file = root.join("velha/antigo.tmp");
        let recent_file = root.join("recente.tmp");
        let open_file = root.join("aberto.tmp");
        fs::write(&old_file, b"12345").unwrap();
        fs::write(&recent_file, b"1").unwrap();
        fs::write(&open_file, b"1").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
        for path in [&old_file, &open_file, &root.join("velha")] {
            fs::File::options()
                .write(true)
                .open(path)
                .or_else(|_| fs::File::open(path))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let _socket = std::os::unix::net::UnixListener::bind(root.join("s.sock")).unwrap();
        let _held = fs::File::open(&open_file).unwrap();

        let snapshot = InUseFiles::snapshot();
        let guard = EntryGuard::new("user-temp", &root, Some(&snapshot));
        let mut result = CleanupResult {
            reclaimed_bytes: 0,
            deleted_files: 0,
            deleted_directories: 0,
            skipped_items: 0,
        };
        let cutoff = SystemTime::now().checked_sub(Duration::from_secs(24 * 60 * 60));
        let mut visited = 0;
        for entry in fs::read_dir(&root).unwrap().flatten() {
            remove_entry(&entry.path(), &mut result, &mut visited, cutoff, &guard);
        }
        assert!(!old_file.exists());
        assert!(recent_file.exists());
        assert!(open_file.exists());
        assert!(root.join("s.sock").exists());
        assert_eq!(result.deleted_files, 1);
        assert_eq!(result.reclaimed_bytes, 5);
        assert_eq!(
            result.skipped_items, 1,
            "o arquivo aberto conta como em uso"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
