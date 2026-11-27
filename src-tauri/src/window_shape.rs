//! Recorte nativo das janelas flutuantes (Runner e painel rápido).
//!
//! O WebView2 158 pinta uma base opaca atrás da página mesmo com
//! `DefaultBackgroundColor` alpha=0 (WebView2Feedback #5752), então uma janela
//! transparente vira uma moldura escura em volta do painel. Sem transparência,
//! os cantos arredondados do painel revelariam o preto da janela.
//!
//! A saída é recortar a própria janela com `SetWindowRgn`: fora da região não
//! existe janela, portanto não existe fundo para aparecer atrás da curva.

/// Aplica um recorte de cantos arredondados na janela (Windows).
///
/// `radius` é em pixels lógicos; o fator de escala do monitor é aplicado aqui.
/// Silencioso por design: se a janela não expõe HWND (Linux/macOS ou janela já
/// destruída), o recorte simplesmente não é aplicado.
pub fn apply_rounded(window: &tauri::WebviewWindow, radius: f64) {
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};

        let Ok(hwnd) = window.hwnd() else {
            return;
        };
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
    #[cfg(not(windows))]
    {
        let _ = (window, radius);
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
