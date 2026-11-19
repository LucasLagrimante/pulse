//! Arquivos usados recentemente no Linux, lidos do
//! `$XDG_DATA_HOME/recently-used.xbel` (especificação freedesktop Desktop
//! Bookmarks). GTK, KDE (KIO/`KRecentDocument`), Nemo, Chrome e LibreOffice
//! gravam ali. Complementa a varredura das pastas XDG com arquivos abertos de
//! outros lugares (projetos, pendrives).
//!
//! O critério de "recente" continua o mesmo da varredura: data de modificação
//! do arquivo dentro da janela. Entradas que já não existem, ocultas, links,
//! pastas ou de tipos fora das categorias são ignoradas.

use super::{category, ignored, RecentFile, MAX_RESULTS};
use std::{
    ffi::OsString,
    fs,
    os::unix::ffi::OsStringExt,
    path::{Component, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Teto do arquivo lido: listas normais têm dezenas de KB.
const MAX_XBEL_BYTES: u64 = 8 * 1024 * 1024;

fn xbel_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("recently-used.xbel"))
}

pub(super) fn recently_used(cutoff: SystemTime) -> Vec<RecentFile> {
    let Some(path) = xbel_path() else {
        return Vec::new();
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return Vec::new();
    };
    if metadata.len() > MAX_XBEL_BYTES {
        return Vec::new();
    }
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    // Bookmarks mais recentes costumam ser gravados no fim do XBEL.
    for (checked, path) in parse_file_hrefs(&content).into_iter().rev().enumerate() {
        if checked > 0 && checked.is_multiple_of(15) {
            std::thread::yield_now();
        }
        if files.len() >= MAX_RESULTS {
            break;
        }
        if let Some(file) = recent_file(path, cutoff) {
            files.push(file);
        }
    }
    files
}

fn recent_file(path: PathBuf, cutoff: SystemTime) -> Option<RecentFile> {
    let hidden = path.components().any(|component| {
        matches!(component, Component::Normal(name) if name.to_string_lossy().starts_with('.'))
    });
    if hidden || ignored(&path) {
        return None;
    }
    let category = category(&path)?;
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata.modified().ok()?;
    if modified < cutoff {
        return None;
    }
    Some(RecentFile {
        name: path.file_name()?.to_string_lossy().into_owned(),
        path: path.to_string_lossy().into_owned(),
        category: category.into(),
        size_bytes: metadata.len(),
        modified_at_unix_ms: modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64,
    })
}

/// Caminhos locais (`file://`) dos atributos `href` de cada `<bookmark>`, na
/// ordem do arquivo. URLs de outros esquemas são descartadas.
pub(super) fn parse_file_hrefs(xml: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<bookmark ") {
        rest = &rest[start + "<bookmark ".len()..];
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..tag_end];
        if let Some(href) = attribute(tag, "href") {
            if let Some(path) = file_uri_to_path(&decode_entities(href)) {
                paths.push(path);
            }
        }
        rest = &rest[tag_end..];
    }
    paths
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut search = tag;
    loop {
        let index = search.find(name)?;
        let before_ok = index == 0
            || search[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after = &search[index + name.len()..];
        if before_ok {
            if let Some(value) = after.trim_start().strip_prefix('=') {
                let value = value.trim_start();
                let quote = value.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let value = &value[1..];
                    return value.find(quote).map(|end| &value[..end]);
                }
            }
        }
        search = after;
    }
}

fn decode_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `file:///home/u/a%20b.pdf` → `/home/u/a b.pdf`. Aceita `file://localhost/`.
/// Bytes percent-encoded viram bytes do caminho (nomes não UTF-8 continuam
/// válidos no Linux).
pub(super) fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let Some(value) = rest
                .get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                decoded.push(value);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    if decoded.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(decoded)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0">
  <bookmark href="file:///home/u/Downloads/" added="2026-06-10T14:30:37Z" modified="2026-10-21T14:51:26Z" visited="2026-10-21T14:51:26Z">
  </bookmark>
  <bookmark href="https://claude.ai/x?a=1&amp;b=2" added="2026-10-07T12:43:33Z">
  </bookmark>
  <bookmark href="file:///home/u/Documentos/Chat%20IA%20%C3%A1.pdf" added="2026-10-15T14:13:20Z">
  </bookmark>
  <bookmark added="2026-10-15T14:13:20Z" href='file://localhost/tmp/R&amp;D.txt'>
  </bookmark>
</xbel>"#;

    #[test]
    fn parses_only_local_file_bookmarks() {
        assert_eq!(
            parse_file_hrefs(SAMPLE),
            vec![
                PathBuf::from("/home/u/Downloads/"),
                PathBuf::from("/home/u/Documentos/Chat IA á.pdf"),
                PathBuf::from("/tmp/R&D.txt"),
            ]
        );
    }

    #[test]
    fn decodes_uris_and_entities() {
        assert_eq!(decode_entities("a&amp;b&#233;&#xE9;&bogus"), "a&béé&bogus");
        assert_eq!(decode_entities("x&nada;y"), "x&nada;y");
        assert_eq!(
            file_uri_to_path("file:///a%2Fb"),
            Some(PathBuf::from("/a/b"))
        );
        assert_eq!(file_uri_to_path("file:///x%00y"), None);
        assert_eq!(file_uri_to_path("file://server/share/x"), None);
        assert_eq!(file_uri_to_path("https://x/y"), None);
    }

    #[test]
    fn keeps_only_recent_visible_files_of_known_categories() {
        let dir = std::env::temp_dir().join(format!("pulse-xbel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".oculta")).unwrap();
        let pdf = dir.join("relatorio.pdf");
        fs::write(&pdf, b"%PDF").unwrap();
        fs::write(dir.join(".oculta/x.pdf"), b"%PDF").unwrap();
        fs::write(dir.join("dados.bin"), b"x").unwrap();
        let cutoff = SystemTime::now() - Duration::from_secs(60);

        let found = recent_file(pdf.clone(), cutoff).expect("pdf recente");
        assert_eq!(found.category, "document");
        assert_eq!(found.name, "relatorio.pdf");
        assert!(recent_file(dir.join(".oculta/x.pdf"), cutoff).is_none());
        assert!(recent_file(dir.join("dados.bin"), cutoff).is_none());
        assert!(recent_file(dir.join("sumiu.pdf"), cutoff).is_none());
        assert!(recent_file(pdf, SystemTime::now() + Duration::from_secs(60)).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Coleta real só de leitura do `recently-used.xbel` desta máquina.
    #[test]
    fn real_recently_used_entries_are_valid_files() {
        let cutoff = SystemTime::now() - Duration::from_secs(30 * 86_400);
        for file in recently_used(cutoff) {
            assert!(std::path::Path::new(&file.path).is_file(), "{}", file.path);
            assert!(category(std::path::Path::new(&file.path)).is_some());
        }
    }
}
