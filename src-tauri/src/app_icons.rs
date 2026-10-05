//! Extração de ícones de aplicativos para a tela de Inicialização e Segurança.
//!
//! O ícone associado ao comando de boot é lido pelo shell do Windows
//! (`SHGetFileInfoW`) e convertido para PNG em data URI, pronto para ser
//! exibido diretamente no WebView. O resultado é cacheado por caminho de
//! executável (inclusive falhas) para não repetir trabalho de GDI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(windows)]
use std::sync::Mutex;

#[cfg(windows)]
static ICON_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

#[cfg(windows)]
fn icon_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Retorna um data URI PNG com o ícone associado ao comando de inicialização.
///
/// `None` significa que o caminho não pôde ser resolvido ou o shell não
/// forneceu um ícone — nunca um ícone fictício.
pub fn icon_for_command(command: &str) -> Option<String> {
    let path = resolve_executable(command)?;
    icon_for_resolved_path(&path)
}

/// Retorna um data URI PNG com o ícone do executável em `path`.
///
/// `None` = sem ícone disponível (nunca fictício). Compartilha o cache de
/// `icon_for_command`, então é barato chamar em polling (ex: mixer).
pub fn icon_for_path(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    icon_for_resolved_path(path)
}

fn icon_for_resolved_path(path: &Path) -> Option<String> {
    let key = path.to_string_lossy().to_lowercase();

    #[cfg(windows)]
    {
        if let Ok(cache) = icon_cache().lock() {
            if let Some(cached) = cache.get(&key) {
                return cached.clone();
            }
        }

        let icon = extract_icon_data_uri(path);

        if let Ok(mut cache) = icon_cache().lock() {
            cache.insert(key, icon.clone());
        }

        icon
    }

    #[cfg(not(windows))]
    {
        let _ = key;
        None
    }
}

/// Extrai o caminho executável de uma linha de comando do registro `Run`.
///
/// Aceita caminhos entre aspas (`"C:\App\a.exe" --flag`), caminhos sem aspas
/// com espaços (delimitados pela extensão executável) e nomes simples
/// resolvidos pelo `PATH`. Também expande variáveis `%VAR%`.
fn resolve_executable(command: &str) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }

    let expanded = expand_env_vars(trimmed);
    let candidate = extract_command_path(&expanded)?;
    if candidate.is_empty() {
        return None;
    }

    let path = PathBuf::from(candidate);
    if path.is_file() {
        return Some(path);
    }

    // Nome simples (`OneDrive.exe`): procura nos diretórios do PATH.
    if path.components().count() == 1 {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let full = dir.join(&path);
                if full.is_file() {
                    return Some(full);
                }
            }
        }
    }

    None
}

const EXECUTABLE_EXTENSIONS: [&str; 8] = [
    ".exe", ".com", ".bat", ".cmd", ".lnk", ".url", ".ps1", ".vbs",
];

fn extract_command_path(expanded: &str) -> Option<String> {
    if let Some(rest) = expanded.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }

    let lower = expanded.to_lowercase();
    let mut end: Option<usize> = None;
    for ext in EXECUTABLE_EXTENSIONS {
        if let Some(idx) = lower.find(ext) {
            let after = idx + ext.len();
            let at_boundary = after == lower.len() || lower.as_bytes().get(after) == Some(&b' ');
            if at_boundary {
                end = Some(match end {
                    Some(previous) => previous.min(after),
                    None => after,
                });
            }
        }
    }

    match end {
        Some(end) => Some(expanded[..end].trim().to_string()),
        None => expanded.split_whitespace().next().map(str::to_string),
    }
}

fn expand_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut parts = input.split('%');

    if let Some(first) = parts.next() {
        result.push_str(first);
    }

    // Partes internas alternam entre nome de variável e texto literal.
    let mut is_variable = true;
    for part in parts {
        if is_variable {
            match std::env::var(part) {
                Ok(value) => result.push_str(&value),
                Err(_) => {
                    result.push('%');
                    result.push_str(part);
                    result.push('%');
                }
            }
        } else {
            result.push_str(part);
        }
        is_variable = !is_variable;
    }

    result
}

#[cfg(windows)]
fn extract_icon_data_uri(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut info = SHFILEINFOW::default();
        let result = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            Default::default(),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );

        if result == 0 || info.hIcon.is_invalid() {
            return None;
        }

        let icon = info.hIcon;
        let png = icon_to_png(icon);
        let _ = DestroyIcon(icon);
        png
    }
}

#[cfg(windows)]
unsafe fn icon_to_png(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<String> {
    use windows::Win32::Graphics::Gdi::DeleteObject;
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    let mut info = ICONINFO::default();
    GetIconInfo(icon, &mut info).ok()?;

    let color = info.hbmColor;
    let mask = info.hbmMask;

    let rgba = if !color.is_invalid() {
        bitmap_to_rgba(color, if mask.is_invalid() { None } else { Some(mask) })
    } else {
        None
    };

    if !color.is_invalid() {
        let _ = DeleteObject(color.into());
    }
    if !mask.is_invalid() {
        let _ = DeleteObject(mask.into());
    }

    let (width, height, pixels) = rgba?;
    encode_png_data_uri(width, height, &pixels)
}

#[cfg(windows)]
unsafe fn bitmap_to_rgba(
    color: windows::Win32::Graphics::Gdi::HBITMAP,
    mask: Option<windows::Win32::Graphics::Gdi::HBITMAP>,
) -> Option<(u32, u32, Vec<u8>)> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, GetObjectW, BITMAP, HDC};

    let mut bmp = BITMAP::default();
    if GetObjectW(
        color.into(),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut BITMAP as *mut c_void),
    ) == 0
    {
        return None;
    }

    let width = bmp.bmWidth;
    let height = bmp.bmHeight;
    if width <= 0 || height <= 0 {
        return None;
    }

    let hdc: HDC = CreateCompatibleDC(None);
    if hdc.is_invalid() {
        return None;
    }

    let color_bits = read_bitmap_bits(hdc, color, width, height, 32);
    let mask_bits = match mask {
        Some(mask) if !mask.is_invalid() => read_bitmap_bits(hdc, mask, width, height, 1),
        _ => None,
    };
    let _ = DeleteDC(hdc);

    let color_bits = color_bits?;

    let w = width as usize;
    let h = height as usize;
    let mut rgba = vec![0u8; w * h * 4];
    let mut has_alpha = false;

    for y in 0..h {
        // GetDIBits entrega as linhas de baixo para cima (bottom-up).
        let src_row = (h - 1 - y) * w * 4;
        for x in 0..w {
            let src = src_row + x * 4;
            let dst = (y * w + x) * 4;
            let alpha = color_bits[src + 3];
            if alpha != 0 {
                has_alpha = true;
            }
            rgba[dst] = color_bits[src + 2];
            rgba[dst + 1] = color_bits[src + 1];
            rgba[dst + 2] = color_bits[src];
            rgba[dst + 3] = alpha;
        }
    }

    if !has_alpha {
        // Ícones legados não trazem canal alfa: a transparência vem da máscara AND.
        if let Some(mask_bits) = mask_bits.as_ref() {
            let mask_stride = w.div_ceil(32) * 4;
            for y in 0..h {
                let src_row = (h - 1 - y) * mask_stride;
                for x in 0..w {
                    let byte = mask_bits[src_row + x / 8];
                    let transparent = (byte >> (7 - (x % 8))) & 1 == 1;
                    rgba[(y * w + x) * 4 + 3] = if transparent { 0 } else { 255 };
                }
            }
        } else {
            for pixel in rgba.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
    }

    Some((width as u32, height as u32, rgba))
}

#[cfg(windows)]
unsafe fn read_bitmap_bits(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    width: i32,
    height: i32,
    bit_count: u16,
) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{GetDIBits, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS};

    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = bit_count;
    info.bmiHeader.biCompression = 0; // BI_RGB

    let stride = if bit_count >= 32 {
        width as usize * 4
    } else {
        (width as usize).div_ceil(32) * 4
    };
    let mut buffer = vec![0u8; stride * height as usize];

    let lines = GetDIBits(
        hdc,
        bitmap,
        0,
        height as u32,
        Some(buffer.as_mut_ptr() as *mut core::ffi::c_void),
        &mut info,
        DIB_RGB_COLORS,
    );

    if lines == 0 {
        None
    } else {
        Some(buffer)
    }
}

fn encode_png_data_uri(width: u32, height: u32, rgba: &[u8]) -> Option<String> {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;

    let mut buffer = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut buffer, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
        writer.finish().ok()?;
    }

    Some(format!("data:image/png;base64,{}", BASE64.encode(&buffer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrai_caminho_entre_aspas_com_argumentos() {
        assert_eq!(
            extract_command_path("\"C:\\Program Files\\App\\app.exe\" --minimized"),
            Some("C:\\Program Files\\App\\app.exe".to_string())
        );
    }

    #[test]
    fn extrai_caminho_sem_aspas_por_extensao() {
        assert_eq!(
            extract_command_path("C:\\Program Files\\App\\app.exe --minimized"),
            Some("C:\\Program Files\\App\\app.exe".to_string())
        );
    }

    #[test]
    fn extrai_nome_simples() {
        assert_eq!(
            extract_command_path("OneDrive.exe -silent"),
            Some("OneDrive.exe".to_string())
        );
    }

    #[test]
    fn expande_variaveis_de_ambiente() {
        std::env::set_var("PULSE_TEST_DIR", "C:\\Apps");
        assert_eq!(
            expand_env_vars("%PULSE_TEST_DIR%\\app.exe"),
            "C:\\Apps\\app.exe"
        );
        assert_eq!(expand_env_vars("C:\\fixo\\app.exe"), "C:\\fixo\\app.exe");
    }
}
