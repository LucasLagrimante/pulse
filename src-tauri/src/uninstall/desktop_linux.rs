//! Entradas `.desktop` (freedesktop) e ícones de tema.
//!
//! O que conta como "aplicativo para o usuário" no Linux é o que o menu do
//! ambiente mostra: `Type=Application` sem `NoDisplay=true`/`Hidden=true`.
//! É esse o filtro que separa os ~150 programas das ~3000 bibliotecas do dpkg.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;

/// `.desktop` maior que isso não é atalho, é lixo: não lemos.
const MAX_DESKTOP_BYTES: u64 = 256 * 1024;
const MAX_DESKTOP_FILES: usize = 4000;
/// Ícone embutido no IPC: acima disso o inventário pesaria megabytes.
const MAX_ICON_BYTES: u64 = 96 * 1024;
/// Tamanhos pequenos primeiro: a lista mostra 32 px.
const ICON_SIZES: &[&str] = &[
    "48x48", "64x64", "32x32", "96x96", "128x128", "scalable", "256x256",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesktopEntry {
    pub path: PathBuf,
    /// Nome do arquivo sem `.desktop` (`org.kde.dolphin`).
    pub id: String,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub exec: Option<String>,
    pub try_exec: Option<String>,
    pub entry_type: Option<String>,
    pub no_display: bool,
    pub hidden: bool,
}

impl DesktopEntry {
    pub fn is_visible_app(&self) -> bool {
        self.entry_type.as_deref() == Some("Application")
            && !self.no_display
            && !self.hidden
            && self
                .name
                .as_deref()
                .is_some_and(|name| !name.trim().is_empty())
    }
}

/// `pt_BR.UTF-8@euro` → `["pt_BR", "pt"]`, na ordem de preferência da spec.
pub fn locale_preferences() -> Vec<String> {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|variable| std::env::var(variable).ok())
        .find(|value| !value.trim().is_empty() && value != "C" && value != "POSIX")
        .unwrap_or_default();
    locale_variants(&raw)
}

pub fn locale_variants(raw: &str) -> Vec<String> {
    let base = raw
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if base.is_empty() {
        return Vec::new();
    }
    let mut variants = vec![base.clone()];
    if let Some((language, _)) = base.split_once('_') {
        variants.push(language.to_string());
    }
    variants
}

fn parse_bool(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true")
}

/// Lê só o grupo `[Desktop Entry]`; ações (`[Desktop Action x]`) têm `Name`
/// próprio e não podem sobrescrever o do app.
pub fn parse_desktop_entry(text: &str, locales: &[String]) -> DesktopEntry {
    let mut entry = DesktopEntry::default();
    let mut in_main = false;
    // Rank do nome escolhido: 0 = localizado mais preferido; usize::MAX = nenhum.
    let mut name_rank = usize::MAX;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().to_string();
        match key {
            "Type" => entry.entry_type = Some(value),
            "Icon" if !value.is_empty() => entry.icon = Some(value),
            "Exec" if !value.is_empty() => entry.exec = Some(value),
            "TryExec" if !value.is_empty() => entry.try_exec = Some(value),
            "NoDisplay" => entry.no_display = parse_bool(&value),
            "Hidden" => entry.hidden = parse_bool(&value),
            "Name" => {
                let rank = locales.len();
                if rank <= name_rank && !value.is_empty() {
                    entry.name = Some(value);
                    name_rank = rank;
                }
            }
            _ => {
                if let Some(locale) = key
                    .strip_prefix("Name[")
                    .and_then(|rest| rest.strip_suffix(']'))
                {
                    if let Some(rank) = locales.iter().position(|item| item == locale) {
                        if rank < name_rank && !value.is_empty() {
                            entry.name = Some(value);
                            name_rank = rank;
                        }
                    }
                }
            }
        }
    }
    entry
}

pub fn read_desktop_file(path: &Path, locales: &[String]) -> Option<DesktopEntry> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DESKTOP_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let mut entry = parse_desktop_entry(&String::from_utf8_lossy(&bytes), locales);
    entry.path = path.to_path_buf();
    entry.id = path
        .file_name()?
        .to_string_lossy()
        .strip_suffix(".desktop")?
        .to_string();
    Some(entry)
}

/// Todos os `.desktop` de uma pasta (sem recursão), visíveis ou não.
pub fn read_desktop_dir(dir: &Path, locales: &[String]) -> Vec<DesktopEntry> {
    let Ok(iter) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<DesktopEntry> = iter
        .flatten()
        .take(MAX_DESKTOP_FILES)
        .map(|item| item.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
        .filter_map(|path| read_desktop_file(&path, locales))
        .collect();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    entries
}

/// Primeiro token de `Exec`, com as regras de aspas da spec (`"..."` agrupa,
/// `\"`, `\\`, `` \` `` e `\$` são escapes dentro das aspas).
pub fn exec_program(exec: &str) -> Option<String> {
    let mut chars = exec.trim().chars().peekable();
    let mut token = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
        while let Some(character) = chars.next() {
            match character {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        token.push(escaped);
                    }
                }
                '"' => return (!token.is_empty()).then_some(token),
                other => token.push(other),
            }
        }
        // Aspas sem fechamento: entrada inválida.
        return None;
    }
    for character in chars {
        if character.is_whitespace() {
            break;
        }
        token.push(character);
    }
    (!token.is_empty()).then_some(token)
}

fn mime_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Data URI do arquivo de ícone (PNG ou SVG). XPM/SVGZ e arquivos grandes
/// ficam sem ícone: a UI mostra o monograma, nunca uma imagem inventada.
pub fn icon_data_uri(path: &Path) -> Option<String> {
    let mime = mime_for(path)?;
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_ICON_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    Some(format!(
        "data:{};base64,{}",
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Raízes de ícone na ordem de busca: usuário, Flatpak, sistema.
pub fn icon_roots(home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(home.join(".local/share/icons"));
        roots.push(home.join(".local/share/flatpak/exports/share/icons"));
    }
    roots.push(PathBuf::from("/var/lib/flatpak/exports/share/icons"));
    roots.push(PathBuf::from("/usr/local/share/icons"));
    roots.push(PathBuf::from("/usr/share/icons"));
    roots
}

/// Resolve `Icon=` para um data URI: caminho absoluto direto, senão nome no
/// tema `hicolor` (onde todo app é obrigado a instalar o ícone) e
/// `/usr/share/pixmaps`. Só confere existência — nenhum varrimento de tema.
pub fn resolve_icon(icon: &str, roots: &[PathBuf]) -> Option<String> {
    let icon = icon.trim();
    if icon.is_empty() {
        return None;
    }
    if icon.starts_with('/') {
        return icon_data_uri(Path::new(icon));
    }
    // Nome com extensão (`app.png`) é aceito pela maioria dos menus.
    let stem = icon
        .strip_suffix(".png")
        .or_else(|| icon.strip_suffix(".svg"))
        .unwrap_or(icon);
    if stem.contains('/') || stem.contains("..") {
        return None;
    }
    for root in roots {
        for size in ICON_SIZES {
            let dir = root.join("hicolor").join(size).join("apps");
            for extension in ["png", "svg"] {
                if let Some(uri) = icon_data_uri(&dir.join(format!("{stem}.{extension}"))) {
                    return Some(uri);
                }
            }
        }
    }
    for extension in ["png", "svg"] {
        if let Some(uri) =
            icon_data_uri(&Path::new("/usr/share/pixmaps").join(format!("{stem}.{extension}")))
        {
            return Some(uri);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
[Desktop Entry]
Type=Application
Name=Files
Name[pt_BR]=Arquivos
Name[pt]=Ficheiros
Exec=\"/opt/My App/bin/app\" %U
Icon=org.example.App
NoDisplay=false

[Desktop Action new-window]
Name=New Window
Name[pt_BR]=Nova janela
Exec=app --new-window
";

    #[test]
    fn desktop_usa_nome_localizado_e_ignora_acoes() {
        let locales = locale_variants("pt_BR.UTF-8");
        let entry = parse_desktop_entry(FIXTURE, &locales);
        assert_eq!(entry.name.as_deref(), Some("Arquivos"));
        assert_eq!(entry.icon.as_deref(), Some("org.example.App"));
        assert_eq!(entry.exec.as_deref(), Some("\"/opt/My App/bin/app\" %U"));
        assert!(entry.is_visible_app());

        let portugues = parse_desktop_entry(FIXTURE, &locale_variants("pt_PT.UTF-8"));
        assert_eq!(portugues.name.as_deref(), Some("Ficheiros"));
        let sem_locale = parse_desktop_entry(FIXTURE, &[]);
        assert_eq!(sem_locale.name.as_deref(), Some("Files"));
    }

    #[test]
    fn desktop_oculto_ou_que_nao_e_aplicativo_nao_conta() {
        let hidden = parse_desktop_entry(
            "[Desktop Entry]\nType=Application\nName=X\nNoDisplay=true\n",
            &[],
        );
        assert!(!hidden.is_visible_app());
        let link = parse_desktop_entry("[Desktop Entry]\nType=Link\nName=X\n", &[]);
        assert!(!link.is_visible_app());
        let removed = parse_desktop_entry(
            "[Desktop Entry]\nType=Application\nName=X\nHidden=true\n",
            &[],
        );
        assert!(!removed.is_visible_app());
        let unnamed = parse_desktop_entry("[Desktop Entry]\nType=Application\n", &[]);
        assert!(!unnamed.is_visible_app());
    }

    #[test]
    fn primeiro_token_do_exec_respeita_aspas() {
        assert_eq!(
            exec_program("\"/opt/My App/bin/app\" %U").as_deref(),
            Some("/opt/My App/bin/app")
        );
        assert_eq!(
            exec_program("/usr/bin/firefox %u").as_deref(),
            Some("/usr/bin/firefox")
        );
        assert_eq!(
            exec_program("\"/a/b \\\"c\\\"\"").as_deref(),
            Some("/a/b \"c\"")
        );
        assert_eq!(exec_program("\"aberto"), None);
        assert_eq!(exec_program("   "), None);
    }

    #[test]
    fn variantes_de_locale() {
        assert_eq!(locale_variants("pt_BR.UTF-8"), vec!["pt_BR", "pt"]);
        assert_eq!(locale_variants("de_DE@euro"), vec!["de_DE", "de"]);
        assert_eq!(locale_variants("en"), vec!["en"]);
        assert!(locale_variants("").is_empty());
    }

    #[test]
    fn icone_inexistente_ou_com_travessia_fica_sem_imagem() {
        let roots = vec![PathBuf::from("/caminho/que/nao/existe")];
        assert!(resolve_icon("nao-existe-pulse-teste", &roots).is_none());
        assert!(resolve_icon("../../etc/passwd", &roots).is_none());
        assert!(resolve_icon("", &roots).is_none());
    }

    #[test]
    fn icone_png_vira_data_uri() {
        let dir = std::env::temp_dir().join(format!("pulse-icon-{}", std::process::id()));
        let apps = dir.join("hicolor/48x48/apps");
        fs::create_dir_all(&apps).unwrap();
        fs::write(apps.join("pulse-teste.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let uri = resolve_icon("pulse-teste", std::slice::from_ref(&dir));
        fs::remove_dir_all(&dir).ok();
        assert_eq!(uri.as_deref(), Some("data:image/png;base64,iVBORw=="));
    }
}
