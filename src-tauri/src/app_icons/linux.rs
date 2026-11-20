//! Ícones e identidade de aplicativos no Linux (padrões freedesktop.org).
//!
//! - Entradas `.desktop` de `~/.local/share/applications`, `$XDG_DATA_DIRS`,
//!   exports do Flatpak e do Snap formam um índice (TTL de 5 min) que liga um
//!   processo ao aplicativo: id do Flatpak, caminho/nome do executável do
//!   `Exec`/`TryExec`, pasta de instalação e `StartupWMClass`.
//! - O `Icon=` é resolvido no tema atual (KDE `kdeglobals` ou GTK
//!   `settings.ini`), nos temas herdados e em `hicolor`, depois em `pixmaps`.
//!   PNG e SVG viram data URI (mesmo formato do Windows); XPM não é suportado.
//! - Tudo é cacheado, inclusive as falhas: chamar em polling é barato.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const INDEX_TTL: Duration = Duration::from_secs(300);
/// Ícone acima disso pesa demais no IPC a cada coleta; tenta o próximo tamanho.
const MAX_ICON_BYTES: u64 = 96 * 1024;
const MAX_DESKTOP_BYTES: u64 = 256 * 1024;
const ICON_SIZES: [&str; 9] = [
    "32", "48", "64", "scalable", "24", "96", "128", "256", "512",
];

/// Programas que só executam outro código: indexar pelo nome deles faria todo
/// `python3` herdar o ícone do primeiro app escrito em Python.
const LAUNCHERS: [&str; 22] = [
    "env", "sh", "bash", "dash", "zsh", "python", "python2", "python3", "perl", "ruby", "node",
    "java", "mono", "wine", "gjs", "flatpak", "snap", "pkexec", "sudo", "electron", "qmlscene",
    "xdg-open",
];

fn is_launcher(name: &str) -> bool {
    let lower = name.to_lowercase();
    LAUNCHERS.contains(&lower.as_str())
        || (lower.starts_with("python")
            && lower[6..].chars().all(|c| c.is_ascii_digit() || c == '.'))
        || lower.starts_with("electron")
}

/// Pastas onde o executável real costuma ficar junto de bibliotecas do
/// sistema inteiro: não identificam um aplicativo.
const GENERIC_DIRS: [&str; 12] = [
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/games",
    "/usr/libexec",
    "/usr/lib",
    "/usr/lib64",
    "/usr/lib/x86_64-linux-gnu",
    "/opt",
];

/// Uma entrada `.desktop` de aplicativo.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DesktopApp {
    /// Id freedesktop sem `.desktop` (`org.kde.dolphin`, `firefox_firefox`).
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub exec: Option<String>,
    pub try_exec: Option<String>,
    pub wm_class: Option<String>,
    pub no_display: bool,
}

/// Interpreta o grupo `[Desktop Entry]`. `None` para entradas ocultas, que
/// não são `Application` ou sem nome.
pub fn parse_desktop_entry(id: &str, text: &str) -> Option<DesktopApp> {
    let mut in_entry = false;
    let mut app = DesktopApp {
        id: id.to_owned(),
        ..DesktopApp::default()
    };
    let mut name = None;
    let mut name_pt = None;
    let mut name_pt_br = None;
    let mut kind = None;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "Type" => kind = Some(value),
            "Name" => name = Some(value),
            "Name[pt_BR]" => name_pt_br = Some(value),
            "Name[pt]" => name_pt = Some(value),
            "Icon" if !value.is_empty() => app.icon = Some(value),
            "Exec" if !value.is_empty() => app.exec = Some(value),
            "TryExec" if !value.is_empty() => app.try_exec = Some(value),
            "StartupWMClass" if !value.is_empty() => app.wm_class = Some(value),
            "NoDisplay" => app.no_display = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    if hidden || kind.as_deref() != Some("Application") {
        return None;
    }
    app.name = name_pt_br.or(name_pt).or(name).filter(|n| !n.is_empty())?;
    Some(app)
}

/// Divide uma linha `Exec` em argumentos (aspas duplas e barra invertida),
/// descartando os códigos de campo (`%f`, `%U`…).
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    has_token = true;
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args.retain(|arg| !(arg.len() == 2 && arg.starts_with('%')));
    args
}

/// Programa efetivo de uma linha `Exec`, pulando `env VAR=valor`.
/// `None` quando o programa é um lançador genérico (`python3 app.py`).
pub fn exec_program(exec: &str) -> Option<String> {
    let args = split_exec(exec);
    let mut iter = args.into_iter().peekable();
    if iter
        .peek()
        .is_some_and(|first| first == "env" || first.ends_with("/env"))
    {
        iter.next();
    }
    let program = iter.find(|arg| !(arg.contains('=') && !arg.starts_with('/')))?;
    let base = Path::new(&program)
        .file_name()?
        .to_string_lossy()
        .into_owned();
    (!is_launcher(&base)).then_some(program)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// `$XDG_DATA_HOME` + `$XDG_DATA_DIRS` + exports de Flatpak/Snap, sem repetição.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = home_dir();
    match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dirs.push(dir),
        _ => {
            if let Some(home) = &home {
                dirs.push(home.join(".local/share"));
            }
        }
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(
        system
            .split(':')
            .filter(|d| d.starts_with('/'))
            .map(PathBuf::from),
    );
    if let Some(home) = &home {
        dirs.push(home.join(".local/share/flatpak/exports/share"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop"));
    dirs.push(PathBuf::from("/usr/share"));
    let mut unique: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

fn resolve_in_path(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    if path.components().count() != 1 {
        return None;
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(path))
        .find(|candidate| candidate.is_file())
}

#[derive(Default)]
struct DesktopIndex {
    apps: Vec<DesktopApp>,
    by_id: HashMap<String, usize>,
    by_exec_path: HashMap<PathBuf, usize>,
    by_exec_name: HashMap<String, usize>,
    by_exec_dir: HashMap<PathBuf, usize>,
    by_wm_class: HashMap<String, usize>,
    icon_dirs: Vec<PathBuf>,
}

impl DesktopIndex {
    fn add(&mut self, app: DesktopApp) {
        let index = self.apps.len();
        let id = app.id.to_lowercase();
        self.by_id.entry(id.clone()).or_insert(index);
        // `org.kde.dolphin` também responde por `dolphin`.
        if let Some(tail) = id.rsplit('.').next().filter(|tail| *tail != id) {
            self.by_id.entry(tail.to_owned()).or_insert(index);
        }
        if let Some(class) = &app.wm_class {
            self.by_wm_class
                .entry(class.to_lowercase())
                .or_insert(index);
        }
        let programs = [
            app.exec.as_deref().and_then(exec_program),
            app.try_exec.as_deref().and_then(exec_program),
        ];
        for program in programs.into_iter().flatten() {
            if let Some(name) = Path::new(&program).file_name() {
                self.by_exec_name
                    .entry(name.to_string_lossy().to_lowercase())
                    .or_insert(index);
            }
            let Some(resolved) = resolve_in_path(&program) else {
                continue;
            };
            self.by_exec_path.entry(resolved.clone()).or_insert(index);
            let Ok(canonical) = std::fs::canonicalize(&resolved) else {
                continue;
            };
            if let Some(name) = canonical.file_name() {
                let name = name.to_string_lossy().to_lowercase();
                if !is_launcher(&name) {
                    self.by_exec_name.entry(name).or_insert(index);
                }
            }
            if let Some(parent) = canonical.parent() {
                if !GENERIC_DIRS.iter().any(|dir| parent == Path::new(dir)) {
                    self.by_exec_dir
                        .entry(parent.to_path_buf())
                        .or_insert(index);
                }
            }
            self.by_exec_path.entry(canonical).or_insert(index);
        }
        self.apps.push(app);
    }
}

fn collect_desktop_files(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() && depth > 0 {
            collect_desktop_files(&entry.path(), &format!("{prefix}{name}-"), depth - 1, out);
        } else if let Some(stem) = name.strip_suffix(".desktop") {
            // Links simbólicos são comuns (exports do Flatpak) e seguros aqui.
            out.push((format!("{prefix}{stem}"), entry.path()));
        }
    }
}

/// Tema de ícones atual: KDE (`kdeglobals`) e depois GTK 3/4.
fn current_icon_theme() -> Option<String> {
    let home = home_dir()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let read_key = |file: PathBuf, group: &str, key: &str| -> Option<String> {
        let text = std::fs::read_to_string(file).ok()?;
        let mut in_group = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_group = line == group;
            } else if in_group {
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == key && !v.trim().is_empty() {
                        return Some(v.trim().to_owned());
                    }
                }
            }
        }
        None
    };
    read_key(config.join("kdeglobals"), "[Icons]", "Theme")
        .or_else(|| {
            read_key(
                config.join("gtk-4.0/settings.ini"),
                "[Settings]",
                "gtk-icon-theme-name",
            )
        })
        .or_else(|| {
            read_key(
                config.join("gtk-3.0/settings.ini"),
                "[Settings]",
                "gtk-icon-theme-name",
            )
        })
}

/// Interpreta `Inherits=` de um `index.theme`.
pub fn parse_theme_inherits(text: &str) -> Vec<String> {
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Icon Theme]";
        } else if in_group {
            if let Some(value) = line.strip_prefix("Inherits=") {
                return value
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        }
    }
    Vec::new()
}

fn icon_bases(data: &[PathBuf]) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(home) = home_dir() {
        bases.push(home.join(".icons"));
    }
    bases.extend(data.iter().map(|dir| dir.join("icons")));
    bases.retain(|base| base.is_dir());
    bases
}

fn theme_chain(bases: &[PathBuf]) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut pending: Vec<String> = current_icon_theme().into_iter().collect();
    while let Some(theme) = pending.pop() {
        if chain.contains(&theme)
            || chain.len() >= 6
            || theme.contains('/')
            || theme.starts_with('.')
        {
            continue;
        }
        let inherits = bases
            .iter()
            .find_map(|base| std::fs::read_to_string(base.join(&theme).join("index.theme")).ok())
            .map(|text| parse_theme_inherits(&text))
            .unwrap_or_default();
        chain.push(theme);
        // Ordem de herança preservada: o primeiro herdado é consultado antes.
        pending.extend(inherits.into_iter().rev());
    }
    chain.retain(|theme| theme != "hicolor");
    chain.push("hicolor".to_owned());
    chain
}

fn build_icon_dirs(data: &[PathBuf]) -> Vec<PathBuf> {
    let bases = icon_bases(data);
    let mut dirs = Vec::new();
    for theme in theme_chain(&bases) {
        for base in &bases {
            let root = base.join(&theme);
            if !root.is_dir() {
                continue;
            }
            for size in ICON_SIZES {
                let candidates = if size == "scalable" {
                    vec![root.join("scalable/apps"), root.join("apps/scalable")]
                } else {
                    vec![
                        root.join(format!("{size}x{size}/apps")),
                        root.join(format!("apps/{size}")),
                        root.join(format!("{size}/apps")),
                    ]
                };
                dirs.extend(candidates.into_iter().filter(|dir| dir.is_dir()));
            }
        }
    }
    for dir in data {
        let pixmaps = dir.join("pixmaps");
        if pixmaps.is_dir() && !dirs.contains(&pixmaps) {
            dirs.push(pixmaps);
        }
    }
    dirs
}

fn build_index() -> DesktopIndex {
    let data = data_dirs();
    let mut files = Vec::new();
    for dir in &data {
        collect_desktop_files(&dir.join("applications"), "", 2, &mut files);
    }
    let mut parsed: Vec<DesktopApp> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    for (id, path) in files {
        // O primeiro diretório tem precedência (usuário antes do sistema).
        if seen_ids.contains(&id) {
            continue;
        }
        seen_ids.push(id.clone());
        let too_big = std::fs::metadata(&path)
            .map(|m| m.len() > MAX_DESKTOP_BYTES)
            .unwrap_or(true);
        if too_big {
            continue;
        }
        if let Some(app) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| parse_desktop_entry(&id, &text))
        {
            parsed.push(app);
        }
    }
    let mut index = DesktopIndex::default();
    // Entradas visíveis ganham das auxiliares (`NoDisplay=true`).
    let (visible, hidden): (Vec<_>, Vec<_>) = parsed.into_iter().partition(|app| !app.no_display);
    for app in visible.into_iter().chain(hidden) {
        index.add(app);
    }
    index.icon_dirs = build_icon_dirs(&data);
    index
}

struct Caches {
    index: Option<(Instant, Arc<DesktopIndex>)>,
    /// Data URI por chave (`icon:<nome>`, `exe:<caminho>`, `proc:<…>`).
    icons: Option<HashMap<String, Option<String>>>,
}

static CACHES: Mutex<Caches> = Mutex::new(Caches {
    index: None,
    icons: None,
});

fn index() -> Arc<DesktopIndex> {
    if let Ok(mut caches) = CACHES.lock() {
        if let Some((built, index)) = &caches.index {
            if built.elapsed() < INDEX_TTL {
                return index.clone();
            }
        }
        let fresh = Arc::new(build_index());
        caches.index = Some((Instant::now(), fresh.clone()));
        // Apps instalados/removidos mudam ícones: o cache acompanha o índice.
        caches.icons = None;
        return fresh;
    }
    Arc::new(build_index())
}

fn cached(key: &str, compute: impl FnOnce() -> Option<String>) -> Option<String> {
    if let Ok(caches) = CACHES.lock() {
        if let Some(hit) = caches.icons.as_ref().and_then(|icons| icons.get(key)) {
            return hit.clone();
        }
    }
    let value = compute();
    if let Ok(mut caches) = CACHES.lock() {
        caches
            .icons
            .get_or_insert_with(HashMap::new)
            .insert(key.to_owned(), value.clone());
    }
    value
}

/// Lê um PNG/SVG pequeno e devolve o data URI. Valida a assinatura para não
/// embutir um arquivo qualquer com extensão trocada.
pub fn file_data_uri(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    let size = std::fs::metadata(path).ok()?.len();
    if size == 0 || size > MAX_ICON_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mime = match extension.as_str() {
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
        "svg" if String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).contains("<svg") => {
            "image/svg+xml"
        }
        _ => return None,
    };
    Some(format!("data:{mime};base64,{}", BASE64.encode(&bytes)))
}

fn lookup_icon(index: &DesktopIndex, icon: &str) -> Option<String> {
    let icon = icon.trim();
    if icon.is_empty() {
        return None;
    }
    let as_path = Path::new(icon);
    if as_path.is_absolute() {
        return file_data_uri(as_path);
    }
    if icon.contains('/') || icon.starts_with('.') {
        return None;
    }
    // `Icon=foo.png` é tolerado pela especificação em entradas antigas.
    let name = icon
        .strip_suffix(".png")
        .or_else(|| icon.strip_suffix(".svg"))
        .unwrap_or(icon);
    index.icon_dirs.iter().find_map(|dir| {
        ["png", "svg"]
            .iter()
            .find_map(|ext| file_data_uri(&dir.join(format!("{name}.{ext}"))))
    })
}

/// Data URI de um nome de ícone do tema (ou caminho absoluto de imagem).
pub fn icon_data_uri(icon: &str) -> Option<String> {
    let index = index();
    cached(&format!("icon:{icon}"), || lookup_icon(&index, icon))
}

/// Id do Flatpak dono do processo (`/proc/<pid>/root/.flatpak-info`).
pub fn flatpak_app_id(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let text = std::fs::read_to_string(format!("/proc/{pid}/root/.flatpak-info")).ok()?;
    parse_flatpak_info(&text)
}

pub fn parse_flatpak_info(text: &str) -> Option<String> {
    let mut in_app = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_app = line == "[Application]";
        } else if in_app {
            if let Some(name) = line.strip_prefix("name=") {
                let name = name.trim();
                return (!name.is_empty()).then(|| name.to_owned());
            }
        }
    }
    None
}

/// Nome do snap quando o executável vem de `/snap/<nome>/…`.
pub fn snap_name(exe: &str) -> Option<String> {
    let rest = exe.strip_prefix("/snap/")?;
    let name = rest.split('/').next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

fn match_app<'a>(
    index: &'a DesktopIndex,
    flatpak: Option<&str>,
    exe: Option<&str>,
    name: &str,
) -> Option<&'a DesktopApp> {
    let by_id = |id: &str| index.by_id.get(&id.to_lowercase()).copied();
    let found = flatpak
        .and_then(by_id)
        .or_else(|| {
            let snap = snap_name(exe?)?;
            by_id(&format!("{snap}_{snap}")).or_else(|| by_id(&snap))
        })
        .or_else(|| {
            let exe = Path::new(exe?);
            index.by_exec_path.get(exe).copied().or_else(|| {
                let canonical = std::fs::canonicalize(exe).ok()?;
                index.by_exec_path.get(&canonical).copied()
            })
        })
        .or_else(|| {
            let base = Path::new(exe?)
                .file_name()?
                .to_string_lossy()
                .to_lowercase();
            if is_launcher(&base) {
                return None;
            }
            index.by_exec_name.get(&base).copied()
        })
        .or_else(|| {
            let parent = Path::new(exe?).parent()?;
            index.by_exec_dir.get(parent).copied()
        })
        .or_else(|| {
            let lower = name.to_lowercase();
            if is_launcher(&lower) {
                return None;
            }
            index
                .by_wm_class
                .get(&lower)
                .copied()
                .or_else(|| by_id(&lower))
        })?;
    index.apps.get(found)
}

/// Aplicativo `.desktop` que corresponde ao processo, se houver.
pub fn desktop_app_for_process(pid: u32, exe: Option<&str>, name: &str) -> Option<DesktopApp> {
    let index = index();
    let flatpak = flatpak_app_id(pid);
    match_app(&index, flatpak.as_deref(), exe, name).cloned()
}

/// Ícone do processo: aplicativo `.desktop` correspondente; sem ele, um
/// ícone do tema com o nome do executável (ex.: `htop`).
pub fn icon_for_process(pid: u32, exe: Option<&str>, name: &str) -> Option<String> {
    let flatpak = flatpak_app_id(pid);
    let key = format!(
        "proc:{}|{}|{}",
        flatpak.as_deref().unwrap_or_default(),
        exe.unwrap_or_default(),
        name
    );
    let index = index();
    cached(&key, || {
        if let Some(app) = match_app(&index, flatpak.as_deref(), exe, name) {
            if let Some(uri) = app
                .icon
                .as_deref()
                .and_then(|icon| lookup_icon(&index, icon))
            {
                return Some(uri);
            }
        }
        let base = exe
            .and_then(|exe| Path::new(exe).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_owned());
        if is_launcher(&base) {
            return None;
        }
        lookup_icon(&index, &base.to_lowercase())
    })
}

/// Extrai o id do aplicativo de uma linha `flatpak run [opções] <ref>`.
pub fn flatpak_run_app_id(args: &[String]) -> Option<String> {
    let mut iter = args.iter().peekable();
    if iter
        .peek()
        .is_some_and(|first| *first == "env" || first.ends_with("/env"))
    {
        iter.next();
        while iter
            .peek()
            .is_some_and(|arg| arg.contains('=') && !arg.starts_with('/'))
        {
            iter.next();
        }
    }
    let program = iter.next()?;
    let base = Path::new(program).file_name()?.to_string_lossy();
    if !base.eq_ignore_ascii_case("flatpak") {
        return None;
    }
    if iter.next().map(String::as_str) != Some("run") {
        return None;
    }
    let mut skip_next = false;
    for arg in iter {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--" {
            continue;
        }
        if arg.starts_with('-') {
            if !arg.contains('=')
                && matches!(
                    arg.as_str(),
                    "--branch"
                        | "--arch"
                        | "--command"
                        | "--cwd"
                        | "--runtime"
                        | "--runtime-version"
                        | "--commit"
                        | "--app-path"
                        | "--usr-path"
                )
            {
                skip_next = true;
            }
            continue;
        }
        let app_ref = arg.strip_prefix("app/").unwrap_or(arg);
        let id = app_ref.split('/').next().unwrap_or("");
        return (!id.is_empty() && id.contains('.')).then(|| id.to_owned());
    }
    None
}

/// Ícone de um executável, arquivo `.desktop` ou imagem (`.png`/`.svg`) do host.
pub fn icon_for_executable(path: &Path) -> Option<String> {
    let text = path.to_string_lossy().into_owned();
    let ext = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase());
    if matches!(ext.as_deref(), Some("png" | "svg")) {
        return cached(&format!("file:{text}"), || file_data_uri(path));
    }
    if ext.as_deref() == Some("desktop") {
        let index = index();
        return cached(&format!("file:{text}"), || {
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let app = parse_desktop_entry(&id, &std::fs::read_to_string(path).ok()?)?;
            lookup_icon(&index, app.icon.as_deref()?)
        });
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    icon_for_process(0, Some(&text), &name)
}

/// Último recurso de `icon_for_command`: `flatpak run`, `env VAR=1 …`,
/// arquivo `.desktop`, id de aplicativo ou nome de ícone do tema.
pub fn icon_for_command_token(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let args = split_exec(trimmed);
    let token = args.first()?.clone();
    let index = index();
    cached(&format!("cmd:{trimmed}"), || {
        if let Some(app_id) = flatpak_run_app_id(&args) {
            let lower = app_id.to_lowercase();
            if let Some(app) = index.by_id.get(&lower).and_then(|i| index.apps.get(*i)) {
                if let Some(icon) = app.icon.as_deref() {
                    if let Some(uri) = lookup_icon(&index, icon) {
                        return Some(uri);
                    }
                }
            }
            return lookup_icon(&index, &app_id);
        }
        if let Some(program) = exec_program(trimmed) {
            if let Some(resolved) = resolve_in_path(&program) {
                let text = resolved.to_string_lossy().into_owned();
                let name = resolved
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Some(app) = match_app(&index, None, Some(&text), &name) {
                    if let Some(uri) = app
                        .icon
                        .as_deref()
                        .and_then(|icon| lookup_icon(&index, icon))
                    {
                        return Some(uri);
                    }
                }
                if let Some(uri) = lookup_icon(&index, &name.to_lowercase()) {
                    return Some(uri);
                }
            }
            if let Some(base) = Path::new(&program)
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
            {
                if let Some(app) = index
                    .by_exec_name
                    .get(&base)
                    .or_else(|| index.by_id.get(&base))
                    .and_then(|i| index.apps.get(*i))
                {
                    if let Some(uri) = app
                        .icon
                        .as_deref()
                        .and_then(|icon| lookup_icon(&index, icon))
                    {
                        return Some(uri);
                    }
                }
                if let Some(uri) = lookup_icon(&index, &base) {
                    return Some(uri);
                }
            }
        }
        let path = Path::new(&token);
        if token.ends_with(".desktop") && path.is_absolute() {
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let app = parse_desktop_entry(&id, &std::fs::read_to_string(path).ok()?)?;
            return lookup_icon(&index, app.icon.as_deref()?);
        }
        if path.is_absolute() {
            return file_data_uri(path);
        }
        let id = token.trim_end_matches(".desktop").to_lowercase();
        if let Some(app) = index.by_id.get(&id).and_then(|i| index.apps.get(*i)) {
            if let Some(icon) = app.icon.as_deref() {
                return lookup_icon(&index, icon);
            }
        }
        if is_launcher(&id) {
            return None;
        }
        lookup_icon(&index, &id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreta_entrada_desktop_com_nome_em_portugues() {
        let text = "[Desktop Entry]\nType=Application\nName=Files\nName[pt_BR]=Arquivos\nIcon=org.gnome.Nautilus\nExec=nautilus --new-window %U\nStartupWMClass=org.gnome.Nautilus\n\n[Desktop Action new]\nName=Nova\nExec=nautilus --other\n";
        let app = parse_desktop_entry("org.gnome.Nautilus", text).unwrap();
        assert_eq!(app.name, "Arquivos");
        assert_eq!(app.icon.as_deref(), Some("org.gnome.Nautilus"));
        assert_eq!(app.exec.as_deref(), Some("nautilus --new-window %U"));
        assert_eq!(app.wm_class.as_deref(), Some("org.gnome.Nautilus"));
    }

    #[test]
    fn ignora_entradas_ocultas_e_nao_aplicativos() {
        assert!(parse_desktop_entry("x", "[Desktop Entry]\nType=Link\nName=X\n").is_none());
        assert!(parse_desktop_entry(
            "x",
            "[Desktop Entry]\nType=Application\nName=X\nHidden=true\n"
        )
        .is_none());
        assert!(parse_desktop_entry("x", "[Desktop Entry]\nType=Application\n").is_none());
    }

    #[test]
    fn divide_exec_com_aspas_e_codigos_de_campo() {
        assert_eq!(
            split_exec("\"/opt/My App/app\" --flag %F"),
            vec!["/opt/My App/app".to_owned(), "--flag".to_owned()]
        );
        assert_eq!(
            split_exec("env FOO=1 code %U"),
            vec!["env", "FOO=1", "code"]
        );
    }

    #[test]
    fn programa_do_exec_pula_env_e_recusa_lancadores() {
        assert_eq!(
            exec_program("env FOO=1 BAR=2 code %U").as_deref(),
            Some("code")
        );
        assert_eq!(
            exec_program("/usr/bin/firefox %u").as_deref(),
            Some("/usr/bin/firefox")
        );
        assert_eq!(exec_program("python3 /usr/share/app/main.py"), None);
        assert_eq!(
            exec_program("/usr/bin/flatpak run --branch=stable com.example.App"),
            None
        );
    }

    #[test]
    fn le_heranca_de_tema() {
        let text = "[Icon Theme]\nName=Mint-Y\nInherits=Adwaita,hicolor\n\n[16x16/apps]\nSize=16\n";
        assert_eq!(parse_theme_inherits(text), vec!["Adwaita", "hicolor"]);
    }

    #[test]
    fn le_id_do_flatpak_e_nome_do_snap() {
        let info = "[Application]\nname=com.rtosta.zapzap\nruntime=runtime/org.kde.Platform/x86_64/6.7\n\n[Instance]\ninstance-id=1\n";
        assert_eq!(
            parse_flatpak_info(info).as_deref(),
            Some("com.rtosta.zapzap")
        );
        assert_eq!(
            snap_name("/snap/firefox/4793/usr/lib/firefox/firefox").as_deref(),
            Some("firefox")
        );
        assert_eq!(snap_name("/usr/bin/firefox"), None);
    }

    #[test]
    fn indice_casa_processo_por_id_executavel_pasta_e_classe() {
        let mut index = DesktopIndex::default();
        index.add(DesktopApp {
            id: "org.kde.dolphin".into(),
            name: "Dolphin".into(),
            icon: Some("system-file-manager".into()),
            exec: Some("dolphin %u".into()),
            ..DesktopApp::default()
        });
        index.add(DesktopApp {
            id: "com.rtosta.zapzap".into(),
            name: "ZapZap".into(),
            exec: Some("/usr/bin/flatpak run com.rtosta.zapzap".into()),
            ..DesktopApp::default()
        });
        index.add(DesktopApp {
            id: "chat".into(),
            name: "Chat".into(),
            wm_class: Some("ChatApp".into()),
            ..DesktopApp::default()
        });
        let find =
            |flatpak, exe, name| match_app(&index, flatpak, exe, name).map(|app| app.name.clone());
        assert_eq!(
            find(None, Some("/usr/bin/dolphin"), "dolphin").as_deref(),
            Some("Dolphin")
        );
        assert_eq!(
            find(Some("com.rtosta.zapzap"), Some("/app/bin/zapzap"), "zapzap").as_deref(),
            Some("ZapZap")
        );
        assert_eq!(find(None, None, "chatapp").as_deref(), Some("Chat"));
        // Interpretadores nunca herdam identidade de um app.
        assert_eq!(find(None, Some("/usr/bin/python3"), "python3"), None);
    }

    #[test]
    fn data_uri_valida_assinatura_e_tamanho() {
        let dir = std::env::temp_dir().join(format!("pulse-icon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.png");
        std::fs::write(&fake, b"nao sou png").unwrap();
        assert!(file_data_uri(&fake).is_none());
        let svg = dir.join("ok.svg");
        std::fs::write(
            &svg,
            b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        )
        .unwrap();
        assert!(file_data_uri(&svg)
            .unwrap()
            .starts_with("data:image/svg+xml;base64,"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extrai_id_de_comando_flatpak_run() {
        assert_eq!(
            flatpak_run_app_id(&split_exec(
                "/usr/bin/flatpak run --branch=stable --arch=x86_64 com.rtosta.zapzap @@u %U @@"
            ))
            .as_deref(),
            Some("com.rtosta.zapzap")
        );
        assert_eq!(
            flatpak_run_app_id(&split_exec(
                "env FOO=1 flatpak run --branch stable app/org.mozilla.firefox/x86_64/stable"
            ))
            .as_deref(),
            Some("org.mozilla.firefox")
        );
        assert_eq!(flatpak_run_app_id(&split_exec("flatpak list")), None);
        assert_eq!(flatpak_run_app_id(&split_exec("/usr/bin/firefox")), None);
    }

    #[test]
    fn indice_real_resolve_algum_icone() {
        // Smoke test na máquina: com aplicativos instalados, ao menos um
        // ícone de aplicativo precisa resolver para data URI.
        let index = build_index();
        if index.apps.is_empty() {
            return;
        }
        let resolved = index
            .apps
            .iter()
            .filter_map(|app| app.icon.as_deref())
            .take(40)
            .filter_map(|icon| lookup_icon(&index, icon))
            .count();
        assert!(
            resolved > 0,
            "nenhum ícone resolvido entre {} apps",
            index.apps.len()
        );
    }
}
