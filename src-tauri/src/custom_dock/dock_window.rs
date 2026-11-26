//! Dock inferior redesenhada estilo macOS (Win32 + `ID2D1DCRenderTarget` + `UpdateLayeredWindow`).
//!
//! Melhorias arquiteturais:
//! - Alfa por pixel (`ULW_ALPHA` / `AC_SRC_ALPHA`): ZERO faixa cinza opaca no topo;
//!   acima da pílula só os ícones ampliados e a pílula flutuante de tooltip têm pixels
//!   opacos — o resto é `alpha = 0` 100% invisível e click-through!
//! - Pílula justa e simétrica em torno dos ícones (Porcelain / Obsidian), com borda
//!   interna luminosa de 1px e sombra suave embaixo.
//! - Escalamento real por DPI (`GetDpiForWindow(hwnd)` / 96.0).
//! - Agrupamento de múltiplas janelas por app + badge + ciclo no clique +
//!   `SwitchToThisWindow(hwnd, true)` (corrige tela branca em apps WinUI 3).
//! - Botão integrado "Arquivos" na ponta esquerda para abrir o módulo `"Arquivos"` do Pulse.
//! - Reserva WorkArea inferior (`ABE_BOTTOM`) quando `dock_auto_hide == false`, e
//!   libera imediatamente (`ABM_REMOVE`) quando `dock_auto_hide == true`.

use super::{
    appbar::{AppBarEdge, AppBarSpace},
    icons::{create_pulse_files_icon, d2d_bitmap_from_icon, load_high_res_icon, IconImage},
    shell_log,
    topbar::resolve_host_exe_path,
    DockAppCandidate,
};
use crate::beta_features::BetaFeaturesConfig;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            COLORREF, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WAIT_OBJECT_0, WPARAM,
        },
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
                },
                D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_CLIP,
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT,
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
            },
            DirectWrite::{
                DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
            },
            Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush,
                GetMonitorInfoW, MonitorFromPoint, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER,
                BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
                HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
            },
        },
        System::{
            Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            LibraryLoader::GetModuleHandleW,
            Threading::{
                AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
                PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
            },
            Shell::{
                PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
                ShellExecuteW,
            },
            WindowsAndMessaging::{
                AppendMenuW, BringWindowToTop, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
                DestroyMenu, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW,
                GetCursorPos, GetForegroundWindow, GetMessageW, GetWindow, GetWindowLongPtrW,
                GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
                IsIconic, IsWindow, IsWindowVisible, KillTimer, LoadCursorW,
                MsgWaitForMultipleObjectsEx, PeekMessageW, PostMessageW, PostQuitMessage,
                RegisterClassW, ReplyMessage, SetForegroundWindow, SetTimer, SetWindowLongPtrW,
                SetWindowPos, ShowWindow, SwitchToThisWindow, TrackPopupMenu, TranslateMessage,
                UnregisterClassW, CS_HREDRAW, CS_VREDRAW, EVENT_OBJECT_CREATE,
                EVENT_OBJECT_DESTROY, EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_FOREGROUND,
                GWLP_USERDATA, GWL_EXSTYLE, GW_OWNER, HWND_TOPMOST, IDC_ARROW, MF_CHECKED,
                MF_SEPARATOR, MF_STRING, MSG, MWMO_INPUTAVAILABLE, PM_NOREMOVE, QS_SENDMESSAGE,
                SC_MINIMIZE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SW_HIDE,
                SW_MINIMIZE, SW_SHOWNA, SW_SHOWNORMAL, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
                TPM_RETURNCMD, ULW_ALPHA, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP,
                WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL,
                WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SYSCOMMAND, WM_TIMER, WNDCLASSW,
                WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_POPUP,
            },
        },
    },
};

const WM_MOUSELEAVE: u32 = 0x02A3;
const MA_NOACTIVATE: isize = 3;

const WM_APP_APPLY_CONFIG: u32 = WM_APP + 21;
const WM_APP_REFRESH_ITEMS: u32 = WM_APP + 22;
const WM_APP_SHUTDOWN: u32 = WM_APP + 23;
const WM_APP_RECHECK_APPBAR: u32 = WM_APP + 24;
const WM_APP_REVEAL: u32 = WM_APP + 25;
const WM_APP_HOLD: u32 = WM_APP + 26;

/// Tempo em que a Dock permanece visível após um toque na tecla Windows,
/// mesmo com auto-hide ligado ou com um jogo em tela cheia.
const HOTKEY_REVEAL_MS: u64 = 2500;

const TIMER_RECONCILE_ID: usize = 1;
const TIMER_RECONCILE_MS: u32 = 1100;
const TIMER_AUTOHIDE_ID: usize = 2;
const TIMER_AUTOHIDE_MS: u32 = 120;
const TIMER_ANIM_ID: usize = 3;
const TIMER_ANIM_MS: u32 = 16;

const MENU_NEW_WINDOW: u32 = 1001;
const MENU_PIN_TOGGLE: u32 = 1002;
const MENU_CLOSE_WINDOWS: u32 = 1003;
const MENU_OPEN_FILES: u32 = 1004;
const MENU_OPEN_OVERVIEW: u32 = 1005;
const MENU_OPEN_SETTINGS: u32 = 1006;
const MENU_RESTORE_TASKBAR: u32 = 1007;
const MENU_FOCUS_WINDOW_BASE: u32 = 1100;

const APPS_FOLDER: &str = "shell:AppsFolder\\";
const PULSE_FILES_KEY: &str = "pulse://files";

/// PKEY_AppUserModel_ID (`{9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}`, 5)
const PKEY_AUMID: windows::Win32::Foundation::PROPERTYKEY =
    windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };

static DOCK_HWND: AtomicIsize = AtomicIsize::new(0);
static DOCK_RUNNING: AtomicBool = AtomicBool::new(false);
static ITEM_COUNT: AtomicUsize = AtomicUsize::new(0);
static DOCK_THREAD: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
static DOCK_CONFIG: OnceLock<Mutex<BetaFeaturesConfig>> = OnceLock::new();

fn thread_slot() -> &'static Mutex<Option<JoinHandle<()>>> {
    DOCK_THREAD.get_or_init(|| Mutex::new(None))
}

fn config_slot() -> &'static Mutex<BetaFeaturesConfig> {
    DOCK_CONFIG.get_or_init(|| Mutex::new(BetaFeaturesConfig::default()))
}

pub fn is_running() -> bool {
    DOCK_RUNNING.load(Ordering::SeqCst)
}

pub fn item_count() -> usize {
    ITEM_COUNT.load(Ordering::SeqCst)
}

/// Re-alinha a AppBar inferior após a barra nativa do Windows ser ocultada
/// (elimina o deslocamento residual de 48px no primeiro boot do Pulse Shell).
pub fn recheck_appbar() {
    let raw = DOCK_HWND.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(raw as *mut _)),
                WM_APP_RECHECK_APPBAR,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Revela a Dock por alguns segundos quando o Pulse Runner é aberto pela tecla
/// Windows. Não bloqueia: apenas posta uma mensagem na thread da Dock.
pub fn reveal_from_shortcut() {
    let raw = DOCK_HWND.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(raw as *mut _)),
                WM_APP_REVEAL,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Mantém a Dock visível enquanto o Pulse Runner estiver aberto.
pub fn hold_from_shortcut(hold: bool) {
    let raw = DOCK_HWND.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(raw as *mut _)),
                WM_APP_HOLD,
                WPARAM(usize::from(hold)),
                LPARAM(0),
            );
        }
    }
}

pub fn start(config: &BetaFeaturesConfig) -> Result<(), String> {
    if let Ok(mut cfg) = config_slot().lock() {
        *cfg = config.clone();
    }
    let mut guard = thread_slot().lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() && DOCK_RUNNING.load(Ordering::SeqCst) {
        let raw = DOCK_HWND.load(Ordering::SeqCst);
        if raw != 0 {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(raw as *mut _)),
                    WM_APP_APPLY_CONFIG,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        return Ok(());
    }
    if let Some(old) = guard.take() {
        let _ = old.join();
    }

    let (ready_tx, ready_rx) = mpsc::channel();
    let initial = config.clone();
    let handle = thread::Builder::new()
        .name("pulse-custom-dock".to_owned())
        .spawn(move || run_dock_thread(initial, ready_tx))
        .map_err(|e| format!("Falha ao iniciar a thread da Dock: {e}"))?;

    match ready_rx.recv() {
        Ok(Ok(())) => {
            *guard = Some(handle);
            Ok(())
        }
        Ok(Err(err)) => {
            let _ = handle.join();
            Err(err)
        }
        Err(_) => {
            let _ = handle.join();
            Err("A thread da Dock encerrou antes de inicializar.".to_owned())
        }
    }
}

pub fn stop() {
    let taken = thread_slot().lock().ok().and_then(|mut g| g.take());
    let raw = DOCK_HWND.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(raw as *mut _)),
                WM_APP_SHUTDOWN,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
    if let Some(handle) = taken {
        wait_thread_pumping_messages(handle);
    }
    DOCK_RUNNING.store(false, Ordering::SeqCst);
    DOCK_HWND.store(0, Ordering::SeqCst);
    ITEM_COUNT.store(0, Ordering::SeqCst);
}

fn wait_thread_pumping_messages(handle: JoinHandle<()>) {
    use std::os::windows::io::AsRawHandle;
    let raw_handle = [HANDLE(handle.as_raw_handle())];
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u32;
        if left == 0 {
            break;
        }
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&raw_handle),
                left,
                QS_SENDMESSAGE,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait.0 != WAIT_OBJECT_0.0 + 1 {
            break;
        }
        unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        }
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}

pub fn list_candidates() -> Vec<DockAppCandidate> {
    let config = crate::beta_features::load();
    let windows = enumerate_top_windows(HWND(std::ptr::null_mut()));
    let mut seen: HashMap<String, DockAppCandidate> = HashMap::new();

    for pinned in &config.dock_pinned_apps {
        let key = pinned.to_lowercase();
        let name = display_name_from_source(pinned);
        seen.insert(
            key,
            DockAppCandidate {
                name,
                exe_path: pinned.clone(),
                is_pinned: true,
                is_running: false,
            },
        );
    }

    for win in windows {
        let Some(src) = win.source else {
            continue;
        };
        let key = win.app_key.clone();
        if let Some(existing) = seen.get_mut(&key) {
            existing.is_running = true;
            if existing.name.is_empty() {
                existing.name = win.title;
            }
        } else {
            seen.insert(
                key,
                DockAppCandidate {
                    name: display_name_from_source(&src),
                    exe_path: src,
                    is_pinned: false,
                    is_running: true,
                },
            );
        }
    }

    let mut list: Vec<DockAppCandidate> = seen.into_values().collect();
    list.sort_by(|a, b| {
        b.is_pinned
            .cmp(&a.is_pinned)
            .then_with(|| b.is_running.cmp(&a.is_running))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    list
}

#[derive(Clone)]
struct RunningWindow {
    hwnd: HWND,
    title: String,
    app_key: String,
    source: Option<String>,
    is_foreground: bool,
}

#[derive(Clone)]
struct DockItem {
    key: String,
    label: String,
    source: Option<String>,
    windows: Vec<HWND>,
    pinned: bool,
    is_pulse_files: bool,
    active: bool,
    rect: D2D_RECT_F,
    hit_left: f32,
    hit_right: f32,
}

struct DibSurface {
    memdc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl DibSurface {
    fn new(width: i32, height: i32) -> Result<Self, String> {
        let w = width.max(1);
        let h = height.max(1);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
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
                .map_err(|e| format!("CreateDIBSection (Dock) falhou: {e}"))?;
            let memdc = CreateCompatibleDC(None);
            SelectObject(memdc, HGDIOBJ(dib.0));
            Ok(Self {
                memdc,
                dib,
                bits: bits.cast(),
                width: w,
                height: h,
            })
        }
    }

    fn clear_bits(&mut self) {
        let len = (self.width * self.height * 4) as usize;
        unsafe {
            std::slice::from_raw_parts_mut(self.bits, len).fill(0);
        }
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

struct DockMetrics {
    base_icon: f32,
    max_icon: f32,
    gap: f32,
    pad_x: f32,
    pad_top: f32,
    pad_bottom: f32,
    sep_width: f32,
    bottom_margin: f32,
    shadow_pad: f32,
    tooltip_space: f32,
    pill_radius: f32,
}

impl DockMetrics {
    fn from_config(config: &BetaFeaturesConfig, scale: f32) -> Self {
        let base_icon = config.dock_icon_size as f32 * scale;
        let zoom = if config.dock_magnification { 1.35 } else { 1.0 };
        Self {
            base_icon,
            max_icon: base_icon * zoom,
            gap: 9.0 * scale,
            pad_x: 12.0 * scale,
            pad_top: 8.0 * scale,
            pad_bottom: 10.0 * scale,
            sep_width: 14.0 * scale,
            bottom_margin: 6.0 * scale,
            shadow_pad: 10.0 * scale,
            tooltip_space: 36.0 * scale,
            pill_radius: 18.0 * scale,
        }
    }

    fn pill_height(&self) -> f32 {
        self.pad_top + self.base_icon + self.pad_bottom
    }

    /// Espaço que a AppBar inferior deve reservar quando `dock_auto_hide == false`.
    fn reserved_workarea_thickness(&self, scale: f32) -> i32 {
        (self.pill_height() + self.bottom_margin + 2.0 * scale).round() as i32
    }
}

struct DockState {
    hwnd: HWND,
    config: BetaFeaturesConfig,
    scale: f32,
    monitor: RECT,
    appbar_space: Option<AppBarSpace>,
    dc_target: ID2D1DCRenderTarget,
    dwrite: IDWriteFactory,
    tooltip_format: IDWriteTextFormat,
    badge_format: IDWriteTextFormat,
    initial_format: IDWriteTextFormat,
    surface: DibSurface,
    icon_cache: HashMap<String, Option<ID2D1Bitmap>>,
    cached_icon_px: u32,
    items: Vec<DockItem>,
    hovered_index: Option<usize>,
    mouse_x_target: Option<f32>,
    mouse_x_smooth: f32,
    hover_intensity: f32,
    hover_target: f32,
    animating: bool,
    last_anim_tick: Instant,
    tracking_mouse: bool,
    hidden_by_autohide: bool,
    /// Um jogo em tela cheia não esconde mais a Dock: ela passa a operar em
    /// overlay (só aparece no hot corner / quando revelada pelo atalho).
    fullscreen_active: bool,
    /// Protege a Dock do auto-hide enquanto o popup nativo está aberto.
    menu_open: bool,
    /// Reveal temporário disparado pela tecla Windows.
    reveal_until: Option<Instant>,
    /// Mantém a Dock visível enquanto o Pulse Runner estiver aberto.
    hold_from_launcher: bool,
    /// Menu Iniciar nativo em primeiro plano (runner pausado por jogo
    /// protegido ou desligado): a Dock aparece junto, sem hook de teclado.
    start_menu_open: bool,
    /// Cache da última janela em foco verificada: (hwnd, é o Iniciar?).
    start_probe: (isize, bool),
    /// Arraste de ícones com botão esquerdo: (chave do app, fixado?, x inicial).
    /// Guarda a chave em vez do índice para sobreviver ao reconcile de 1,1s.
    drag_source: Option<(String, bool, f32)>,
    dragging: bool,
    drag_x: f32,
    win_x: i32,
    win_y: i32,
    pill_rect: D2D_RECT_F,
    first_render_logged: bool,
}

impl DockState {
    fn sync_appbar(&mut self) {
        let m = DockMetrics::from_config(&self.config, self.scale);
        if self.config.dock_auto_hide {
            if let Some(space) = self.appbar_space.take() {
                space.close();
            }
        } else {
            let thickness = m.reserved_workarea_thickness(self.scale);
            if let Some(ref space) = self.appbar_space {
                space.set_thickness(thickness);
            } else {
                match AppBarSpace::start(AppBarEdge::Bottom, thickness) {
                    Ok(space) => self.appbar_space = Some(space),
                    Err(err) => shell_log::error(
                        "dock_appbar",
                        "Falha ao iniciar AppBarSpace(bottom)",
                        &err,
                    ),
                }
            }
        }
    }

    fn update_fonts(&mut self) {
        if let Ok((tf, bf, inf)) = create_dock_fonts(&self.dwrite, self.scale) {
            self.tooltip_format = tf;
            self.badge_format = bf;
            self.initial_format = inf;
        }
    }

    fn rebuild_items(&mut self) {
        let running = enumerate_top_windows(self.hwnd);
        let mut grouped: HashMap<String, (Option<String>, Vec<RunningWindow>)> = HashMap::new();
        let mut seen_order: Vec<String> = Vec::new();

        for win in running {
            let entry = grouped.entry(win.app_key.clone()).or_insert_with(|| {
                seen_order.push(win.app_key.clone());
                (win.source.clone(), Vec::new())
            });
            if entry.0.is_none() && win.source.is_some() {
                entry.0 = win.source.clone();
            }
            entry.1.push(win);
        }

        // Preserva ordem estável de HWNDs já conhecidos para cada app (clique cicla previsivelmente)
        let mut prev_order_by_key: HashMap<String, Vec<HWND>> = HashMap::new();
        for item in &self.items {
            prev_order_by_key.insert(item.key.clone(), item.windows.clone());
        }

        let stabilize_windows =
            |key: &str, wins: Vec<RunningWindow>| -> (Vec<HWND>, bool, String) {
                let active = wins.iter().any(|w| w.is_foreground);
                let fg_title = wins
                    .iter()
                    .find(|w| w.is_foreground)
                    .or_else(|| wins.first())
                    .map(|w| w.title.clone())
                    .unwrap_or_default();
                let current_hwnds: Vec<HWND> = wins.iter().map(|w| w.hwnd).collect();
                let mut stable: Vec<HWND> = prev_order_by_key
                    .get(key)
                    .map(|old| {
                        old.iter()
                            .copied()
                            .filter(|h| current_hwnds.contains(h))
                            .collect()
                    })
                    .unwrap_or_default();
                for h in current_hwnds {
                    if !stable.contains(&h) {
                        stable.push(h);
                    }
                }
                (stable, active, fg_title)
            };

        let mut items = Vec::new();
        let mut consumed = HashSet::new();

        // 0. Botão integrado do Explorador de Arquivos do Pulse (slot Finder estilo macOS)
        items.push(DockItem {
            key: PULSE_FILES_KEY.to_owned(),
            label: "Arquivos (Pulse)".to_owned(),
            source: None,
            windows: Vec::new(),
            pinned: true,
            is_pulse_files: true,
            active: false,
            rect: D2D_RECT_F::default(),
            hit_left: 0.0,
            hit_right: 0.0,
        });

        // 1. Apps fixados pelo usuário na ordem salva
        for pinned_src in &self.config.dock_pinned_apps {
            let key = source_key(pinned_src);
            consumed.insert(key.clone());
            let (windows, active, fg_title) = if let Some((_, wins)) = grouped.remove(&key) {
                stabilize_windows(&key, wins)
            } else {
                (Vec::new(), false, String::new())
            };
            let base_name = display_name_from_source(pinned_src);
            let label = if base_name.is_empty() && !fg_title.is_empty() {
                fg_title
            } else {
                base_name
            };
            items.push(DockItem {
                key,
                label,
                source: Some(pinned_src.clone()),
                windows,
                pinned: true,
                is_pulse_files: false,
                active,
                rect: D2D_RECT_F::default(),
                hit_left: 0.0,
                hit_right: 0.0,
            });
        }

        // 2. Apps abertos não fixados. A ordem salva por arraste tem prioridade;
        //    chaves ainda não salvas mantêm a ordem de enumeração (stable sort).
        let saved_unpinned_order = self.config.dock_unpinned_order.clone();
        let mut unpinned_keys: Vec<String> = seen_order
            .into_iter()
            .filter(|key| !consumed.contains(key))
            .collect();
        unpinned_keys.sort_by_key(|key| {
            saved_unpinned_order
                .iter()
                .position(|saved| saved == key)
                .unwrap_or(usize::MAX)
        });
        for key in unpinned_keys {
            let Some((src, wins)) = grouped.remove(&key) else {
                continue;
            };
            let (windows, active, fg_title) = stabilize_windows(&key, wins);
            let label = src
                .as_deref()
                .map(|source| display_name_from_source_with_title(source, &fg_title))
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("ApplicationFrameHost"))
                .unwrap_or_else(|| fg_title.chars().take(28).collect());
            items.push(DockItem {
                key,
                label,
                source: src,
                windows,
                pinned: false,
                is_pulse_files: false,
                active,
                rect: D2D_RECT_F::default(),
                hit_left: 0.0,
                hit_right: 0.0,
            });
        }

        self.items = items;
        ITEM_COUNT.store(self.items.len(), Ordering::SeqCst);
        self.reposition_window();
    }

    fn has_separator(&self) -> bool {
        let pinned_count = self.items.iter().filter(|i| i.pinned).count();
        pinned_count > 0 && pinned_count < self.items.len()
    }

    fn compute_layout(&mut self) -> (i32, i32) {
        let m = DockMetrics::from_config(&self.config, self.scale);
        let n = self.items.len();
        let has_sep = self.has_separator();

        // Largura fixa em repouso + folga lateral para expansão suave de zoom e sombra
        let rest_icons_w = if n == 0 {
            160.0 * self.scale
        } else {
            m.pad_x * 2.0
                + n as f32 * m.base_icon
                + n.saturating_sub(1) as f32 * m.gap
                + if has_sep { m.sep_width } else { 0.0 }
        };
        let zoom_extra_w = if self.config.dock_magnification {
            (m.max_icon - m.base_icon) * 2.6
        } else {
            0.0
        };
        let total_w = rest_icons_w + zoom_extra_w + m.shadow_pad * 2.0;
        let top_headroom = (m.max_icon - m.base_icon) + m.tooltip_space;
        let total_h = top_headroom + m.pill_height() + m.shadow_pad;

        let base_left = (total_w - rest_icons_w) * 0.5 + m.pad_x;
        let pinned_exists = self.items.iter().any(|i| i.pinned);

        // Calcula tamanho de cada slot com base em `mouse_x_smooth` e `hover_intensity`
        let mut sizes: Vec<f32> = Vec::with_capacity(n);
        let mut cursor_base = base_left;
        let mut saw_unpinned = false;
        let range = 2.1 * (m.base_icon + m.gap);

        for item in &self.items {
            if pinned_exists && !item.pinned && !saw_unpinned {
                cursor_base += m.sep_width;
                saw_unpinned = true;
            }
            let center_base = cursor_base + m.base_icon * 0.5;
            let scale_up = if self.config.dock_magnification && self.hover_intensity > 0.001 {
                let d = (self.mouse_x_smooth - center_base).abs();
                if d < range {
                    let f = 0.5 * (1.0 + (std::f32::consts::PI * d / range).cos());
                    m.base_icon + (m.max_icon - m.base_icon) * f * self.hover_intensity
                } else {
                    m.base_icon
                }
            } else {
                m.base_icon
            };
            sizes.push(scale_up);
            cursor_base += m.base_icon + m.gap;
        }

        let drawn_total_w: f32 = sizes.iter().sum::<f32>()
            + n.saturating_sub(1) as f32 * m.gap
            + if has_sep { m.sep_width } else { 0.0 };

        let pill_left = (total_w - drawn_total_w) * 0.5 - m.pad_x;
        let pill_right = pill_left + drawn_total_w + m.pad_x * 2.0;
        let pill_top = top_headroom;
        let pill_bottom = top_headroom + m.pill_height();
        self.pill_rect = D2D_RECT_F {
            left: pill_left,
            top: pill_top,
            right: pill_right,
            bottom: pill_bottom,
        };

        let icon_bottom = pill_bottom - m.pad_bottom;
        let mut x = pill_left + m.pad_x;
        let mut saw_unpinned = false;
        let mut hovered = None;

        for (idx, (item, size)) in self.items.iter_mut().zip(sizes).enumerate() {
            if pinned_exists && !item.pinned && !saw_unpinned {
                x += m.sep_width;
                saw_unpinned = true;
            }
            let top = icon_bottom - size;
            item.rect = D2D_RECT_F {
                left: x,
                top,
                right: x + size,
                bottom: icon_bottom,
            };
            item.hit_left = x - m.gap * 0.5;
            item.hit_right = x + size + m.gap * 0.5;

            if let Some(mx) = self.mouse_x_target {
                if mx >= item.hit_left && mx < item.hit_right {
                    hovered = Some(idx);
                }
            }
            x += size + m.gap;
        }

        self.hovered_index = hovered;
        (total_w.ceil() as i32, total_h.ceil() as i32)
    }

    fn reposition_window(&mut self) {
        let mon = primary_monitor_rect();
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        if (scale - self.scale).abs() > 0.01 {
            self.scale = scale;
            self.update_fonts();
        }
        self.monitor = mon;

        let (width, height) = self.compute_layout();
        if width != self.surface.width || height != self.surface.height {
            if let Ok(new_surface) = DibSurface::new(width, height) {
                self.surface = new_surface;
            }
        }

        let m = DockMetrics::from_config(&self.config, self.scale);
        let mon_w = mon.right - mon.left;
        self.win_x = mon.left + (mon_w - width) / 2;
        self.win_y =
            mon.bottom - height - m.bottom_margin.round() as i32 + m.shadow_pad.round() as i32;

        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                self.win_x,
                self.win_y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            if self.hidden_by_autohide {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_SHOWNA);
            }
        }
    }

    fn ensure_icon_bitmap(&mut self, item: &DockItem) -> Option<ID2D1Bitmap> {
        let m = DockMetrics::from_config(&self.config, self.scale);
        let target_px = (m.max_icon.ceil() as u32).clamp(48, 128);
        if self.cached_icon_px != target_px {
            self.icon_cache.clear();
            self.cached_icon_px = target_px;
        }
        if let Some(cached) = self.icon_cache.get(&item.key) {
            if cached.is_some() || item.windows.is_empty() {
                return cached.clone();
            }
        }

        let img: Option<IconImage> = if item.is_pulse_files {
            Some(create_pulse_files_icon(target_px))
        } else {
            load_high_res_icon(
                item.windows.first().copied(),
                item.source.as_deref(),
                target_px,
            )
        };
        let bmp = img.and_then(|im| d2d_bitmap_from_icon(&self.dc_target, &im));
        self.icon_cache.insert(item.key.clone(), bmp.clone());
        bmp
    }

    fn render(&mut self) {
        if self.hidden_by_autohide {
            return;
        }
        self.compute_layout();

        let items_snapshot = self.items.clone();
        let bitmaps: Vec<Option<ID2D1Bitmap>> = items_snapshot
            .iter()
            .map(|it| self.ensure_icon_bitmap(it))
            .collect();

        let dark = self.config.dock_theme == "dark";
        let s = self.scale;
        let m = DockMetrics::from_config(&self.config, s);
        let pill = self.pill_rect;

        let pill_fill = if dark {
            rgba(0.115, 0.135, 0.168, 0.86)
        } else {
            rgba(0.955, 0.968, 0.985, 0.86)
        };
        let inner_highlight = if dark {
            rgba(1.0, 1.0, 1.0, 0.14)
        } else {
            rgba(1.0, 1.0, 1.0, 0.72)
        };
        let outer_border = if dark {
            rgba(0.0, 0.0, 0.0, 0.42)
        } else {
            rgba(0.15, 0.22, 0.30, 0.15)
        };
        let divider_color = if dark {
            rgba(1.0, 1.0, 1.0, 0.18)
        } else {
            rgba(0.16, 0.23, 0.31, 0.18)
        };
        let slot_fallback = if dark {
            rgba(0.20, 0.24, 0.30, 0.92)
        } else {
            rgba(0.84, 0.88, 0.93, 0.92)
        };
        let tooltip_bg = if dark {
            rgba(0.11, 0.13, 0.16, 0.95)
        } else {
            rgba(0.98, 0.99, 1.0, 0.96)
        };
        let tooltip_border = if dark {
            rgba(1.0, 1.0, 1.0, 0.16)
        } else {
            rgba(0.15, 0.22, 0.30, 0.16)
        };
        let text_color = if dark {
            rgba(0.94, 0.96, 0.99, 1.0)
        } else {
            rgba(0.11, 0.16, 0.22, 1.0)
        };
        let accent = rgba(0.94, 0.56, 0.07, 1.0); // #ef8f12
        let dot_idle = if dark {
            rgba(0.84, 0.89, 0.95, 0.65)
        } else {
            rgba(0.22, 0.29, 0.38, 0.62)
        };

        self.surface.clear_bits();
        let bind_rc = RECT {
            left: 0,
            top: 0,
            right: self.surface.width,
            bottom: self.surface.height,
        };

        unsafe {
            if let Err(err) = self.dc_target.BindDC(self.surface.memdc, &bind_rc) {
                shell_log::error("dock_render", "BindDC falhou na Dock", format!("{err}"));
                return;
            }
            self.dc_target.BeginDraw();
            self.dc_target.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));

            // Sombra suave embaixo da pílula (camadas suaves sem hard-cut)
            for (spread, y_off, alpha) in [
                (6.0 * s, 4.0 * s, 0.06),
                (3.5 * s, 2.5 * s, 0.08),
                (1.5 * s, 1.5 * s, 0.09),
            ] {
                if let Ok(sh_brush) = self
                    .dc_target
                    .CreateSolidColorBrush(&rgba(0.0, 0.0, 0.0, alpha), None)
                {
                    let sr = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: pill.left - spread,
                            top: pill.top - spread * 0.25 + y_off,
                            right: pill.right + spread,
                            bottom: pill.bottom + spread * 0.65 + y_off,
                        },
                        radiusX: m.pill_radius + spread * 0.5,
                        radiusY: m.pill_radius + spread * 0.5,
                    };
                    self.dc_target.FillRoundedRectangle(&sr, &sh_brush);
                }
            }

            if let (
                Ok(pill_brush),
                Ok(inner_brush),
                Ok(outer_brush),
                Ok(div_brush),
                Ok(slot_brush),
                Ok(tt_bg_brush),
                Ok(tt_border_brush),
                Ok(text_brush),
                Ok(accent_brush),
                Ok(dot_brush),
                Ok(badge_bg_brush),
                Ok(badge_fg_brush),
            ) = (
                self.dc_target.CreateSolidColorBrush(&pill_fill, None),
                self.dc_target.CreateSolidColorBrush(&inner_highlight, None),
                self.dc_target.CreateSolidColorBrush(&outer_border, None),
                self.dc_target.CreateSolidColorBrush(&divider_color, None),
                self.dc_target.CreateSolidColorBrush(&slot_fallback, None),
                self.dc_target.CreateSolidColorBrush(&tooltip_bg, None),
                self.dc_target.CreateSolidColorBrush(&tooltip_border, None),
                self.dc_target.CreateSolidColorBrush(&text_color, None),
                self.dc_target.CreateSolidColorBrush(&accent, None),
                self.dc_target.CreateSolidColorBrush(&dot_idle, None),
                self.dc_target
                    .CreateSolidColorBrush(&rgba(0.94, 0.56, 0.07, 0.94), None),
                self.dc_target
                    .CreateSolidColorBrush(&rgba(1.0, 1.0, 1.0, 1.0), None),
            ) {
                // Pílula principal justa aos ícones
                let outer_rr = D2D1_ROUNDED_RECT {
                    rect: pill,
                    radiusX: m.pill_radius,
                    radiusY: m.pill_radius,
                };
                self.dc_target.FillRoundedRectangle(&outer_rr, &pill_brush);
                self.dc_target
                    .DrawRoundedRectangle(&outer_rr, &outer_brush, 1.0, None);

                let inner_rr = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pill.left + 1.0,
                        top: pill.top + 1.0,
                        right: pill.right - 1.0,
                        bottom: pill.bottom - 1.0,
                    },
                    radiusX: (m.pill_radius - 1.0).max(2.0),
                    radiusY: (m.pill_radius - 1.0).max(2.0),
                };
                self.dc_target
                    .DrawRoundedRectangle(&inner_rr, &inner_brush, 1.0, None);

                let has_sep = self.has_separator();
                let pinned_count = items_snapshot.iter().filter(|i| i.pinned).count();

                for (idx, item) in items_snapshot.iter().enumerate() {
                    if has_sep && idx == pinned_count {
                        let sep_x = item.rect.left - m.sep_width * 0.5 - m.gap * 0.45;
                        let sep_rect = D2D_RECT_F {
                            left: sep_x - 0.5,
                            top: pill.top + m.pad_top * 1.1,
                            right: sep_x + 0.5,
                            bottom: pill.bottom - m.pad_bottom * 0.9,
                        };
                        self.dc_target.FillRectangle(&sep_rect, &div_brush);
                    }

                    if let Some(ref bmp) = bitmaps[idx] {
                        self.dc_target.DrawBitmap(
                            bmp,
                            Some(&item.rect),
                            1.0,
                            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                            None,
                        );
                    } else {
                        let fallback_rr = D2D1_ROUNDED_RECT {
                            rect: item.rect,
                            radiusX: 10.0 * s,
                            radiusY: 10.0 * s,
                        };
                        self.dc_target
                            .FillRoundedRectangle(&fallback_rr, &slot_brush);
                        let initial = item
                            .label
                            .chars()
                            .next()
                            .unwrap_or('?')
                            .to_uppercase()
                            .to_string();
                        let wide: Vec<u16> = initial.encode_utf16().collect();
                        self.dc_target.DrawText(
                            &wide,
                            &self.initial_format,
                            &item.rect,
                            &text_brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }

                    // Badge numérico discreto no canto superior direito quando >1 janela aberta
                    if item.windows.len() > 1 {
                        let badge_txt = item.windows.len().min(99).to_string();
                        let badge_sz = 15.0 * s;
                        let badge_rect = D2D_RECT_F {
                            left: item.rect.right - badge_sz + 1.0 * s,
                            top: item.rect.top - 1.0 * s,
                            right: item.rect.right + 1.0 * s,
                            bottom: item.rect.top + badge_sz - 1.0 * s,
                        };
                        let badge_rr = D2D1_ROUNDED_RECT {
                            rect: badge_rect,
                            radiusX: badge_sz * 0.5,
                            radiusY: badge_sz * 0.5,
                        };
                        self.dc_target
                            .FillRoundedRectangle(&badge_rr, &badge_bg_brush);
                        let wide: Vec<u16> = badge_txt.encode_utf16().collect();
                        self.dc_target.DrawText(
                            &wide,
                            &self.badge_format,
                            &badge_rect,
                            &badge_fg_brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }

                    // Ponto indicador estilo macOS abaixo de apps em execução
                    if !item.windows.is_empty() {
                        let cx = (item.rect.left + item.rect.right) * 0.5;
                        let dot_w = if item.active {
                            11.0 * s
                        } else if item.windows.len() > 1 {
                            7.5 * s
                        } else {
                            4.5 * s
                        };
                        let dot_h = 3.5 * s;
                        let dot_y = pill.bottom - m.pad_bottom * 0.55 - dot_h * 0.5;
                        let dot_rr = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: cx - dot_w * 0.5,
                                top: dot_y,
                                right: cx + dot_w * 0.5,
                                bottom: dot_y + dot_h,
                            },
                            radiusX: dot_h * 0.5,
                            radiusY: dot_h * 0.5,
                        };
                        let brush = if item.active {
                            &accent_brush
                        } else {
                            &dot_brush
                        };
                        self.dc_target.FillRoundedRectangle(&dot_rr, brush);
                    }
                }

                // Indicador de soltura do arraste (mesma linguagem visual da Barra Superior).
                if self.dragging {
                    let gx = self.drag_x.clamp(pill.left + 2.0 * s, pill.right - 2.0 * s);
                    self.dc_target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: gx - 1.5 * s,
                                top: pill.top + 4.0 * s,
                                right: gx + 1.5 * s,
                                bottom: pill.bottom - 4.0 * s,
                            },
                            radiusX: 1.5 * s,
                            radiusY: 1.5 * s,
                        },
                        &accent_brush,
                    );
                }

                // Pílula flutuante de tooltip acima do ícone em foco (ao redor é alpha = 0!)
                if let Some(hovered_idx) = self.hovered_index {
                    if let Some(item) = items_snapshot.get(hovered_idx) {
                        let tip = if item.windows.len() > 1 {
                            format!("{} ({})", item.label, item.windows.len())
                        } else {
                            item.label.clone()
                        };
                        let wide: Vec<u16> = tip.encode_utf16().collect();
                        let text_w = self.measure_tooltip_width(&wide).min(240.0 * s);
                        let tip_w = text_w + 20.0 * s;
                        let tip_h = 24.0 * s;
                        let cx = (item.rect.left + item.rect.right) * 0.5;
                        let mut left = cx - tip_w * 0.5;
                        left = left.clamp(
                            4.0 * s,
                            (self.surface.width as f32 - tip_w - 4.0 * s).max(4.0 * s),
                        );
                        let bottom = (item.rect.top - 7.0 * s).max(tip_h + 2.0 * s);
                        let top = bottom - tip_h;

                        let tip_rect = D2D_RECT_F {
                            left,
                            top,
                            right: left + tip_w,
                            bottom,
                        };
                        let tip_rr = D2D1_ROUNDED_RECT {
                            rect: tip_rect,
                            radiusX: 10.0 * s,
                            radiusY: 10.0 * s,
                        };
                        self.dc_target.FillRoundedRectangle(&tip_rr, &tt_bg_brush);
                        self.dc_target
                            .DrawRoundedRectangle(&tip_rr, &tt_border_brush, 1.0, None);
                        self.dc_target.DrawText(
                            &wide,
                            &self.tooltip_format,
                            &tip_rect,
                            &text_brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
            }

            if let Err(err) = self.dc_target.EndDraw(None, None) {
                shell_log::error("dock_render", "EndDraw falhou na Dock", format!("{err}"));
            }
            let _ = GdiFlush();

            let size = SIZE {
                cx: self.surface.width,
                cy: self.surface.height,
            };
            let pos = POINT {
                x: self.win_x,
                y: self.win_y,
            };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let ulw = windows::Win32::UI::WindowsAndMessaging::UpdateLayeredWindow(
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
            if let Err(err) = ulw {
                shell_log::error(
                    "dock_render",
                    "UpdateLayeredWindow falhou na Dock",
                    format!(
                        "{err} pos=({},{}) size={}x{}",
                        pos.x, pos.y, size.cx, size.cy
                    ),
                );
            } else if !self.first_render_logged {
                self.first_render_logged = true;
                shell_log::info(
                    "dock_render",
                    format!(
                        "Primeiro frame da Dock renderizado {}x{} pos=({},{}) scale={:.2} itens={}",
                        size.cx,
                        size.cy,
                        pos.x,
                        pos.y,
                        self.scale,
                        items_snapshot.len()
                    ),
                );
            }
        }
    }

    fn measure_tooltip_width(&self, wide: &[u16]) -> f32 {
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(
                wide,
                &self.tooltip_format,
                320.0 * self.scale,
                28.0 * self.scale,
            ) else {
                return (wide.len() as f32) * 7.0 * self.scale;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut metrics);
            metrics.width.ceil()
        }
    }

    fn step_animation(&mut self) {
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_anim_tick)
            .as_secs_f32()
            .min(0.05);
        self.last_anim_tick = now;

        let follow = 1.0 - (-dt / 0.045).exp();
        if let Some(target_x) = self.mouse_x_target {
            self.mouse_x_smooth += (target_x - self.mouse_x_smooth) * follow;
        }
        self.hover_intensity +=
            (self.hover_target - self.hover_intensity) * (1.0 - (-dt / 0.06).exp());

        let settled_hover = (self.hover_target - self.hover_intensity).abs() < 0.01;
        let settled_mouse = self
            .mouse_x_target
            .is_none_or(|tx| (tx - self.mouse_x_smooth).abs() < 0.4);
        if settled_hover && settled_mouse {
            self.hover_intensity = self.hover_target;
            if let Some(tx) = self.mouse_x_target {
                self.mouse_x_smooth = tx;
            }
            self.animating = false;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ANIM_ID);
            }
        }
        self.render();
    }

    fn start_animation(&mut self) {
        if !self.animating {
            self.animating = true;
            self.last_anim_tick = Instant::now();
            unsafe {
                let _ = SetTimer(Some(self.hwnd), TIMER_ANIM_ID, TIMER_ANIM_MS, None);
            }
        }
    }

    fn item_index_at(&self, x: f32, y: f32) -> Option<usize> {
        self.items.iter().position(|it| {
            x >= it.hit_left && x < it.hit_right && y >= it.rect.top && y <= self.pill_rect.bottom
        })
    }

    fn handle_left_click(&mut self, x: f32, y: f32) {
        let Some(idx) = self.item_index_at(x, y) else {
            return;
        };
        let Some(item) = self.items.get(idx).cloned() else {
            return;
        };

        if item.is_pulse_files {
            super::open_standalone_files_window();
            return;
        }

        if item.windows.is_empty() {
            if let Some(ref src) = item.source {
                launch_source(src);
            }
            return;
        }

        unsafe {
            let fg = GetForegroundWindow();
            let focused_pos = if IsIconic(fg).as_bool() {
                None
            } else {
                item.windows.iter().position(|&h| h == fg)
            };
            match focused_pos {
                Some(_) if item.windows.len() == 1 => {
                    let target = item.windows[0];
                    let _ = ShowWindow(target, SW_MINIMIZE);
                    if !IsIconic(target).as_bool() {
                        let _ = PostMessageW(
                            Some(target),
                            WM_SYSCOMMAND,
                            WPARAM(SC_MINIMIZE as usize),
                            LPARAM(0),
                        );
                    }
                }
                Some(i) => {
                    let next = item.windows[(i + 1) % item.windows.len()];
                    focus_window_reliable(next);
                }
                None => {
                    let target = item
                        .windows
                        .iter()
                        .copied()
                        .find(|&h| IsWindow(Some(h)).as_bool() && !IsIconic(h).as_bool())
                        .unwrap_or(item.windows[0]);
                    focus_window_reliable(target);
                }
            }
        }
    }

    /// Inicia um candidato a arraste. Ícones sem origem (botão Arquivos) não
    /// participam: clique neles continua abrindo o módulo.
    fn start_drag_candidate(&mut self, x: f32, y: f32) {
        let Some(idx) = self.item_index_at(x, y) else {
            self.drag_source = None;
            self.dragging = false;
            return;
        };
        let Some(item) = self.items.get(idx) else {
            self.drag_source = None;
            self.dragging = false;
            return;
        };
        if item.is_pulse_files || item.source.is_none() {
            self.drag_source = None;
            self.dragging = false;
            return;
        }
        self.drag_source = Some((item.key.clone(), item.pinned, x));
        self.dragging = false;
        self.drag_x = x;
        self.mouse_x_target = None;
        self.hovered_index = None;
        self.hover_target = 0.0;
        unsafe {
            let _ = SetCapture(self.hwnd);
        }
    }

    /// Solta o ícone: arraste confirma a nova ordem, clique simples mantém o
    /// comportamento original (focar/lançar app).
    fn finish_drag_or_click(&mut self, x: f32, y: f32) {
        let was_dragging = self.dragging;
        let source = self.drag_source.take();
        self.dragging = false;
        unsafe {
            let _ = ReleaseCapture();
        }

        if was_dragging {
            if let Some((key, pinned, _)) = source {
                self.commit_dock_reorder(&key, pinned, x);
            }
            self.render();
            return;
        }
        self.handle_left_click(x, y);
    }

    /// Reordena dentro do próprio grupo (fixados com fixados, abertos com
    /// abertos). Cruzar a divisória não altera o estado de fixação: a soltura
    /// é ancorada no ícone do mesmo grupo mais próximo do cursor.
    fn commit_dock_reorder(&mut self, src_key: &str, src_pinned: bool, drop_x: f32) {
        let target_key = self
            .items
            .iter()
            .filter(|it| it.pinned == src_pinned && !it.is_pulse_files && it.key != src_key)
            .map(|it| {
                let mid = (it.rect.left + it.rect.right) * 0.5;
                (it.key.clone(), (drop_x - mid).abs())
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(key, _)| key);
        let Some(target_key) = target_key else {
            return;
        };

        let updated = if src_pinned {
            let mut entries: Vec<(String, String)> = self
                .items
                .iter()
                .filter(|it| it.pinned && !it.is_pulse_files)
                .map(|it| {
                    (
                        it.key.clone(),
                        it.source.clone().unwrap_or_else(|| it.key.clone()),
                    )
                })
                .collect();
            let Some(from) = entries.iter().position(|(key, _)| key == src_key) else {
                return;
            };
            let Some(to) = entries.iter().position(|(key, _)| key == &target_key) else {
                return;
            };
            let moved = entries.remove(from);
            entries.insert(to, moved);
            crate::beta_features::save_dock_order(
                entries.into_iter().map(|(_, src)| src).collect(),
                self.config.dock_unpinned_order.clone(),
            )
        } else {
            let mut order: Vec<String> = self
                .items
                .iter()
                .filter(|it| !it.pinned)
                .map(|it| it.key.clone())
                .collect();
            let Some(from) = order.iter().position(|key| key == src_key) else {
                return;
            };
            let Some(to) = order.iter().position(|key| key == &target_key) else {
                return;
            };
            let moved = order.remove(from);
            order.insert(to, moved);
            crate::beta_features::save_dock_order(self.config.dock_pinned_apps.clone(), order)
        };

        match updated {
            Ok(config) => {
                self.config = config.clone();
                super::notify_config_changed(&config);
                self.rebuild_items();
            }
            Err(err) => {
                shell_log::error("dock_reorder", "Falha ao salvar ordem da Dock", &err);
            }
        }
    }

    fn handle_wheel(&self, x: f32, y: f32, delta: i16) {
        let Some(idx) = self.item_index_at(x, y) else {
            return;
        };
        let Some(item) = self.items.get(idx) else {
            return;
        };
        if item.windows.len() < 2 {
            return;
        }
        unsafe {
            let fg = GetForegroundWindow();
            let cur = item.windows.iter().position(|&h| h == fg).unwrap_or(0);
            let next = if delta < 0 {
                (cur + 1) % item.windows.len()
            } else {
                (cur + item.windows.len() - 1) % item.windows.len()
            };
            focus_window_reliable(item.windows[next]);
        }
    }

    fn handle_right_click(&mut self, x: f32, y: f32) {
        let clicked_item = self
            .item_index_at(x, y)
            .and_then(|i| self.items.get(i).cloned());

        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let mut open_hwnds: Vec<HWND> = Vec::new();

            if let Some(ref target) = clicked_item {
                if target.is_pulse_files {
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING,
                        MENU_OPEN_FILES as usize,
                        w!("Abrir Arquivos no Pulse"),
                    );
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING,
                        MENU_OPEN_OVERVIEW as usize,
                        w!("Abrir Visão geral do Pulse"),
                    );
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                } else {
                    if !target.windows.is_empty() {
                        let fg = GetForegroundWindow();
                        for (wi, &win_hwnd) in target.windows.iter().take(10).enumerate() {
                            let title = menu_window_title(win_hwnd);
                            let wide: Vec<u16> =
                                title.encode_utf16().chain(std::iter::once(0)).collect();
                            let flags = if win_hwnd == fg {
                                MF_STRING | MF_CHECKED
                            } else {
                                MF_STRING
                            };
                            let _ = AppendMenuW(
                                menu,
                                flags,
                                (MENU_FOCUS_WINDOW_BASE as usize) + wi,
                                PCWSTR(wide.as_ptr()),
                            );
                            open_hwnds.push(win_hwnd);
                        }
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                    }

                    if target.source.is_some() {
                        let _ = AppendMenuW(
                            menu,
                            MF_STRING,
                            MENU_NEW_WINDOW as usize,
                            w!("Nova janela"),
                        );
                        let pin_label = if target.pinned {
                            w!("Desafixar da Dock")
                        } else {
                            w!("Fixar na Dock")
                        };
                        let _ = AppendMenuW(menu, MF_STRING, MENU_PIN_TOGGLE as usize, pin_label);
                    }

                    if !target.windows.is_empty() {
                        let close_label = if target.windows.len() > 1 {
                            w!("Fechar todas as janelas")
                        } else {
                            w!("Fechar janela")
                        };
                        let _ =
                            AppendMenuW(menu, MF_STRING, MENU_CLOSE_WINDOWS as usize, close_label);
                    }
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
            }

            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_SETTINGS as usize,
                w!("Configurar Pulse Shell"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_RESTORE_TASKBAR as usize,
                w!("Restaurar barra do Windows e desligar Pulse Shell"),
            );

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.hwnd);
            // O menu modal captura o mouse e dispara WM_MOUSELEAVE na Dock. Sem
            // esta proteção, o timer de auto-hide escondia a janela enquanto o
            // usuário escolhia "Fixar"/"Fechar" no popup.
            self.menu_open = true;
            let selected = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD,
                pt.x,
                pt.y,
                Some(0),
                self.hwnd,
                None,
            );
            self.menu_open = false;
            let _ = DestroyMenu(menu);
            self.update_visibility_state();
            self.render();

            let cmd = selected.0 as u32;
            if cmd >= MENU_FOCUS_WINDOW_BASE {
                let wi = (cmd - MENU_FOCUS_WINDOW_BASE) as usize;
                if let Some(&target_hwnd) = open_hwnds.get(wi) {
                    focus_window_reliable(target_hwnd);
                }
                return;
            }

            match cmd {
                MENU_OPEN_FILES => super::open_standalone_files_window(),
                MENU_OPEN_OVERVIEW => super::open_main_module("Visão geral"),
                MENU_NEW_WINDOW => {
                    if let Some(src) = clicked_item.and_then(|i| i.source) {
                        launch_source(&src);
                    }
                }
                MENU_PIN_TOGGLE => {
                    if let Some(src) = clicked_item.and_then(|i| i.source) {
                        if let Ok(updated) = crate::beta_features::toggle_pinned_app(&src) {
                            self.config = updated.clone();
                            super::notify_config_changed(&updated);
                            self.rebuild_items();
                            self.render();
                        }
                    }
                }
                MENU_CLOSE_WINDOWS => {
                    if let Some(target) = clicked_item {
                        for win_hwnd in target.windows {
                            let _ = PostMessageW(Some(win_hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                    }
                }
                MENU_OPEN_SETTINGS => super::open_beta_settings_in_main(),
                MENU_RESTORE_TASKBAR => super::trigger_emergency_restore_from_shell(),
                _ => {}
            }
        }
    }

    /// Em tela cheia, jogos em modo exclusivo recolocam a própria janela no
    /// topo da Z-order. Reafirmar TOPMOST a cada tick mantém a Dock acima deles
    /// (em jogos borderless isso é suficiente).
    fn reassert_topmost(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    /// Indica se a janela em primeiro plano é o Menu Iniciar/Pesquisa nativos.
    /// Só lê o foreground (nada de hook), com cache por hwnd.
    fn probe_start_menu(&mut self) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        let key = foreground.0 as isize;
        if key == 0 {
            return false;
        }
        if self.start_probe.0 != key {
            self.start_probe = (key, is_start_menu_window(foreground));
        }
        self.start_probe.1
    }

    /// Aplica a lupa conforme a posição do cursor (coordenadas da janela).
    fn apply_hover(&mut self, x: f32, y: f32) {
        let over_item = self.item_index_at(x, y).is_some()
            || (y >= self.pill_rect.top
                && y <= self.pill_rect.bottom
                && x >= self.pill_rect.left
                && x <= self.pill_rect.right);
        if over_item {
            if self.mouse_x_target.is_none() {
                self.mouse_x_smooth = x;
            }
            self.mouse_x_target = Some(x);
            self.hover_target = 1.0;
        } else {
            self.mouse_x_target = None;
            self.hovered_index = None;
            self.hover_target = 0.0;
        }
        self.start_animation();
    }

    fn sync_hover_from_cursor(&mut self, cursor: POINT) {
        if self.dragging || self.menu_open {
            return;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut rect) }.is_err() {
            return;
        }
        let inside = cursor.x >= rect.left
            && cursor.x < rect.right
            && cursor.y >= rect.top
            && cursor.y < rect.bottom;
        if inside {
            self.apply_hover((cursor.x - rect.left) as f32, (cursor.y - rect.top) as f32);
        } else if self.mouse_x_target.is_some() {
            self.apply_hover(-1.0, -1.0);
        }
    }

    fn update_visibility_state(&mut self) {
        let fullscreen = is_foreground_fullscreen_on_primary(self.hwnd);
        let now = Instant::now();
        if self.reveal_until.is_some_and(|until| now >= until) {
            self.reveal_until = None;
        }
        let start_open = self.probe_start_menu();
        if start_open != self.start_menu_open {
            shell_log::info(
                "dock_visibility",
                if start_open {
                    "Menu Iniciar aberto; Dock visível"
                } else {
                    "Menu Iniciar fechado; Dock liberada"
                },
            );
            self.start_menu_open = start_open;
        }
        let forced_visible =
            self.reveal_until.is_some() || self.hold_from_launcher || self.start_menu_open;

        let mut cursor = POINT::default();
        let has_cursor = unsafe { GetCursorPos(&mut cursor).is_ok() };
        if self.start_menu_open && has_cursor {
            // Com o Iniciar aberto a Dock pode não receber WM_MOUSEMOVE; a lupa
            // segue a posição do cursor lida a cada tick.
            self.sync_hover_from_cursor(cursor);
        }

        // Em tela cheia (jogo) a Dock fica sempre oculta, mesmo sem auto-hide,
        // e a borda inferior NÃO a revela: passar o mouse no fundo da tela
        // durante o jogo não pode cobrir nada. Só a tecla Windows (runner ou
        // Menu Iniciar) a mostra. Fora de tela cheia vale o hot corner.
        let mut want_hide_auto = false;
        if (self.config.dock_auto_hide || fullscreen)
            && !forced_visible
            && !self.menu_open
            && self.drag_source.is_none()
            && has_cursor
        {
            let zone = (9.0 * self.scale).max(6.0) as i32;
            let near_bottom = !fullscreen
                && cursor.y >= self.monitor.bottom - zone
                && cursor.x >= self.monitor.left
                && cursor.x < self.monitor.right;
            let inside_pill = self.mouse_x_target.is_some();
            want_hide_auto = !(near_bottom || inside_pill);
        }

        if fullscreen != self.fullscreen_active || want_hide_auto != self.hidden_by_autohide {
            if fullscreen != self.fullscreen_active {
                shell_log::info(
                    "dock_visibility",
                    format!(
                        "Foreground {} tela cheia; Dock em {}",
                        if fullscreen { "entrou em" } else { "saiu de" },
                        if want_hide_auto {
                            "auto-hide"
                        } else {
                            "overlay"
                        }
                    ),
                );
            }
            self.fullscreen_active = fullscreen;
            self.hidden_by_autohide = want_hide_auto;
            self.reposition_window();
            if !self.hidden_by_autohide {
                self.reassert_topmost();
                self.render();
            }
        } else if !self.hidden_by_autohide && (forced_visible || self.menu_open) {
            // Enquanto estiver visível por atalho, compete com a Z-order do jogo.
            self.reassert_topmost();
        }
    }
}

fn run_dock_thread(initial_config: BetaFeaturesConfig, ready: mpsc::Sender<Result<(), String>>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let res = run_dock_loop(initial_config, &ready);
    if let Err(err) = res {
        let _ = ready.send(Err(err));
    }
    DOCK_RUNNING.store(false, Ordering::SeqCst);
    DOCK_HWND.store(0, Ordering::SeqCst);
    ITEM_COUNT.store(0, Ordering::SeqCst);
    unsafe {
        CoUninitialize();
    }
}

fn run_dock_loop(
    initial_config: BetaFeaturesConfig,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let d2d_factory: ID2D1Factory = unsafe {
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| format!("D2D1CreateFactory (Dock) falhou: {e}"))?
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
    let dc_target = unsafe {
        d2d_factory
            .CreateDCRenderTarget(&props)
            .map_err(|e| format!("CreateDCRenderTarget (Dock) falhou: {e}"))?
    };
    unsafe {
        dc_target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    }
    let dwrite: IDWriteFactory = unsafe {
        DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(|e| format!("DWriteCreateFactory (Dock) falhou: {e}"))?
    };

    let hinstance =
        unsafe { GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW falhou: {e}"))? };
    let class_name = w!("PulseMacDockLayeredClass");
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(dock_wndproc),
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..Default::default()
    };
    unsafe {
        let _ = RegisterClassW(&wc);
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class_name,
            w!("Pulse Dock"),
            WS_POPUP,
            0,
            0,
            320,
            120,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW (Dock) falhou: {e}"))?
    };

    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
    let (tooltip_format, badge_format, initial_format) = create_dock_fonts(&dwrite, scale)?;
    let surface = DibSurface::new(320, 120)?;
    let mon = primary_monitor_rect();

    let mut state = Box::new(DockState {
        hwnd,
        config: initial_config,
        scale,
        monitor: mon,
        appbar_space: None,
        dc_target,
        dwrite,
        tooltip_format,
        badge_format,
        initial_format,
        surface,
        icon_cache: HashMap::new(),
        cached_icon_px: 0,
        items: Vec::new(),
        hovered_index: None,
        mouse_x_target: None,
        mouse_x_smooth: 0.0,
        hover_intensity: 0.0,
        hover_target: 0.0,
        animating: false,
        last_anim_tick: Instant::now(),
        tracking_mouse: false,
        hidden_by_autohide: false,
        fullscreen_active: false,
        menu_open: false,
        reveal_until: None,
        hold_from_launcher: false,
        start_menu_open: false,
        start_probe: (0, false),
        drag_source: None,
        dragging: false,
        drag_x: 0.0,
        win_x: 0,
        win_y: 0,
        pill_rect: D2D_RECT_F::default(),
        first_render_logged: false,
    });

    // Renderiza a Dock imediatamente antes de reservar WorkArea (zero bloqueio).
    state.rebuild_items();
    state.render();
    state.sync_appbar();

    let state_ptr = Box::into_raw(state);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        let _ = SetTimer(Some(hwnd), TIMER_RECONCILE_ID, TIMER_RECONCILE_MS, None);
        let _ = SetTimer(Some(hwnd), TIMER_AUTOHIDE_ID, TIMER_AUTOHIDE_MS, None);
    }

    DOCK_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    DOCK_RUNNING.store(true, Ordering::SeqCst);

    let hooks = install_win_event_hooks();
    let _ = ready.send(Ok(()));

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    for hook in hooks {
        if !hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
    }

    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_RECONCILE_ID);
        let _ = KillTimer(Some(hwnd), TIMER_AUTOHIDE_ID);
        let _ = KillTimer(Some(hwnd), TIMER_ANIM_ID);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let mut boxed = Box::from_raw(state_ptr);
        if let Some(space) = boxed.appbar_space.take() {
            space.close();
        }
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(class_name, Some(hinstance.into()));
    }

    Ok(())
}

fn install_win_event_hooks() -> Vec<HWINEVENTHOOK> {
    let mut hooks = Vec::new();
    for (min, max) in [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY),
        (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
    ] {
        let hook = unsafe {
            SetWinEventHook(
                min,
                max,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if !hook.is_invalid() {
            hooks.push(hook);
        }
    }
    hooks
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != 0 {
        return;
    }
    let raw = DOCK_HWND.load(Ordering::Relaxed);
    if raw != 0 {
        let _ = PostMessageW(
            Some(HWND(raw as *mut _)),
            WM_APP_REFRESH_ITEMS,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

unsafe extern "system" fn dock_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DockState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *ptr;

    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE),
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as f32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
            state.start_drag_candidate(x, y);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as f32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;

            if let Some((_, _, start_x)) = state.drag_source.as_ref() {
                if state.dragging || (x - *start_x).abs() >= 6.0 * state.scale {
                    state.dragging = true;
                    state.drag_x = x;
                    state.mouse_x_target = None;
                    state.hovered_index = None;
                    state.hover_target = 0.0;
                    state.render();
                    return LRESULT(0);
                }
            }

            if !state.tracking_mouse {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                if TrackMouseEvent(&mut tme).is_ok() {
                    state.tracking_mouse = true;
                }
            }

            state.apply_hover(x, y);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            state.tracking_mouse = false;
            if !state.menu_open {
                state.mouse_x_target = None;
                state.hovered_index = None;
                state.hover_target = 0.0;
                state.start_animation();
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            // Captura perdida (ex.: Alt+Tab): aborta o arraste sem fazer clique.
            if state.dragging || state.drag_source.is_some() {
                state.dragging = false;
                state.drag_source = None;
                state.render();
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as f32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
            state.finish_drag_or_click(x, y);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as f32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
            state.handle_right_click(x, y);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let delta = ((wparam.0 >> 16) as u16) as i16;
            state.handle_wheel((sx - state.win_x) as f32, (sy - state.win_y) as f32, delta);
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                TIMER_RECONCILE_ID => {
                    state.rebuild_items();
                    state.render();
                }
                TIMER_AUTOHIDE_ID => {
                    state.update_visibility_state();
                }
                TIMER_ANIM_ID => {
                    state.step_animation();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_REFRESH_ITEMS => {
            state.rebuild_items();
            state.render();
            LRESULT(0)
        }
        WM_APP_APPLY_CONFIG => {
            if let Ok(cfg) = config_slot().lock() {
                state.config = cfg.clone();
            }
            state.sync_appbar();
            state.rebuild_items();
            state.update_visibility_state();
            state.render();
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            let _ = ReplyMessage(LRESULT(0));
            state.reposition_window();
            state.sync_appbar();
            state.render();
            LRESULT(0)
        }
        WM_APP_RECHECK_APPBAR => {
            if let Some(space) = state.appbar_space.take() {
                space.close();
            }
            state.reposition_window();
            state.sync_appbar();
            state.render();
            LRESULT(0)
        }
        WM_APP_REVEAL => {
            state.reveal_until = Some(Instant::now() + Duration::from_millis(HOTKEY_REVEAL_MS));
            state.update_visibility_state();
            state.reassert_topmost();
            state.render();
            shell_log::info(
                "dock_visibility",
                format!(
                    "Revelada pela tecla Windows (fullscreen={})",
                    state.fullscreen_active
                ),
            );
            LRESULT(0)
        }
        WM_APP_HOLD => {
            let hold = wparam.0 != 0;
            if state.hold_from_launcher != hold {
                shell_log::info(
                    "dock_visibility",
                    format!(
                        "Runner {}; Dock {}",
                        if hold { "aberto" } else { "fechado" },
                        if hold { "presa" } else { "liberada" }
                    ),
                );
            }
            state.hold_from_launcher = hold;
            state.update_visibility_state();
            if hold && !state.hidden_by_autohide {
                state.reassert_topmost();
                state.render();
            }
            LRESULT(0)
        }
        WM_APP_SHUTDOWN => {
            if let Some(space) = state.appbar_space.take() {
                space.close();
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_dock_fonts(
    dwrite: &IDWriteFactory,
    scale: f32,
) -> Result<(IDWriteTextFormat, IDWriteTextFormat, IDWriteTextFormat), String> {
    let mut fonts: Option<IDWriteFontCollection> = None;
    let _ = unsafe { dwrite.GetSystemFontCollection(&mut fonts, false) };
    let has_font = |name: PCWSTR| {
        let mut idx = 0u32;
        let mut exists = windows::core::BOOL(0);
        fonts.as_ref().is_some_and(|f| {
            unsafe { f.FindFamilyName(name, &mut idx, &mut exists) }.is_ok() && exists.as_bool()
        })
    };
    let family = if has_font(w!("Segoe UI Variable Text")) {
        w!("Segoe UI Variable Text")
    } else {
        w!("Segoe UI")
    };

    let make = |weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
                size: f32|
     -> Result<IDWriteTextFormat, String> {
        unsafe {
            let fmt = dwrite
                .CreateTextFormat(
                    family,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * scale,
                    w!("pt-BR"),
                )
                .map_err(|e| format!("CreateTextFormat (Dock) falhou: {e}"))?;
            let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            Ok(fmt)
        }
    };

    Ok((
        make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 11.5)?,
        make(DWRITE_FONT_WEIGHT_BOLD, 9.5)?,
        make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 17.0)?,
    ))
}

fn focus_window_reliable(hwnd: HWND) {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return;
        }
        // Para janelas minimizadas (incluindo WinUI 3 como WhatsApp),
        // SwitchToThisWindow restaura sem o bug de tela branca do SW_RESTORE.
        if IsIconic(hwnd).as_bool() {
            SwitchToThisWindow(hwnd, true);
            return;
        }
        let _ = SetForegroundWindow(hwnd);
        if GetForegroundWindow() == hwnd {
            return;
        }
        // Com o Iniciar (ou outro processo) em foco, o foreground lock recusa o
        // pedido. Anexar a fila de entrada à thread em foco libera a troca sem
        // injetar teclas (nada que um anti-cheat veja como input sintético).
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let own_thread = GetCurrentThreadId();
        if foreground_thread != 0 && foreground_thread != own_thread {
            let attached = AttachThreadInput(own_thread, foreground_thread, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(own_thread, foreground_thread, false);
            }
        }
        if GetForegroundWindow() != hwnd {
            SwitchToThisWindow(hwnd, true);
        }
    }
}

/// Executáveis do Menu Iniciar/Pesquisa nativos do Windows 10/11.
const START_MENU_HOSTS: &[&str] = &[
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "searchapp.exe",
];

fn is_start_menu_window(hwnd: HWND) -> bool {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == 0 {
        return false;
    }
    let Some(path) = process_image_path(pid) else {
        return false;
    };
    let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
    START_MENU_HOSTS
        .iter()
        .any(|host| name.eq_ignore_ascii_case(host))
}

fn process_image_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if !ok || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn primary_monitor_info() -> (RECT, RECT) {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut info).as_bool() {
            (info.rcMonitor, info.rcWork)
        } else {
            let fallback = RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            };
            (fallback, fallback)
        }
    }
}

fn primary_monitor_rect() -> RECT {
    primary_monitor_info().0
}

pub fn is_foreground_fullscreen_on_primary(own_hwnd: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == own_hwnd {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == std::process::id() {
            return false;
        }
        if GetWindowTextLengthW(fg) == 0 {
            return false;
        }
        let mut class_buf = [0u16; 64];
        let clen = GetClassNameW(fg, &mut class_buf).max(0) as usize;
        let class_name = String::from_utf16_lossy(&class_buf[..clen]);
        if matches!(
            class_name.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(fg, &mut rect).is_err() {
            return false;
        }
        let monitor = primary_monitor_rect();
        rect.left <= monitor.left
            && rect.top <= monitor.top
            && rect.right >= monitor.right
            && rect.bottom >= monitor.bottom
    }
}

fn enumerate_top_windows(self_hwnd: HWND) -> Vec<RunningWindow> {
    struct Ctx {
        self_hwnd: HWND,
        foreground: HWND,
        windows: Vec<RunningWindow>,
    }

    let mut ctx = Ctx {
        self_hwnd,
        foreground: unsafe { GetForegroundWindow() },
        windows: Vec::new(),
    };

    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        if hwnd == ctx.self_hwnd || !IsWindowVisible(hwnd).as_bool() {
            return windows::core::BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let topbar_raw = super::topbar::hwnd_raw();
        if topbar_raw != 0 && hwnd.0 as isize == topbar_raw {
            return windows::core::BOOL(1);
        }

        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if (ex_style & WS_EX_TOOLWINDOW.0) != 0 {
            return windows::core::BOOL(1);
        }
        let has_owner = GetWindow(hwnd, GW_OWNER)
            .map(|h| !h.is_invalid())
            .unwrap_or(false);
        if has_owner && (ex_style & WS_EX_APPWINDOW.0) == 0 {
            return windows::core::BOOL(1);
        }

        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut _,
            size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return windows::core::BOOL(1);
        }

        let title_len = GetWindowTextLengthW(hwnd);
        if title_len <= 0 {
            return windows::core::BOOL(1);
        }
        let mut title_buf = vec![0u16; (title_len + 1) as usize];
        let copied = GetWindowTextW(hwnd, &mut title_buf);
        let title = String::from_utf16_lossy(&title_buf[..copied.max(0) as usize])
            .trim()
            .to_owned();
        if title.is_empty() || title == "Program Manager" {
            return windows::core::BOOL(1);
        }

        let mut class_buf = [0u16; 64];
        let clen = GetClassNameW(hwnd, &mut class_buf).max(0) as usize;
        let class_name = String::from_utf16_lossy(&class_buf[..clen]);
        if matches!(
            class_name.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return windows::core::BOOL(1);
        }

        let exe_path = resolve_host_exe_path(pid);
        let (app_key, source) = match exe_path {
            Some(ref path)
                if Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("ApplicationFrameHost.exe")) =>
            {
                if let Some(aumid) = window_aumid(hwnd) {
                    (
                        format!("aumid:{}", aumid.to_lowercase()),
                        Some(format!("{APPS_FOLDER}{aumid}")),
                    )
                } else {
                    (format!("hwnd-{}", hwnd.0 as isize), None)
                }
            }
            Some(path) => (source_key(&path), Some(path)),
            None => (format!("hwnd-{}", hwnd.0 as isize), None),
        };

        ctx.windows.push(RunningWindow {
            hwnd,
            title,
            app_key,
            source,
            is_foreground: hwnd == ctx.foreground,
        });

        windows::core::BOOL(1)
    }

    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.windows
}

fn window_aumid(hwnd: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let value = store.GetValue(&PKEY_AUMID).ok()?;
        let id = value.to_string();
        (!id.is_empty()).then_some(id)
    }
}

fn source_key(source: &str) -> String {
    if let Some(aumid) = source.strip_prefix(APPS_FOLDER) {
        format!("aumid:{}", aumid.to_lowercase())
    } else {
        source.to_lowercase()
    }
}

fn display_name_from_source(source: &str) -> String {
    if let Some(aumid) = source.strip_prefix(APPS_FOLDER) {
        let base = aumid.split('!').next().unwrap_or(aumid);
        let pkg = base.split('_').next().unwrap_or(base);
        let raw = pkg.rsplit('.').next().unwrap_or(pkg);
        return prettify_dock_name(raw);
    }
    let stem = Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim();
    if stem.is_empty() {
        return source.to_owned();
    }
    // O stem de jogos costuma ser técnico (`Marvel-Win64-Shipping.exe`);
    // o nome oficial vem do recurso de versão do PE.
    let prettified = prettify_dock_name(stem);
    let friendly = (!prettified.eq_ignore_ascii_case(stem)).then_some(prettified.as_str());
    let official = crate::process_identity::official_name_for_path(source);
    crate::process_identity::choose_display_name(stem, official.as_deref(), friendly)
}

/// Usa o título da janela quando o executável não fornece um nome oficial
/// (jogos com anti-cheat costumam ter só o nome técnico no recurso de versão).
fn display_name_from_source_with_title(source: &str, window_title: &str) -> String {
    let resolved = display_name_from_source(source);
    let stem = Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if resolved.eq_ignore_ascii_case(stem) && !window_title.is_empty() {
        return window_title.chars().take(28).collect();
    }
    resolved
}

fn prettify_dock_name(stem: &str) -> String {
    let trimmed = stem.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "whatsapp.root" | "whatsappdesktop" | "whatsapp" => "WhatsApp".to_owned(),
        "explorer" => "Explorador".to_owned(),
        "msedge" => "Microsoft Edge".to_owned(),
        "chrome" => "Google Chrome".to_owned(),
        "firefox" => "Firefox".to_owned(),
        "code" => "VS Code".to_owned(),
        "windowsterminal" => "Terminal".to_owned(),
        "notepad" => "Bloco de Notas".to_owned(),
        "discord" => "Discord".to_owned(),
        "pulse-windows" => "Pulse".to_owned(),
        _ => trimmed.strip_suffix(".Root").unwrap_or(trimmed).to_owned(),
    }
}

fn menu_window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) }.max(0) as usize;
    let raw = String::from_utf16_lossy(&buf[..len]);
    let mut short: String = raw.chars().take(54).collect();
    if raw.chars().count() > 54 {
        short.push('…');
    }
    if short.is_empty() {
        "Janela sem título".to_owned()
    } else {
        short.replace('&', "&&")
    }
}

fn launch_source(source: &str) {
    let target = if source.starts_with(APPS_FOLDER) {
        source.to_owned()
    } else {
        store_app_id(source).unwrap_or_else(|| source.to_owned())
    };
    let wide: Vec<u16> = OsStr::new(&target)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Resolve `shell:AppsFolder\<Família>!<Id>` para apps da Microsoft Store fixados
/// pelo caminho do `.exe` dentro de `WindowsApps`.
pub fn store_app_id(path: &str) -> Option<String> {
    const DIR: &str = "\\windowsapps\\";
    let lower = path.to_lowercase();
    let i = lower.find(DIR)? + DIR.len();
    let (folder, exe) = path[i..].split_once('\\')?;
    let parts: Vec<&str> = folder.split('_').collect();
    let [name, _, _, _, publisher] = parts[..] else {
        return None;
    };
    let manifest =
        std::fs::read_to_string(Path::new(&path[..i]).join(folder).join("AppxManifest.xml"))
            .ok()?;
    manifest.split("<Application ").skip(1).find_map(|tag| {
        let tag = &tag[..tag.find('>')?];
        let executable = xml_attr(tag, "Executable")?.replace('/', "\\");
        executable
            .eq_ignore_ascii_case(exe)
            .then(|| xml_attr(tag, "Id").map(|id| format!("{APPS_FOLDER}{name}_{publisher}!{id}")))
            .flatten()
    })
}

fn xml_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = format!(" {tag}").find(&key)? + key.len() - 1;
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_app_id_resolves_windowsapps_manifest() {
        let root = std::env::temp_dir()
            .join(format!("pulse-dock-test-{}", std::process::id()))
            .join("WindowsApps");
        let pkg = root.join("5319275A.WhatsAppDesktop_2.2645.101.0_x64__cv1g1gvanyjgm");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("AppxManifest.xml"),
            r#"<Applications><Application Id="App" Executable="WhatsApp.Root.exe" /></Applications>"#,
        )
        .unwrap();
        let exe = pkg.join("WhatsApp.Root.exe").display().to_string();
        assert_eq!(
            store_app_id(&exe).as_deref(),
            Some("shell:AppsFolder\\5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App")
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
