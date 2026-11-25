//! Extração e renderização de ícones em alta resolução (48–128px) para a Dock
//! e ícones vetoriais rasterizados da identidade visual do Pulse.

use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, path::Path};
use windows::{
    core::{HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, SIZE, S_OK},
        Graphics::{
            Direct2D::{
                Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_SIZE_U},
                ID2D1Bitmap, ID2D1DCRenderTarget, D2D1_BITMAP_PROPERTIES,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetObjectW,
                ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
            },
        },
        Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
        UI::{
            Shell::{
                IShellItemImageFactory, SHCreateItemFromParsingName, SHDefExtractIconW,
                SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SIIGBF_ICONONLY,
            },
            WindowsAndMessaging::{
                DestroyIcon, GetClassLongPtrW, GetIconInfo, SendMessageTimeoutW, GCLP_HICON,
                GCLP_HICONSM, HICON, ICONINFO, ICON_BIG, ICON_SMALL2, SMTO_ABORTIFHUNG, WM_GETICON,
            },
        },
    },
};

/// Imagem 32-bit BGRA com alfa pré-multiplicado (linhas de cima para baixo).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

struct OwnedBitmap(HBITMAP);
impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.0.into());
            }
        }
    }
}

struct OwnedIcon(HICON);
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.0);
            }
        }
    }
}

/// Carrega ícone em alta resolução (`size` entre 48 e 128 px) priorizando:
/// 1. Apps MSIX/UWP (`WindowsApps` ou `shell:AppsFolder\...`) via `IShellItemImageFactory`
///    (evita o ícone genérico de janela branca retornado por `SHDefExtractIconW` em `.exe` UWP como `WhatsApp.Root.exe`)
/// 2. `SHDefExtractIconW` para `.exe` Win32/Electron (com resolução automática de pasta versionada `app-*` como Discord)
/// 3. `WM_GETICON` / classe da janela / `SHGetFileInfoW`
pub fn load_high_res_icon(
    hwnd: Option<HWND>,
    source: Option<&str>,
    target_px: u32,
) -> Option<IconImage> {
    let size = target_px.clamp(48, 128);

    if let Some(src) = source {
        // 1. Apps da Microsoft Store / MSIX (ex.: WhatsApp.Root.exe em WindowsApps):
        // usa shell:AppsFolder\<AUMID> antes de SHDefExtractIconW.
        if src.starts_with("shell:AppsFolder\\") {
            if let Some(img) = extract_shell_image_factory(src, size) {
                return Some(trim_and_pad_square(img));
            }
        } else if let Some(apps_folder_id) = super::dock_window::store_app_id(src) {
            if let Some(img) = extract_shell_image_factory(&apps_folder_id, size) {
                return Some(trim_and_pad_square(img));
            }
            // Se falhar via AppsFolder, prefere o ícone real da janela antes do genérico do exe.
            if let Some(target) = hwnd {
                if let Some((icon, owned)) = extract_window_hicon(target) {
                    let guard = owned.then_some(OwnedIcon(icon));
                    let img = hicon_to_icon_image(icon);
                    drop(guard);
                    if let Some(img) = img {
                        return Some(trim_and_pad_square(img));
                    }
                }
            }
        }

        // 2. Executável Win32 / Electron (inclui fallback para versões atualizadas tipo Discord\app-1.0.xxxx)
        let resolved_exe = resolve_existing_exe_path(src);
        let exe_to_use = resolved_exe.as_deref().unwrap_or(src);
        let is_windows_app = exe_to_use.to_ascii_lowercase().contains("\\windowsapps\\");
        if !is_windows_app
            && !exe_to_use.starts_with("shell:AppsFolder\\")
            && Path::new(exe_to_use).is_file()
        {
            if let Some(img) = extract_exe_icon_shdef(exe_to_use, size) {
                return Some(trim_and_pad_square(img));
            }
        }
        if !is_windows_app {
            if let Some(img) = extract_shell_image_factory(exe_to_use, size) {
                return Some(trim_and_pad_square(img));
            }
        }
    }

    if let Some(target) = hwnd {
        if let Some((icon, owned)) = extract_window_hicon(target) {
            let guard = owned.then_some(OwnedIcon(icon));
            let img = hicon_to_icon_image(icon);
            drop(guard);
            if let Some(img) = img {
                return Some(trim_and_pad_square(img));
            }
        }
    }

    if let Some(src) = source {
        let resolved_exe = resolve_existing_exe_path(src);
        let exe_to_use = resolved_exe.as_deref().unwrap_or(src);
        if let Some((icon, owned)) = extract_shgetfileinfo_hicon(exe_to_use) {
            let guard = owned.then_some(OwnedIcon(icon));
            let img = hicon_to_icon_image(icon);
            drop(guard);
            if let Some(img) = img {
                return Some(trim_and_pad_square(img));
            }
        }
    }

    None
}

/// Se um `.exe` dentro de uma pasta versionada (`...\Discord\app-1.0.9268\Discord.exe`)
/// deixou de existir após auto-update, procura a pasta `app-*` mais recente com o mesmo binário.
fn resolve_existing_exe_path(src: &str) -> Option<String> {
    let path = Path::new(src);
    if path.is_file() {
        return None;
    }
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;
    if !parent_name.to_ascii_lowercase().starts_with("app-") {
        return None;
    }
    let grand_parent = parent.parent()?;
    let entries = std::fs::read_dir(grand_parent).ok()?;
    let mut candidates: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.to_ascii_lowercase().starts_with("app-"))
        })
        .map(|p| p.join(file_name))
        .filter(|p| p.is_file())
        .collect();
    candidates.sort();
    candidates.pop().map(|p| p.to_string_lossy().into_owned())
}

fn extract_exe_icon_shdef(path: &str, size: u32) -> Option<IconImage> {
    let mut icon = HICON::default();
    let hr = unsafe { SHDefExtractIconW(&HSTRING::from(path), 0, 0, Some(&mut icon), None, size) };
    if hr != S_OK || icon.is_invalid() {
        return None;
    }
    let guard = OwnedIcon(icon);
    hicon_to_icon_image(guard.0)
}

fn extract_shell_image_factory(parsing_name: &str, size: u32) -> Option<IconImage> {
    let side = size as i32;
    let bmp = unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(parsing_name), None).ok()?;
        OwnedBitmap(
            factory
                .GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY)
                .ok()?,
        )
    };
    let (width, height, mut bgra) = read_hbitmap_bits(bmp.0)?;
    if bgra.as_chunks::<4>().0.iter().all(|px| px[3] == 0) {
        for px in bgra.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    } else if bgra
        .as_chunks::<4>()
        .0
        .iter()
        .any(|px| px[0] > px[3] || px[1] > px[3] || px[2] > px[3])
    {
        premultiply_bgra(&mut bgra);
    }
    Some(IconImage {
        width,
        height,
        bgra,
    })
}

fn extract_window_hicon(target: HWND) -> Option<(HICON, bool)> {
    unsafe {
        let mut result: usize = 0;
        if SendMessageTimeoutW(
            target,
            WM_GETICON,
            windows::Win32::Foundation::WPARAM(ICON_BIG as usize),
            windows::Win32::Foundation::LPARAM(0),
            SMTO_ABORTIFHUNG,
            80,
            Some(&mut result),
        )
        .0 != 0
            && result != 0
        {
            return Some((HICON(result as *mut _), false));
        }
        if SendMessageTimeoutW(
            target,
            WM_GETICON,
            windows::Win32::Foundation::WPARAM(ICON_SMALL2 as usize),
            windows::Win32::Foundation::LPARAM(0),
            SMTO_ABORTIFHUNG,
            80,
            Some(&mut result),
        )
        .0 != 0
            && result != 0
        {
            return Some((HICON(result as *mut _), false));
        }
        let cls_big = GetClassLongPtrW(target, GCLP_HICON);
        if cls_big != 0 {
            return Some((HICON(cls_big as *mut _), false));
        }
        let cls_small = GetClassLongPtrW(target, GCLP_HICONSM);
        if cls_small != 0 {
            return Some((HICON(cls_small as *mut _), false));
        }
    }
    None
}

fn extract_shgetfileinfo_hicon(path: &str) -> Option<(HICON, bool)> {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut info = SHFILEINFOW::default();
    let res = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if res != 0 && !info.hIcon.is_invalid() {
        Some((info.hIcon, true))
    } else {
        None
    }
}

pub fn hicon_to_icon_image(hicon: HICON) -> Option<IconImage> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(hicon, &mut info) }.ok()?;
    let color = OwnedBitmap(info.hbmColor);
    let mask = OwnedBitmap(info.hbmMask);
    if color.0.is_invalid() {
        return None;
    }
    let (width, height, mut bgra) = read_hbitmap_bits(color.0)?;
    if bgra.as_chunks::<4>().0.iter().any(|px| px[3] != 0) {
        premultiply_bgra(&mut bgra);
    } else {
        let (_, _, mask_bits) = read_hbitmap_bits(mask.0)?;
        if mask_bits.len() != bgra.len() {
            return None;
        }
        for (px, m) in bgra
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(mask_bits.as_chunks::<4>().0.iter())
        {
            if m[0] == 0 {
                px[3] = 255;
            } else {
                *px = [0; 4];
            }
        }
    }
    Some(IconImage {
        width,
        height,
        bgra,
    })
}

fn read_hbitmap_bits(hbmp: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    let mut bm = BITMAP::default();
    let got = unsafe {
        GetObjectW(
            hbmp.into(),
            size_of::<BITMAP>() as i32,
            Some((&raw mut bm).cast()),
        )
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth as u32, bm.bmHeight.unsigned_abs());
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let lines = unsafe {
        let dc = GetDC(None);
        let hdc = CreateCompatibleDC(Some(dc));
        let target_dc = if hdc.is_invalid() { dc } else { hdc };
        let lines = GetDIBits(
            target_dc,
            hbmp,
            0,
            h,
            Some(bgra.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if !hdc.is_invalid() {
            let _ = DeleteDC(hdc);
        }
        ReleaseDC(None, dc);
        lines
    };
    (lines != 0).then_some((w, h, bgra))
}

fn premultiply_bgra(bgra: &mut [u8]) {
    for px in bgra.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        for ch in &mut px[..3] {
            *ch = ((u16::from(*ch) * a + 127) / 255) as u8;
        }
    }
}

/// Remove margens transparentes excessivas e centraliza num quadrado uniforme
/// para que todos os ícones da Dock tenham peso visual homogêneo.
pub fn trim_and_pad_square(img: IconImage) -> IconImage {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return img;
    }
    let alpha_at = |x: u32, y: u32| img.bgra[((y * w + x) * 4 + 3) as usize];
    let (mut l, mut t, mut r, mut b) = (w, h, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            if alpha_at(x, y) > 8 {
                l = l.min(x);
                t = t.min(y);
                r = r.max(x + 1);
                b = b.max(y + 1);
            }
        }
    }
    if l >= r || t >= b {
        return img;
    }
    let cw = r - l;
    let ch = b - t;
    let max_side = cw.max(ch);
    let pad = (max_side / 24).max(1);
    let out_side = max_side + pad * 2;
    let ox = (out_side - cw) / 2;
    let oy = (out_side - ch) / 2;

    let mut out = vec![0u8; (out_side * out_side * 4) as usize];
    for sy in 0..ch {
        let src_row = (((t + sy) * w + l) * 4) as usize;
        let dst_row = (((oy + sy) * out_side + ox) * 4) as usize;
        let row_bytes = (cw * 4) as usize;
        out[dst_row..dst_row + row_bytes].copy_from_slice(&img.bgra[src_row..src_row + row_bytes]);
    }

    IconImage {
        width: out_side,
        height: out_side,
        bgra: out,
    }
}

pub fn d2d_bitmap_from_icon(rt: &ID2D1DCRenderTarget, img: &IconImage) -> Option<ID2D1Bitmap> {
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    unsafe {
        rt.CreateBitmap(
            D2D_SIZE_U {
                width: img.width,
                height: img.height,
            },
            Some(img.bgra.as_ptr().cast()),
            img.width * 4,
            &props,
        )
        .ok()
    }
}

/// Gera a marca vetorial do Pulse (moldura P + sinal ECG laranja) com anti-aliasing.
pub fn create_pulse_mark_icon(size: u32, dark_theme: bool) -> IconImage {
    let s = size.max(16);
    let mut bgra = vec![0u8; (s * s * 4) as usize];
    let scale = s as f32 / 48.0;

    let rail_rgb = if dark_theme {
        [214u8, 226, 240]
    } else {
        [52u8, 73, 94]
    };
    let signal_rgb = [239u8, 143, 18]; // Pulse orange #ef8f12

    let rail = [
        (8.0f32, 34.0f32),
        (8.0, 12.0),
        (12.0, 8.0),
        (36.0, 8.0),
        (40.0, 12.0),
        (40.0, 36.0),
        (36.0, 40.0),
        (20.0, 40.0),
    ];
    draw_aa_polyline(&mut bgra, s, &rail, scale, 3.0 * scale, rail_rgb);

    let signal = [
        (4.0f32, 26.0f32),
        (13.0, 26.0),
        (19.0, 13.0),
        (19.0, 37.0),
        (26.0, 17.0),
        (30.0, 26.0),
        (44.0, 26.0),
    ];
    draw_aa_polyline(&mut bgra, s, &signal, scale, 3.4 * scale, signal_rgb);

    IconImage {
        width: s,
        height: s,
        bgra,
    }
}

/// Gera o ícone nativo do Explorador de Arquivos do Pulse ("Arquivos") para a Dock
/// (estilo Finder/Pasta mineral com acento Pulse).
pub fn create_pulse_files_icon(size: u32) -> IconImage {
    let s = size.max(32);
    let mut bgra = vec![0u8; (s * s * 4) as usize];
    let sf = s as f32;
    let margin = sf * 0.08;
    let w = sf - margin * 2.0;
    let r = sf * 0.20;

    // Corpo principal arredondado com gradiente azul mineral estilo macOS
    for y in 0..s {
        for x in 0..s {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let d = sd_rounded_rect(px - sf * 0.5, py - sf * 0.5, w * 0.5, w * 0.5, r);
            let cov = (0.75 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                let t = ((py - margin) / w).clamp(0.0, 1.0);
                // Gradiente azul ciano/mineral
                let red = lerp_u8(74.0, 32.0, t);
                let green = lerp_u8(168.0, 114.0, t);
                let blue = lerp_u8(248.0, 214.0, t);
                blend_premul_bgra(&mut bgra, s, x, y, [blue, green, red], cov);

                // Borda luminosa interna de 1.5px
                let border_cov = ((1.2 - d.abs()).clamp(0.0, 1.0)) * 0.35;
                if border_cov > 0.0 {
                    blend_premul_bgra(&mut bgra, s, x, y, [255, 255, 255], border_cov);
                }
            }
        }
    }

    // Aba superior da pasta + cartão interno branco translúcido
    let folder_w = w * 0.66;
    let folder_h = w * 0.46;
    let folder_cy = sf * 0.54;
    let fr = sf * 0.07;
    for y in 0..s {
        for x in 0..s {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let d_body = sd_rounded_rect(
                px - sf * 0.5,
                py - folder_cy,
                folder_w * 0.5,
                folder_h * 0.5,
                fr,
            );
            let d_tab = sd_rounded_rect(
                px - (sf * 0.5 - folder_w * 0.24),
                py - (folder_cy - folder_h * 0.52),
                folder_w * 0.22,
                folder_h * 0.14,
                fr * 0.7,
            );
            let d = d_body.min(d_tab);
            let cov = (0.75 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_premul_bgra(&mut bgra, s, x, y, [252, 248, 242], cov * 0.94);
            }
        }
    }

    // Linha de sinal Pulse laranja dentro da pasta
    let scale = s as f32 / 48.0;
    let signal = [
        (16.0f32, 27.5f32),
        (20.0, 27.5),
        (22.5, 22.0),
        (24.5, 32.5),
        (27.0, 24.5),
        (29.0, 27.5),
        (32.5, 27.5),
    ];
    draw_aa_polyline(&mut bgra, s, &signal, scale, 2.2 * scale, [239, 143, 18]);

    IconImage {
        width: s,
        height: s,
        bgra,
    }
}

fn lerp_u8(a: f32, b: f32, t: f32) -> u8 {
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

fn sd_rounded_rect(px: f32, py: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = px.abs() - hx + r;
    let qy = py.abs() - hy + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn draw_aa_polyline(
    bgra: &mut [u8],
    size: u32,
    points: &[(f32, f32)],
    coord_scale: f32,
    stroke_width: f32,
    rgb: [u8; 3],
) {
    let half = stroke_width * 0.5;
    let color_bgr = [rgb[2], rgb[1], rgb[0]];
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mut dist = f32::MAX;
            for pair in points.windows(2) {
                let (x1, y1) = (pair[0].0 * coord_scale, pair[0].1 * coord_scale);
                let (x2, y2) = (pair[1].0 * coord_scale, pair[1].1 * coord_scale);
                let dx = x2 - x1;
                let dy = y2 - y1;
                let len_sq = dx * dx + dy * dy;
                let t = if len_sq == 0.0 {
                    0.0
                } else {
                    (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0)
                };
                let cx = x1 + t * dx;
                let cy = y1 + t * dy;
                dist = dist.min((px - cx).hypot(py - cy));
            }
            let cov = (half + 0.75 - dist).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_premul_bgra(bgra, size, x, y, color_bgr, cov);
            }
        }
    }
}

fn blend_premul_bgra(bgra: &mut [u8], size: u32, x: u32, y: u32, src_bgr: [u8; 3], alpha: f32) {
    let idx = ((y * size + x) * 4) as usize;
    let sa = alpha.clamp(0.0, 1.0);
    let inv = 1.0 - sa;
    for ch in 0..3 {
        let dst = bgra[idx + ch] as f32;
        let src = src_bgr[ch] as f32 * sa;
        bgra[idx + ch] = (src + dst * inv).round().clamp(0.0, 255.0) as u8;
    }
    let da = bgra[idx + 3] as f32;
    bgra[idx + 3] = (sa * 255.0 + da * inv).round().clamp(0.0, 255.0) as u8;
}
