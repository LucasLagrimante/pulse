//! Cantos arredondados das janelas flutuantes (Runner e painel rápido).
//!
//! Duas camadas, porque nenhuma resolve sozinha:
//!
//! 1. **DWM (`DWMWA_WINDOW_CORNER_PREFERENCE`)** — o Windows 11 arredonda a
//!    janela na composição, com antialiasing. É o único caminho para a curva
//!    ficar lisa; `SetWindowRgn` é uma região binária e deixa degrau de 1px.
//! 2. **`SetWindowRgn`** — fallback para Windows 10, onde o DWM não tem essa
//!    preferência. A curva fica serrilhada, mas não sobra fundo quadrado.
//!
//! Contexto: o WebView2 158 pinta base opaca atrás da página mesmo com
//! `DefaultBackgroundColor` alpha=0 (WebView2Feedback #5752), então janela
//! transparente vira moldura escura. O recorte é o que impede o preto de
//! aparecer atrás da curva.

/// Aplica cantos arredondados na janela, preferindo a composição do DWM.
///
/// `radius` é em pixels lógicos e só é usado no fallback `SetWindowRgn`.
pub fn apply_rounded(window: &tauri::WebviewWindow, radius: f64) {
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };

        let Ok(hwnd) = window.hwnd() else {
            return;
        };

        let preference = DWMWCP_ROUND;
        let applied = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&preference).cast(),
                std::mem::size_of_val(&preference) as u32,
            )
        }
        .is_ok();

        if !applied {
            apply_region(hwnd, window, radius);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (window, radius);
    }
}

/// Fallback binário para sistemas sem `DWMWA_WINDOW_CORNER_PREFERENCE`.
#[cfg(windows)]
fn apply_region(
    hwnd: windows::Win32::Foundation::HWND,
    window: &tauri::WebviewWindow,
    radius: f64,
) {
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};

    let Ok(size) = window.outer_size() else {
        return;
    };
    let width = size.width as i32;
    let height = size.height as i32;
    if width <= 0 || height <= 0 {
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let diameter = ((radius * scale).round() as i32).max(1) * 2;
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
        if region.is_invalid() {
            return;
        }
        // `SetWindowRgn` assume a posse da região: não deletar o HRGN.
        let _ = SetWindowRgn(hwnd, Some(region), true);
    }
}

/// Recorta apenas as janelas flutuantes conhecidas, ignorando as demais.
pub fn apply_to_known_window(window: &tauri::WebviewWindow) {
    match window.label() {
        "launcher" => apply_rounded(window, 20.0),
        "tray-flyout" => apply_rounded(window, 20.0),
        _ => {}
    }
}
