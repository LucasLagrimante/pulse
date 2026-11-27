//! Splash nativa do arranque: só a marca do Pulse animada, sem janela visível.
//!
//! Por que nativa e não uma janela Tauri: o WebView2 158 ignora fundo
//! transparente e pinta uma base opaca (WebView2Feedback #5752), o que deixaria
//! uma moldura em volta da marca; além disso o WebView só pinta depois de
//! inicializar. Aqui a janela é Win32 `WS_EX_LAYERED` com alpha por pixel
//! (`UpdateLayeredWindow` + Direct2D, mesmo padrão da Dock): o halo e o brilho
//! difusos nunca são cortados e a splash aparece antes do WebView carregar.
//!
//! O ciclo de vida é: `show` no `setup` → `dismiss` quando o painel principal
//! está pronto (`boot_complete`) ou quando qualquer fluxo abre a janela `main`.
//! Um watchdog de [`WATCHDOG_MS`] garante que o app nunca fique invisível.

/// Tempo máximo de vida da splash antes de forçar a abertura da janela principal.
pub const WATCHDOG_MS: u64 = 10_000;

/// Lógica pura (geometria e animação): testável sem Win32.
mod anim {
    pub const ENTER_MS: f32 = 450.0;
    pub const EXIT_MS: f32 = 220.0;
    pub const SIGNAL_LOOP_MS: f32 = 1800.0;
    /// Início do traço do sinal, depois que a entrada já avançou.
    pub const SIGNAL_DELAY_MS: f32 = 200.0;
    /// Fração do ciclo em que o sinal termina de se desenhar (`signal-trace`).
    pub const SIGNAL_DRAW_FRACTION: f32 = 0.55;

    pub type Point = (f32, f32);

    pub fn ease_out_cubic(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    pub fn ease_in_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if t < 0.5 {
            2.0 * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
        }
    }

    /// Contorno da marca (`PulseMark`, viewBox 48) como polilinha.
    pub fn rail_points() -> Vec<Point> {
        // Cantos: arcos de raio 6 (SVG `a6 6 0 0 1`), no sentido horário.
        fn arc(pts: &mut Vec<Point>, cx: f32, cy: f32, from_deg: f32) {
            for i in 1..=8 {
                let a = (from_deg + 90.0 * i as f32 / 8.0).to_radians();
                pts.push((cx + 6.0 * a.cos(), cy + 6.0 * a.sin()));
            }
        }
        let mut pts = vec![(8.0, 34.0), (8.0, 14.0)];
        arc(&mut pts, 14.0, 14.0, 180.0);
        pts.push((34.0, 8.0));
        arc(&mut pts, 34.0, 14.0, 270.0);
        pts.push((40.0, 34.0));
        arc(&mut pts, 34.0, 34.0, 0.0);
        pts.push((20.0, 40.0));
        pts
    }

    /// Sinal de pulso (`M4 26h10l5-12v24l7-20 4 8h14`).
    pub fn signal_points() -> Vec<Point> {
        vec![
            (4.0, 26.0),
            (14.0, 26.0),
            (19.0, 14.0),
            (19.0, 38.0),
            (26.0, 18.0),
            (30.0, 26.0),
            (44.0, 26.0),
        ]
    }

    fn seg_len(a: Point, b: Point) -> f32 {
        ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
    }

    pub fn polyline_len(pts: &[Point]) -> f32 {
        pts.windows(2).map(|w| seg_len(w[0], w[1])).sum()
    }

    /// Prefixo da polilinha com `fraction` (0..=1) do comprimento total.
    pub fn partial(pts: &[Point], fraction: f32) -> Vec<Point> {
        let target = polyline_len(pts) * fraction.clamp(0.0, 1.0);
        let mut out = Vec::new();
        let Some(&first) = pts.first() else {
            return out;
        };
        out.push(first);
        if target <= 0.0 {
            // Sem segmentos: evita um ponto isolado (cap redondo) no início.
            return out;
        }
        let mut walked = 0.0;
        for w in pts.windows(2) {
            let len = seg_len(w[0], w[1]);
            if walked + len >= target {
                let k = if len > 0.0 {
                    (target - walked) / len
                } else {
                    0.0
                };
                out.push((
                    w[0].0 + (w[1].0 - w[0].0) * k,
                    w[0].1 + (w[1].1 - w[0].1) * k,
                ));
                return out;
            }
            walked += len;
            out.push(w[1]);
        }
        out
    }

    /// Canto superior esquerdo de um quadrado `size` centrado em `(cx, cy)`.
    pub fn centered_origin(cx: i32, cy: i32, size: i32) -> (i32, i32) {
        (cx - size / 2, cy - size / 2)
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Frame {
        /// Opacidade global da janela (0..=1).
        pub alpha: f32,
        pub scale: f32,
        /// Fração do contorno já desenhada.
        pub rail: f32,
        /// Fração do sinal já desenhada.
        pub signal: f32,
        pub signal_opacity: f32,
        /// Intensidade do brilho na ponta do sinal.
        pub glow: f32,
    }

    /// Estado da animação em `elapsed_ms` desde o início; `exit_ms` é o tempo
    /// desde o pedido de saída. `reduce_motion` mantém só o fade.
    pub fn frame(elapsed_ms: f32, exit_ms: Option<f32>, reduce_motion: bool) -> Frame {
        let enter = ease_out_cubic(elapsed_ms / ENTER_MS);
        let mut f = if reduce_motion {
            Frame {
                alpha: enter,
                scale: 1.0,
                rail: 1.0,
                signal: 1.0,
                signal_opacity: 1.0,
                glow: 0.0,
            }
        } else {
            let t = (elapsed_ms - SIGNAL_DELAY_MS).max(0.0);
            let phase = (t % SIGNAL_LOOP_MS) / SIGNAL_LOOP_MS;
            let draw = (phase / SIGNAL_DRAW_FRACTION).min(1.0);
            let started = elapsed_ms >= SIGNAL_DELAY_MS;
            Frame {
                alpha: enter,
                scale: 0.92 + 0.08 * enter,
                rail: enter,
                signal: if started { ease_in_out(draw) } else { 0.0 },
                signal_opacity: if started { 0.4 + 0.6 * draw } else { 0.0 },
                glow: if !started {
                    0.0
                } else if phase < SIGNAL_DRAW_FRACTION {
                    1.0
                } else {
                    (1.0 - (phase - SIGNAL_DRAW_FRACTION) / 0.25).max(0.0)
                },
            }
        };
        if let Some(x) = exit_ms {
            let d = (x / EXIT_MS).clamp(0.0, 1.0);
            f.alpha *= 1.0 - d;
            if !reduce_motion {
                f.scale *= 1.0 + 0.06 * d;
            }
        }
        f
    }

    pub fn exit_finished(exit_ms: f32) -> bool {
        exit_ms >= EXIT_MS
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn easing_nos_extremos() {
            assert_eq!(ease_out_cubic(0.0), 0.0);
            assert_eq!(ease_out_cubic(1.0), 1.0);
            assert_eq!(ease_in_out(0.0), 0.0);
            assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
            assert_eq!(ease_in_out(2.0), 1.0);
        }

        #[test]
        fn polilinha_parcial_respeita_a_fracao() {
            let pts = signal_points();
            let total = polyline_len(&pts);
            assert_eq!(partial(&pts, 1.0).last(), pts.last());
            assert_eq!(partial(&pts, 0.0), vec![pts[0]]);
            let metade = partial(&pts, 0.5);
            assert!((polyline_len(&metade) - total / 2.0).abs() < 1e-3);
        }

        #[test]
        fn contorno_comeca_e_termina_nos_pontos_do_svg() {
            let r = rail_points();
            assert_eq!(r.first(), Some(&(8.0, 34.0)));
            let (x, y) = *r.last().unwrap();
            assert!((x - 20.0).abs() < 1e-4 && (y - 40.0).abs() < 1e-4);
            // Fim de cada arco cai nos pontos do path original.
            assert!(r
                .iter()
                .any(|p| (p.0 - 14.0).abs() < 1e-3 && (p.1 - 8.0).abs() < 1e-3));
            assert!(r
                .iter()
                .any(|p| (p.0 - 40.0).abs() < 1e-3 && (p.1 - 14.0).abs() < 1e-3));
            assert!(r
                .iter()
                .any(|p| (p.0 - 34.0).abs() < 1e-3 && (p.1 - 40.0).abs() < 1e-3));
        }

        #[test]
        fn centraliza_o_quadrado() {
            assert_eq!(centered_origin(600, 400, 200), (500, 300));
        }

        #[test]
        fn entrada_saida_e_movimento_reduzido() {
            let ini = frame(0.0, None, false);
            assert_eq!(ini.alpha, 0.0);
            assert!((ini.scale - 0.92).abs() < 1e-6);
            let meio = frame(900.0, None, false);
            assert_eq!(meio.alpha, 1.0);
            assert_eq!(meio.scale, 1.0);
            let saindo = frame(900.0, Some(EXIT_MS), false);
            assert_eq!(saindo.alpha, 0.0);
            assert!(exit_finished(EXIT_MS));
            let reduzido = frame(900.0, None, true);
            assert_eq!((reduzido.scale, reduzido.signal), (1.0, 1.0));
        }

        #[test]
        fn sinal_so_comeca_depois_do_atraso() {
            assert_eq!(frame(100.0, None, false).signal, 0.0);
            assert!(frame(1000.0, None, false).signal > 0.0);
        }
    }
}

#[cfg(windows)]
mod win {
    use super::{anim, WATCHDOG_MS};
    use std::{
        cell::RefCell,
        mem::size_of,
        sync::{
            atomic::{AtomicIsize, Ordering},
            mpsc, Mutex, OnceLock,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };
    use windows::{
        core::w,
        Win32::{
            Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM},
            Graphics::{
                Direct2D::{
                    Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT},
                    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1StrokeStyle, D2D1_CAP_STYLE_ROUND,
                    D2D1_DASH_STYLE_SOLID, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                    D2D1_LINE_JOIN_ROUND, D2D1_RENDER_TARGET_PROPERTIES,
                    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_STROKE_STYLE_PROPERTIES,
                },
                Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                Gdi::{
                    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush,
                    GetMonitorInfoW, MonitorFromPoint, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER,
                    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP,
                    HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
                },
            },
            System::{
                LibraryLoader::GetModuleHandleW,
                Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
            },
            UI::{
                HiDpi::GetDpiForWindow,
                WindowsAndMessaging::{
                    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                    KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, SetTimer, ShowWindow,
                    SystemParametersInfoW, TranslateMessage, UnregisterClassW, UpdateLayeredWindow,
                    CS_HREDRAW, CS_VREDRAW, MSG, SPI_GETCLIENTAREAANIMATION, SW_SHOWNA,
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, ULW_ALPHA, WM_APP, WM_DESTROY, WM_TIMER,
                    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                    WS_EX_TRANSPARENT, WS_POPUP,
                },
            },
        },
    };
    use windows_numerics::Vector2;

    /// Lado lógico da janela: marca de ~140 px + folga para o halo.
    const WINDOW_LOGICAL: f32 = 200.0;
    /// Lado lógico da marca (viewBox 48 escalado para isto).
    const MARK_LOGICAL: f32 = 140.0;
    const TIMER_ID: usize = 1;
    const TIMER_MS: u32 = 16;
    const WM_APP_DISMISS: u32 = WM_APP + 41;

    static SPLASH_HWND: AtomicIsize = AtomicIsize::new(0);
    static SPLASH_THREAD: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();

    thread_local! {
        static STATE: RefCell<Option<Splash>> = const { RefCell::new(None) };
    }

    /// Cores fixas da splash. Ela flutua sobre o desktop, não sobre o app,
    /// então segue o tema do **sistema**; os valores espelham `--neu-accent`,
    /// `--neu-signal-secondary` e `--neu-bg` de `identity.css` (claro/escuro).
    struct Palette {
        accent: (f32, f32, f32),
        rail: (f32, f32, f32),
        halo: (f32, f32, f32),
    }

    fn rgb(hex: u32) -> (f32, f32, f32) {
        (
            ((hex >> 16) & 0xff) as f32 / 255.0,
            ((hex >> 8) & 0xff) as f32 / 255.0,
            (hex & 0xff) as f32 / 255.0,
        )
    }

    fn palette(light: bool) -> Palette {
        if light {
            Palette {
                accent: rgb(0xd97706),
                rail: rgb(0x4b6b88),
                halo: rgb(0xe6edf5),
            }
        } else {
            Palette {
                accent: rgb(0xf59e0b),
                rail: rgb(0x8da9bf),
                halo: rgb(0x181b20),
            }
        }
    }

    fn system_uses_light_theme() -> bool {
        let mut data: u32 = 1;
        let mut size = size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut data as *mut u32).cast()),
                Some(&mut size),
            )
        };
        status.is_ok() && data != 0
    }

    fn reduce_motion() -> bool {
        let mut enabled: i32 = 1;
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&mut enabled as *mut i32).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        ok.is_ok() && enabled == 0
    }

    struct DibSurface {
        memdc: HDC,
        dib: HBITMAP,
        bits: *mut u8,
        side: i32,
    }

    impl DibSurface {
        fn new(side: i32) -> Result<Self, String> {
            let side = side.max(1);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: side,
                    biHeight: -side,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            unsafe {
                let dib = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                    .map_err(|e| format!("CreateDIBSection (splash) falhou: {e}"))?;
                let memdc = CreateCompatibleDC(None);
                SelectObject(memdc, HGDIOBJ(dib.0));
                Ok(Self {
                    memdc,
                    dib,
                    bits: bits.cast(),
                    side,
                })
            }
        }

        fn clear(&mut self) {
            let len = (self.side * self.side * 4) as usize;
            unsafe { std::slice::from_raw_parts_mut(self.bits, len).fill(0) };
        }
    }

    impl Drop for DibSurface {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteDC(self.memdc);
                let _ = DeleteObject(HGDIOBJ(self.dib.0));
            }
        }
    }

    struct Splash {
        hwnd: HWND,
        target: ID2D1DCRenderTarget,
        stroke: ID2D1StrokeStyle,
        surface: DibSurface,
        scale: f32,
        origin: (i32, i32),
        palette: Palette,
        reduce_motion: bool,
        started: Instant,
        exit_started: Option<Instant>,
        timed_out: bool,
        on_timeout: Box<dyn Fn()>,
        rail: Vec<anim::Point>,
        signal: Vec<anim::Point>,
    }

    fn color(c: (f32, f32, f32), a: f32) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: c.0,
            g: c.1,
            b: c.2,
            a: a.clamp(0.0, 1.0),
        }
    }

    impl Splash {
        /// Desenha um quadro. Retorna `true` quando a saída terminou.
        fn render(&mut self) -> bool {
            let elapsed = self.started.elapsed().as_secs_f32() * 1000.0;
            let exit_ms = self
                .exit_started
                .map(|t| t.elapsed().as_secs_f32() * 1000.0);
            let f = anim::frame(elapsed, exit_ms, self.reduce_motion);
            let side = self.surface.side;
            let half = side as f32 / 2.0;
            // Unidade do viewBox 48 em pixels físicos, já com a escala do quadro.
            let unit = MARK_LOGICAL * self.scale / 48.0 * f.scale;
            let ox = half - 24.0 * unit;
            let oy = half - 24.0 * unit;
            let map = |p: anim::Point| Vector2 {
                X: ox + p.0 * unit,
                Y: oy + p.1 * unit,
            };

            self.surface.clear();
            unsafe {
                let bind = windows::Win32::Foundation::RECT {
                    left: 0,
                    top: 0,
                    right: side,
                    bottom: side,
                };
                if self.target.BindDC(self.surface.memdc, &bind).is_err() {
                    return false;
                }
                self.target.BeginDraw();
                self.target.Clear(Some(&color((0.0, 0.0, 0.0), 0.0)));

                // Halo: elipses concêntricas de baixa opacidade (queda suave),
                // na cor do fundo do tema para a marca ler em qualquer papel de parede.
                let halo_r = half * 0.98;
                for i in 0..14 {
                    let k = i as f32 / 14.0;
                    let r = halo_r * (1.0 - k * 0.85);
                    if let Ok(b) = self
                        .target
                        .CreateSolidColorBrush(&color(self.palette.halo, 0.07), None)
                    {
                        let e = D2D1_ELLIPSE {
                            point: Vector2 { X: half, Y: half },
                            radiusX: r,
                            radiusY: r,
                        };
                        self.target.FillEllipse(&e, &b);
                    }
                }

                let rail_w = 2.2 * unit;
                let sig_w = 3.4 * unit;
                if let Ok(b) = self
                    .target
                    .CreateSolidColorBrush(&color(self.palette.rail, 1.0), None)
                {
                    let pts = anim::partial(&self.rail, f.rail);
                    for w in pts.windows(2) {
                        self.target
                            .DrawLine(map(w[0]), map(w[1]), &b, rail_w, &self.stroke);
                    }
                }
                let sig = anim::partial(&self.signal, f.signal);
                if let Ok(b) = self
                    .target
                    .CreateSolidColorBrush(&color(self.palette.accent, f.signal_opacity), None)
                {
                    for w in sig.windows(2) {
                        self.target
                            .DrawLine(map(w[0]), map(w[1]), &b, sig_w, &self.stroke);
                    }
                }
                // Brilho na ponta do sinal.
                if f.glow > 0.0 {
                    if let Some(&tip) = sig.last() {
                        let c = map(tip);
                        for i in 0..8 {
                            let r = (1.2 + i as f32 * 0.9) * unit;
                            if let Ok(b) = self.target.CreateSolidColorBrush(
                                &color(self.palette.accent, 0.10 * f.glow),
                                None,
                            ) {
                                let e = D2D1_ELLIPSE {
                                    point: c,
                                    radiusX: r,
                                    radiusY: r,
                                };
                                self.target.FillEllipse(&e, &b);
                            }
                        }
                    }
                }
                let _ = self.target.EndDraw(None, None);
                let _ = GdiFlush();

                let size = SIZE { cx: side, cy: side };
                let pos = POINT {
                    x: self.origin.0,
                    y: self.origin.1,
                };
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: (f.alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let _ = UpdateLayeredWindow(
                    self.hwnd,
                    None,
                    Some(&pos),
                    Some(&size),
                    Some(self.surface.memdc),
                    Some(&POINT::default()),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }
            exit_ms.is_some_and(anim::exit_finished)
        }
    }

    enum Action {
        None,
        Finish,
        Timeout,
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_TIMER => {
                let action = STATE.with(|s| {
                    let Ok(mut guard) = s.try_borrow_mut() else {
                        return Action::None;
                    };
                    let Some(st) = guard.as_mut() else {
                        return Action::None;
                    };
                    if st.render() {
                        return Action::Finish;
                    }
                    if st.exit_started.is_none()
                        && !st.timed_out
                        && st.started.elapsed() >= Duration::from_millis(WATCHDOG_MS)
                    {
                        st.timed_out = true;
                        return Action::Timeout;
                    }
                    Action::None
                });
                match action {
                    Action::Finish => {
                        let _ = unsafe { KillTimer(Some(hwnd), TIMER_ID) };
                        let _ = unsafe { DestroyWindow(hwnd) };
                    }
                    Action::Timeout => STATE.with(|s| {
                        if let Ok(guard) = s.try_borrow() {
                            if let Some(st) = guard.as_ref() {
                                (st.on_timeout)();
                            }
                        }
                    }),
                    Action::None => {}
                }
                LRESULT(0)
            }
            WM_APP_DISMISS => {
                STATE.with(|s| {
                    if let Ok(mut guard) = s.try_borrow_mut() {
                        if let Some(st) = guard.as_mut() {
                            st.exit_started.get_or_insert_with(Instant::now);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    fn primary_monitor_center() -> (i32, i32) {
        unsafe {
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut info).as_bool() {
                let r = info.rcMonitor;
                ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
            } else {
                (960, 540)
            }
        }
    }

    fn run(
        center: Option<(i32, i32)>,
        on_timeout: Box<dyn Fn()>,
        ready: &mpsc::Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let factory: windows::Win32::Graphics::Direct2D::ID2D1Factory = unsafe {
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .map_err(|e| format!("D2D1CreateFactory (splash) falhou: {e}"))?
        };
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let target = unsafe {
            factory
                .CreateDCRenderTarget(&props)
                .map_err(|e| format!("CreateDCRenderTarget (splash) falhou: {e}"))?
        };
        let stroke = unsafe {
            factory
                .CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        dashCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        miterLimit: 10.0,
                        dashStyle: D2D1_DASH_STYLE_SOLID,
                        dashOffset: 0.0,
                    },
                    None,
                )
                .map_err(|e| format!("CreateStrokeStyle (splash) falhou: {e}"))?
        };

        let hinstance =
            unsafe { GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW falhou: {e}"))? };
        let class_name = w!("PulseSplashLayeredClass");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        unsafe { RegisterClassW(&wc) };

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                class_name,
                w!("Pulse"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinstance.into()),
                None,
            )
            .map_err(|e| format!("CreateWindowExW (splash) falhou: {e}"))?
        };

        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        let side = (WINDOW_LOGICAL * scale).round() as i32;
        let (cx, cy) = center.unwrap_or_else(primary_monitor_center);
        let origin = anim::centered_origin(cx, cy, side);
        let surface = DibSurface::new(side)?;

        let mut splash = Splash {
            hwnd,
            target,
            stroke,
            surface,
            scale,
            origin,
            palette: palette(system_uses_light_theme()),
            reduce_motion: reduce_motion(),
            started: Instant::now(),
            exit_started: None,
            timed_out: false,
            on_timeout,
            rail: anim::rail_points(),
            signal: anim::signal_points(),
        };
        // Primeiro quadro (alpha 0) antes de mostrar: nada pisca.
        splash.render();
        STATE.with(|s| *s.borrow_mut() = Some(splash));
        SPLASH_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            SetTimer(Some(hwnd), TIMER_ID, TIMER_MS, None);
        }
        let _ = ready.send(Ok(()));

        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        SPLASH_HWND.store(0, Ordering::SeqCst);
        STATE.with(|s| *s.borrow_mut() = None);
        unsafe {
            let _ = UnregisterClassW(class_name, Some(hinstance.into()));
        }
        Ok(())
    }

    pub fn is_active() -> bool {
        SPLASH_HWND.load(Ordering::SeqCst) != 0
    }

    pub fn dismiss() {
        let raw = SPLASH_HWND.load(Ordering::SeqCst);
        if raw != 0 {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(raw as *mut _)),
                    WM_APP_DISMISS,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    /// Abre a splash. `true` se a janela nativa subiu (a `main` deve esperar
    /// `boot_complete`); `false` mantém o fluxo antigo.
    pub fn show(center: Option<(i32, i32)>, on_timeout: impl Fn() + Send + 'static) -> bool {
        if is_active() {
            return true;
        }
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let handle = thread::Builder::new()
            .name("pulse-splash".into())
            .spawn(move || {
                if let Err(e) = run(center, Box::new(on_timeout), &tx) {
                    let _ = tx.send(Err(e));
                }
            });
        let Ok(handle) = handle else {
            return false;
        };
        let ok = match rx.recv_timeout(Duration::from_millis(1500)) {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                crate::debug::record_failure(
                    "splash.show",
                    0,
                    crate::debug::Failure::new(e).stage("create"),
                );
                false
            }
            Err(_) => false,
        };
        *SPLASH_THREAD
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(handle);
        ok
    }
}

#[cfg(windows)]
pub use win::{dismiss, is_active, show};

#[cfg(not(windows))]
pub fn is_active() -> bool {
    false
}

#[cfg(not(windows))]
pub fn dismiss() {}

#[cfg(not(windows))]
pub fn show(_center: Option<(i32, i32)>, _on_timeout: impl Fn() + Send + 'static) -> bool {
    false
}
