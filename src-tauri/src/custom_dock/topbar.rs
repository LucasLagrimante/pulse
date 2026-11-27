//! Barra Superior nativa integrada ao Pulse (estilo macOS, ~32px, AppBar `ABE_TOP`).
//!
//! Renderizada com `ID2D1DCRenderTarget` + DIB 32-bit pré-multiplicada + `UpdateLayeredWindow`.

use super::{
    appbar::{AppBarEdge, AppBarSpace},
    icons::{create_pulse_mark_icon, d2d_bitmap_from_icon},
    power, shell_log,
    telemetry_feed::{self, format_rate_compact, ShellTelemetrySnapshot},
    tray_proxy::{self, TrayClickButton},
    watchdog::{self, PROXY_PROP},
};
use crate::beta_features::BetaFeaturesConfig;
use chrono::{Datelike, Local, Timelike};
use std::{
    ffi::OsString,
    mem::size_of,
    os::windows::ffi::OsStringExt,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::{
    core::{w, PCWSTR, PWSTR},
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
                D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT,
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
            },
            DirectWrite::{
                DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
                DWRITE_WORD_WRAPPING_NO_WRAP,
            },
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
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            LibraryLoader::GetModuleHandleW,
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
            Controls::{
                TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_UPDATETIPTEXTW, TTTOOLINFOW,
            },
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
            },
            Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DestroyWindow, DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW,
                GetPropW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
                GetWindowThreadProcessId, KillTimer, LoadCursorW, MsgWaitForMultipleObjectsEx,
                PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW, ReplyMessage,
                SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos,
                ShowWindow, TrackPopupMenu, TranslateMessage, UnregisterClassW, CHILDID_SELF,
                CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SHOW,
                EVENT_SYSTEM_FOREGROUND, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW, MF_CHECKED,
                MF_SEPARATOR, MF_STRING, MSG, MWMO_INPUTAVAILABLE, OBJID_WINDOW, PM_NOREMOVE,
                QS_SENDMESSAGE, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, TPM_LEFTALIGN,
                TPM_RETURNCMD, TPM_TOPALIGN, ULW_ALPHA, WHEEL_DELTA, WINEVENT_OUTOFCONTEXT,
                WINEVENT_SKIPOWNPROCESS, WM_APP, WM_CAPTURECHANGED, WM_DESTROY, WM_DISPLAYCHANGE,
                WM_DPICHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
                WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_POWERBROADCAST, WM_RBUTTONUP, WM_SETTINGCHANGE,
                WM_TIMECHANGE, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
            },
        },
    },
};

const WM_MOUSELEAVE: u32 = 0x02A3;
const MA_NOACTIVATE: isize = 3;

const WM_APP_APPLY_CONFIG: u32 = WM_APP + 11;
const WM_APP_ENFORCE_TASKBAR_HIDE: u32 = WM_APP + 12;
const WM_APP_TRAY_UPDATED: u32 = WM_APP + 61;
const WM_APP_TELEMETRY_UPDATED: u32 = WM_APP + 62;
const WM_APP_FOREGROUND_CHANGED: u32 = WM_APP + 63;
const WM_APP_SHUTDOWN: u32 = WM_APP + 64;

const TIMER_CLOCK_ID: usize = 1;
const TIMER_RECONCILE_ID: usize = 2;
const RECONCILE_MS: u32 = 1500;

const MENU_OPEN_PULSE: u32 = 2001;
const MENU_OPEN_PERF: u32 = 2002;
const MENU_OPEN_FILES: u32 = 2003;
const MENU_OPEN_STORAGE: u32 = 2004;
const MENU_OPEN_QUOTAS: u32 = 2005;
const MENU_OPEN_BETA: u32 = 2006;
const MENU_OPEN_RUNNER: u32 = 2007;
const MENU_TOGGLE_AUTOHIDE: u32 = 2008;
const MENU_LOCK: u32 = 2010;
const MENU_SLEEP: u32 = 2011;
const MENU_RESTART: u32 = 2012;
const MENU_SHUTDOWN: u32 = 2013;
const MENU_RESTORE_TASKBAR: u32 = 2020;
const MENU_EXIT_PULSE: u32 = 2021;

const BASE_HEIGHT_DIP: f32 = 32.0;
const MAX_TRAY_ICONS_BAR: usize = 10;

/// PKEY_AppUserModel_ID (`{9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}`, 5)
const PKEY_AUMID: windows::Win32::Foundation::PROPERTYKEY =
    windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };

static TOPBAR_HWND: AtomicIsize = AtomicIsize::new(0);
static TOPBAR_RUNNING: AtomicBool = AtomicBool::new(false);
static TOPBAR_THREAD: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
static TOPBAR_CONFIG: OnceLock<Mutex<BetaFeaturesConfig>> = OnceLock::new();

fn thread_slot() -> &'static Mutex<Option<JoinHandle<()>>> {
    TOPBAR_THREAD.get_or_init(|| Mutex::new(None))
}

fn config_slot() -> &'static Mutex<BetaFeaturesConfig> {
    TOPBAR_CONFIG.get_or_init(|| Mutex::new(BetaFeaturesConfig::default()))
}

pub fn hwnd_raw() -> isize {
    TOPBAR_HWND.load(Ordering::Relaxed)
}

pub fn is_running() -> bool {
    TOPBAR_RUNNING.load(Ordering::SeqCst)
}

pub fn start(config: &BetaFeaturesConfig) -> Result<(), String> {
    if let Ok(mut cfg) = config_slot().lock() {
        *cfg = config.clone();
    }
    let mut guard = thread_slot().lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() && TOPBAR_RUNNING.load(Ordering::SeqCst) {
        let raw = TOPBAR_HWND.load(Ordering::SeqCst);
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
        .name("pulse-top-bar".to_owned())
        .spawn(move || run_topbar_thread(initial, ready_tx))
        .map_err(|e| format!("Falha ao iniciar a Barra Superior: {e}"))?;

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
            Err("A thread da Barra Superior encerrou antes da inicialização.".to_owned())
        }
    }
}

pub fn stop() {
    let taken = thread_slot().lock().ok().and_then(|mut g| g.take());
    let raw = TOPBAR_HWND.load(Ordering::SeqCst);
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
    TOPBAR_RUNNING.store(false, Ordering::SeqCst);
    TOPBAR_HWND.store(0, Ordering::SeqCst);
}

fn wait_thread_pumping_messages(handle: JoinHandle<()>) {
    use std::os::windows::io::AsRawHandle;
    let raw_handle = [HANDLE(handle.as_raw_handle())];
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let left = deadline
            .saturating_duration_since(std::time::Instant::now())
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

#[derive(Clone, PartialEq, Eq)]
enum HitAction {
    PulseMenu,
    ActiveApp,
    FilesButton,
    Widget(String),
    TrayIcon(String),
    Volume,
    Battery,
    Clock,
}

struct HitItem {
    action: HitAction,
    left: f32,
    right: f32,
    tooltip: String,
    alert: u8,
    glyph: Option<u16>,
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
                .map_err(|e| format!("CreateDIBSection (TopBar) falhou: {e}"))?;
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

struct TopBarState {
    hwnd: HWND,
    tooltip_hwnd: Option<HWND>,
    config: BetaFeaturesConfig,
    scale: f32,
    monitor: RECT,
    bar_rect: RECT,
    appbar_space: Option<AppBarSpace>,
    surface: DibSurface,
    dc_target: ID2D1DCRenderTarget,
    dwrite: IDWriteFactory,
    font_bold: IDWriteTextFormat,
    font_medium: IDWriteTextFormat,
    font_glyph: IDWriteTextFormat,
    pulse_logo_light: Option<ID2D1Bitmap>,
    pulse_logo_dark: Option<ID2D1Bitmap>,
    tray_bitmaps: Vec<(String, Option<ID2D1Bitmap>)>,
    tray_views: Vec<tray_proxy::TrayIconView>,
    telemetry: ShellTelemetrySnapshot,
    active_app_name: String,
    clock_text: String,
    items: Vec<HitItem>,
    hovered: Option<HitAction>,
    last_tooltip: String,
    tracking_mouse: bool,
    fullscreen: bool,
    first_render_logged: bool,
    drag_source: Option<(HitAction, f32)>,
    dragging: bool,
    drag_x: f32,
}

impl TopBarState {
    fn update_fonts(&mut self) {
        let s = self.scale;
        if let Ok((b, m, g)) = create_text_formats(&self.dwrite, s) {
            self.font_bold = b;
            self.font_medium = m;
            self.font_glyph = g;
        }
        let logo_px = (18.0 * s).round().clamp(16.0, 40.0) as u32;
        self.pulse_logo_light =
            d2d_bitmap_from_icon(&self.dc_target, &create_pulse_mark_icon(logo_px, false));
        self.pulse_logo_dark =
            d2d_bitmap_from_icon(&self.dc_target, &create_pulse_mark_icon(logo_px, true));
    }

    fn sync_geometry(&mut self) {
        let (mon, _) = primary_monitor_info();
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        let w = (mon.right - mon.left).max(1);
        let h = (BASE_HEIGHT_DIP * scale).round().max(24.0) as i32;

        let scale_changed = (scale - self.scale).abs() > 0.01;
        let size_changed = w != self.surface.width || h != self.surface.height;
        self.scale = scale;
        self.monitor = mon;

        if size_changed {
            match DibSurface::new(w, h) {
                Ok(new_surface) => self.surface = new_surface,
                Err(err) => shell_log::error("topbar_surface", "Falha ao recriar DIB", &err),
            }
        }
        if scale_changed {
            self.update_fonts();
        }

        if let Some(ref space) = self.appbar_space {
            space.set_thickness(h);
        }

        self.bar_rect = RECT {
            left: mon.left,
            top: mon.top,
            right: mon.left + w,
            bottom: mon.top + h,
        };

        tray_proxy::bind_topbar(self.hwnd, self.bar_rect, WM_APP_TRAY_UPDATED);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                self.bar_rect.left,
                self.bar_rect.top,
                w,
                h,
                SWP_NOACTIVATE,
            );
            let _ = ShowWindow(
                self.hwnd,
                if self.fullscreen {
                    SW_HIDE
                } else {
                    SW_SHOWNOACTIVATE
                },
            );
        }
    }

    fn refresh_tray_views(&mut self) {
        self.tray_views = tray_proxy::visible_icons();
        self.tray_bitmaps.clear();
        for view in &self.tray_views {
            let bmp = view
                .image
                .as_ref()
                .and_then(|img| d2d_bitmap_from_icon(&self.dc_target, img));
            self.tray_bitmaps.push((view.id.clone(), bmp));
        }
    }

    fn measure_text(&self, text: &str, fmt: &IDWriteTextFormat, max_w: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let h = self.surface.height as f32;
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, fmt, max_w, h) else {
                return (text.chars().count() as f32) * 7.0 * self.scale;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut metrics);
            metrics.width.ceil()
        }
    }

    fn build_layout_and_render(&mut self) {
        if self.fullscreen {
            return;
        }
        let s = self.scale;
        let w = self.surface.width as f32;
        let h = self.surface.height as f32;
        let pad_x = 10.0 * s;
        let chip_pad = 8.0 * s;
        let gap = 4.0 * s;

        let mut items: Vec<HitItem> = Vec::new();

        // --- ESQUERDA: [Logo Pulse] [App em foco] [Arquivos] ---
        let mut lx = pad_x;
        let pulse_w = 28.0 * s;
        items.push(HitItem {
            action: HitAction::PulseMenu,
            left: lx,
            right: lx + pulse_w,
            tooltip: "Menu Pulse e Sistema".to_owned(),
            alert: 0,
            glyph: None,
        });
        lx += pulse_w + gap;

        let app_text = if self.active_app_name.is_empty() {
            "Área de Trabalho".to_owned()
        } else {
            self.active_app_name.clone()
        };
        let app_text_w = self
            .measure_text(&app_text, &self.font_bold, 220.0 * s)
            .min(220.0 * s);
        let app_item_w = app_text_w + chip_pad * 2.0;
        items.push(HitItem {
            action: HitAction::ActiveApp,
            left: lx,
            right: lx + app_item_w,
            tooltip: format!("Aplicativo em foco: {app_text}"),
            alert: 0,
            glyph: None,
        });
        lx += app_item_w + gap;

        let files_text_w = self.measure_text("Arquivos", &self.font_medium, 120.0 * s);
        let files_w = 15.0 * s + 4.0 * s + files_text_w + chip_pad * 2.0;
        items.push(HitItem {
            action: HitAction::FilesButton,
            left: lx,
            right: lx + files_w,
            tooltip: "Abrir Explorador de Arquivos do Pulse".to_owned(),
            alert: 0,
            glyph: None,
        });
        let left_end = lx + files_w + 12.0 * s;

        // --- DIREITA: segue exatamente a ordem de `config.top_bar_widgets` ---
        struct RightSpec {
            action: HitAction,
            width: f32,
            label: String,
            tooltip: String,
            alert: u8,
            glyph: Option<u16>,
        }

        let mut right_specs: Vec<RightSpec> = Vec::new();
        let tray_slot_w = 22.0 * s;
        let mut battery_placed = false;

        let push_battery = |specs: &mut Vec<RightSpec>, this: &Self| {
            // Bateria de notebook (somente quando BatteryFlag != 128 && BatteryLifePercent <= 100)
            if let Some(ref bat) = this.telemetry.battery {
                let text = if bat.charging {
                    format!("⚡{}%", bat.percent)
                } else {
                    format!("{}%", bat.percent)
                };
                let tw = this.measure_text(&text, &this.font_medium, 80.0 * s);
                let bw = 16.0 * s + 3.0 * s + tw + chip_pad * 1.5;
                specs.push(RightSpec {
                    action: HitAction::Battery,
                    width: bw,
                    label: text,
                    tooltip: format!(
                        "Bateria do notebook: {}%{}",
                        bat.percent,
                        if bat.charging { " (carregando)" } else { "" }
                    ),
                    alert: if !bat.charging && bat.percent <= 20 {
                        1
                    } else {
                        0
                    },
                    glyph: None,
                });
            }
        };

        for widget_id in &self.config.top_bar_widgets {
            match widget_id.as_str() {
                "tray" => {
                    for view in self.tray_views.iter().take(MAX_TRAY_ICONS_BAR) {
                        right_specs.push(RightSpec {
                            action: HitAction::TrayIcon(view.id.clone()),
                            width: tray_slot_w,
                            label: String::new(),
                            tooltip: if view.tip.is_empty() {
                                "Ícone da bandeja (arraste para reordenar)".to_owned()
                            } else {
                                view.tip.clone()
                            },
                            alert: 0,
                            glyph: None,
                        });
                    }
                }
                "volume" => {
                    if let (Some(pct), Some(muted)) =
                        (self.telemetry.volume_percent, self.telemetry.volume_muted)
                    {
                        let text = if muted {
                            "Mudo".to_owned()
                        } else {
                            format!("{pct}%")
                        };
                        let tw = self.measure_text(&text, &self.font_medium, 80.0 * s);
                        let vw = 16.0 * s + 3.0 * s + tw + chip_pad * 1.5;
                        right_specs.push(RightSpec {
                            action: HitAction::Volume,
                            width: vw,
                            label: text,
                            tooltip: if muted {
                                "Som silenciado (clique alterna mudo · direito abre Painel de Áudio · roda ajusta)"
                                    .to_owned()
                            } else {
                                format!(
                                    "Volume: {pct}% (clique alterna mudo · direito abre Painel de Áudio · roda ajusta)"
                                )
                            },
                            alert: 0,
                            glyph: None,
                        });
                    }
                }
                "clock" => {
                    if !battery_placed {
                        push_battery(&mut right_specs, self);
                        battery_placed = true;
                    }
                    let cw =
                        self.measure_text(&self.clock_text, self.font_semi_or_med(), 200.0 * s)
                            + chip_pad * 2.0;
                    right_specs.push(RightSpec {
                        action: HitAction::Clock,
                        width: cw,
                        label: self.clock_text.clone(),
                        tooltip:
                            "Calendário, data e hora (clique para abrir · arraste para reordenar)"
                                .to_owned(),
                        alert: 0,
                        glyph: None,
                    });
                }
                "peripherals" => {
                    if self.config.peripheral_battery {
                        if self.telemetry.peripherals.is_empty() {
                            let label = "Perif. —".to_owned();
                            let tw = self.measure_text(&label, &self.font_medium, 180.0 * s);
                            right_specs.push(RightSpec {
                                action: HitAction::Widget("peripherals".to_owned()),
                                width: tw + chip_pad * 2.0,
                                label,
                                tooltip: "Nenhum periférico com bateria medida (clique para abrir)"
                                    .to_owned(),
                                alert: 0,
                                glyph: None,
                            });
                        } else {
                            for p in &self.telemetry.peripherals {
                                let tw = self.measure_text(&p.label, &self.font_medium, 180.0 * s);
                                let pw = 15.0 * s + 3.0 * s + tw + chip_pad * 1.5;
                                right_specs.push(RightSpec {
                                    action: HitAction::Widget("peripherals".to_owned()),
                                    width: pw,
                                    label: p.label.clone(),
                                    tooltip: p.tooltip.clone(),
                                    alert: u8::from(p.low_battery),
                                    glyph: Some(peripheral_glyph(p.kind)),
                                });
                            }
                        }
                    }
                }
                "quotas" => {
                    for q in &self.telemetry.quotas {
                        let tw = self.measure_text(&q.label, &self.font_medium, 180.0 * s);
                        right_specs.push(RightSpec {
                            action: HitAction::Widget("quotas".to_owned()),
                            width: tw + chip_pad * 2.0,
                            label: q.label.clone(),
                            tooltip: q.tooltip.clone(),
                            alert: q.severity,
                            glyph: None,
                        });
                    }
                }
                other => {
                    if let Some(sp) = self.build_widget_spec(other) {
                        right_specs.push(RightSpec {
                            action: HitAction::Widget(widget_id.clone()),
                            width: sp.0,
                            label: sp.1,
                            tooltip: sp.2,
                            alert: sp.3,
                            glyph: None,
                        });
                    }
                }
            }
        }
        if !battery_placed {
            push_battery(&mut right_specs, self);
        }

        // Posiciona da direita para a esquerda respeitando `left_end`
        let total_right_w: f32 = right_specs.iter().map(|r| r.width).sum::<f32>()
            + gap * right_specs.len().saturating_sub(1) as f32;
        let mut rx = (w - pad_x - total_right_w).max(left_end);

        let mut tray_screen_rects: Vec<(String, RECT)> = Vec::new();
        let mut right_draw_items: Vec<(usize, String)> = Vec::new();

        for sp in right_specs {
            if rx + sp.width > w - pad_x + 1.0 {
                break;
            }
            if let HitAction::TrayIcon(ref id) = sp.action {
                tray_screen_rects.push((
                    id.clone(),
                    RECT {
                        left: self.bar_rect.left + rx.round() as i32,
                        top: self.bar_rect.top,
                        right: self.bar_rect.left + (rx + sp.width).round() as i32,
                        bottom: self.bar_rect.bottom,
                    },
                ));
            }
            let item_idx = items.len();
            items.push(HitItem {
                action: sp.action,
                left: rx,
                right: rx + sp.width,
                tooltip: sp.tooltip,
                alert: sp.alert,
                glyph: sp.glyph,
            });
            right_draw_items.push((item_idx, sp.label));
            rx += sp.width + gap;
        }
        tray_proxy::update_icon_rects(tray_screen_rects);

        // --- DESENHO DIRECT2D NA DIB ---
        let dark = self.config.dock_theme == "dark";
        let bg = if dark {
            rgba(0.10, 0.12, 0.15, 0.90)
        } else {
            rgba(0.95, 0.965, 0.985, 0.90)
        };
        let hairline = if dark {
            rgba(1.0, 1.0, 1.0, 0.10)
        } else {
            rgba(0.16, 0.23, 0.31, 0.14)
        };
        let hover_fill = if dark {
            rgba(1.0, 1.0, 1.0, 0.11)
        } else {
            rgba(0.10, 0.16, 0.24, 0.08)
        };
        let chip_fill = if dark {
            rgba(1.0, 1.0, 1.0, 0.06)
        } else {
            rgba(0.10, 0.16, 0.24, 0.05)
        };
        let text_primary = if dark {
            rgba(0.94, 0.96, 0.99, 0.96)
        } else {
            rgba(0.11, 0.16, 0.22, 0.94)
        };
        let text_muted = if dark {
            rgba(0.76, 0.82, 0.89, 0.78)
        } else {
            rgba(0.28, 0.36, 0.45, 0.80)
        };
        let warn_bg = rgba(0.94, 0.56, 0.07, 0.22);
        let warn_fg = if dark {
            rgba(0.98, 0.70, 0.24, 1.0)
        } else {
            rgba(0.76, 0.39, 0.02, 1.0)
        };
        let crit_bg = rgba(0.90, 0.22, 0.22, 0.24);
        let crit_fg = if dark {
            rgba(1.0, 0.46, 0.46, 1.0)
        } else {
            rgba(0.75, 0.11, 0.14, 1.0)
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
                shell_log::error(
                    "topbar_render",
                    "BindDC falhou na Barra Superior",
                    format!("{err}"),
                );
                return;
            }
            self.dc_target.BeginDraw();
            self.dc_target.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));

            if let (
                Ok(bg_brush),
                Ok(line_brush),
                Ok(hover_brush),
                Ok(chip_brush),
                Ok(fg_brush),
                Ok(muted_brush),
                Ok(warn_bg_brush),
                Ok(warn_fg_brush),
                Ok(crit_bg_brush),
                Ok(crit_fg_brush),
            ) = (
                self.dc_target.CreateSolidColorBrush(&bg, None),
                self.dc_target.CreateSolidColorBrush(&hairline, None),
                self.dc_target.CreateSolidColorBrush(&hover_fill, None),
                self.dc_target.CreateSolidColorBrush(&chip_fill, None),
                self.dc_target.CreateSolidColorBrush(&text_primary, None),
                self.dc_target.CreateSolidColorBrush(&text_muted, None),
                self.dc_target.CreateSolidColorBrush(&warn_bg, None),
                self.dc_target.CreateSolidColorBrush(&warn_fg, None),
                self.dc_target.CreateSolidColorBrush(&crit_bg, None),
                self.dc_target.CreateSolidColorBrush(&crit_fg, None),
            ) {
                // Fundo da barra superior + divisor sutil inferior de 1px
                self.dc_target.FillRectangle(
                    &D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: w,
                        bottom: h - 1.0,
                    },
                    &bg_brush,
                );
                self.dc_target.FillRectangle(
                    &D2D_RECT_F {
                        left: 0.0,
                        top: h - 1.0,
                        right: w,
                        bottom: h,
                    },
                    &line_brush,
                );

                // Fundo de hover / chip de widget / alertas
                for item in &items {
                    let is_hovered = self.hovered.as_ref() == Some(&item.action);
                    let pill = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: item.left,
                            top: 4.0 * s,
                            right: item.right,
                            bottom: h - 5.0 * s,
                        },
                        radiusX: 6.0 * s,
                        radiusY: 6.0 * s,
                    };
                    if item.alert == 2 {
                        self.dc_target.FillRoundedRectangle(&pill, &crit_bg_brush);
                    } else if item.alert == 1 {
                        self.dc_target.FillRoundedRectangle(&pill, &warn_bg_brush);
                    } else if is_hovered {
                        self.dc_target.FillRoundedRectangle(&pill, &hover_brush);
                    } else if matches!(item.action, HitAction::Widget(_)) {
                        self.dc_target.FillRoundedRectangle(&pill, &chip_brush);
                    }
                }

                // 1. Logo Pulse
                if let Some(item) = items.first() {
                    let logo_bmp = if dark {
                        self.pulse_logo_dark.as_ref()
                    } else {
                        self.pulse_logo_light.as_ref()
                    };
                    if let Some(bmp) = logo_bmp {
                        let ls = 18.0 * s;
                        let lx = (item.left + item.right - ls) * 0.5;
                        let ly = (h - ls) * 0.5;
                        self.dc_target.DrawBitmap(
                            bmp,
                            Some(&D2D_RECT_F {
                                left: lx,
                                top: ly,
                                right: lx + ls,
                                bottom: ly + ls,
                            }),
                            1.0,
                            windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                            None,
                        );
                    }
                }

                // 2. Nome do app em foco
                if let Some(item) = items.get(1) {
                    let wide: Vec<u16> = app_text.encode_utf16().collect();
                    let rect = D2D_RECT_F {
                        left: item.left + chip_pad,
                        top: 0.0,
                        right: item.right - chip_pad,
                        bottom: h - 1.0,
                    };
                    self.dc_target.DrawText(
                        &wide,
                        &self.font_bold,
                        &rect,
                        &fg_brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }

                // 3. Botão Arquivos
                if let Some(item) = items.get(2) {
                    let glyph = [0xE8B7u16]; // Folder glyph Segoe Fluent/MDL2
                    let gr = D2D_RECT_F {
                        left: item.left + chip_pad,
                        top: 0.0,
                        right: item.left + chip_pad + 15.0 * s,
                        bottom: h - 1.0,
                    };
                    self.dc_target.DrawText(
                        &glyph,
                        &self.font_glyph,
                        &gr,
                        &muted_brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                    let wide: Vec<u16> = "Arquivos".encode_utf16().collect();
                    let tr = D2D_RECT_F {
                        left: item.left + chip_pad + 19.0 * s,
                        top: 0.0,
                        right: item.right - chip_pad,
                        bottom: h - 1.0,
                    };
                    self.dc_target.DrawText(
                        &wide,
                        &self.font_medium,
                        &tr,
                        &fg_brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }

                // 4. Itens da direita
                for (item_idx, label) in right_draw_items {
                    let Some(item) = items.get(item_idx) else {
                        continue;
                    };
                    let brush = match item.alert {
                        2 => &crit_fg_brush,
                        1 => &warn_fg_brush,
                        _ => &fg_brush,
                    };
                    match &item.action {
                        HitAction::TrayIcon(ref id) => {
                            if let Some((_, Some(bmp))) =
                                self.tray_bitmaps.iter().find(|(k, _)| k == id)
                            {
                                let isz = 16.0 * s;
                                let ix = (item.left + item.right - isz) * 0.5;
                                let iy = (h - isz) * 0.5;
                                self.dc_target.DrawBitmap(
                                    bmp,
                                    Some(&D2D_RECT_F {
                                        left: ix,
                                        top: iy,
                                        right: ix + isz,
                                        bottom: iy + isz,
                                    }),
                                    1.0,
                                    windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                                    None,
                                );
                            }
                        }
                        HitAction::Volume => {
                            let glyph = [volume_glyph(
                                self.telemetry.volume_percent.unwrap_or(50),
                                self.telemetry.volume_muted.unwrap_or(false),
                            )];
                            let gr = D2D_RECT_F {
                                left: item.left + chip_pad * 0.75,
                                top: 0.0,
                                right: item.left + chip_pad * 0.75 + 16.0 * s,
                                bottom: h - 1.0,
                            };
                            self.dc_target.DrawText(
                                &glyph,
                                &self.font_glyph,
                                &gr,
                                brush,
                                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                            let wide: Vec<u16> = label.encode_utf16().collect();
                            let tr = D2D_RECT_F {
                                left: item.left + chip_pad * 0.75 + 19.0 * s,
                                top: 0.0,
                                right: item.right - chip_pad * 0.75,
                                bottom: h - 1.0,
                            };
                            self.dc_target.DrawText(
                                &wide,
                                &self.font_medium,
                                &tr,
                                brush,
                                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                        HitAction::Battery => {
                            let pct = self
                                .telemetry
                                .battery
                                .as_ref()
                                .map(|b| b.percent)
                                .unwrap_or(100);
                            let charging =
                                self.telemetry.battery.as_ref().is_some_and(|b| b.charging);
                            let glyph = [battery_glyph(pct, charging)];
                            let gr = D2D_RECT_F {
                                left: item.left + chip_pad * 0.75,
                                top: 0.0,
                                right: item.left + chip_pad * 0.75 + 16.0 * s,
                                bottom: h - 1.0,
                            };
                            self.dc_target.DrawText(
                                &glyph,
                                &self.font_glyph,
                                &gr,
                                brush,
                                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                            let wide: Vec<u16> = label.encode_utf16().collect();
                            let tr = D2D_RECT_F {
                                left: item.left + chip_pad * 0.75 + 19.0 * s,
                                top: 0.0,
                                right: item.right - chip_pad * 0.75,
                                bottom: h - 1.0,
                            };
                            self.dc_target.DrawText(
                                &wide,
                                &self.font_medium,
                                &tr,
                                brush,
                                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                        HitAction::Clock => {
                            let wide: Vec<u16> = label.encode_utf16().collect();
                            let tr = D2D_RECT_F {
                                left: item.left + chip_pad,
                                top: 0.0,
                                right: item.right - chip_pad,
                                bottom: h - 1.0,
                            };
                            self.dc_target.DrawText(
                                &wide,
                                &self.font_medium,
                                &tr,
                                brush,
                                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                        HitAction::Widget(_) => {
                            let text_left = if let Some(g) = item.glyph {
                                let glyph = [g];
                                let gr = D2D_RECT_F {
                                    left: item.left + chip_pad * 0.75,
                                    top: 0.0,
                                    right: item.left + chip_pad * 0.75 + 15.0 * s,
                                    bottom: h - 1.0,
                                };
                                self.dc_target.DrawText(
                                    &glyph,
                                    &self.font_glyph,
                                    &gr,
                                    brush,
                                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                                item.left + chip_pad * 0.75 + 18.0 * s
                            } else {
                                item.left + chip_pad
                            };
                            let text_right = if item.glyph.is_some() {
                                item.right - chip_pad * 0.75
                            } else {
                                item.right - chip_pad
                            };
                            let wide: Vec<u16> = label.encode_utf16().collect();
                            let tr = D2D_RECT_F {
                                left: text_left,
                                top: 0.0,
                                right: text_right,
                                bottom: h - 1.0,
                            };
                            self.dc_target.DrawText(
                                &wide,
                                &self.font_medium,
                                &tr,
                                brush,
                                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                        _ => {}
                    }
                }
                // Indicador visual de arraste (drag-and-drop de widgets / ícones do tray)
                if self.dragging {
                    let accent = rgba(0.937, 0.561, 0.071, 0.95); // Pulse orange
                    if let Ok(accent_brush) = self.dc_target.CreateSolidColorBrush(&accent, None) {
                        let gx = self.drag_x.clamp(2.0, w - 4.0);
                        self.dc_target.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: gx - 1.5 * s,
                                    top: 4.0 * s,
                                    right: gx + 1.5 * s,
                                    bottom: h - 5.0 * s,
                                },
                                radiusX: 1.5 * s,
                                radiusY: 1.5 * s,
                            },
                            &accent_brush,
                        );
                    }
                }
            }

            if let Err(err) = self.dc_target.EndDraw(None, None) {
                shell_log::error(
                    "topbar_render",
                    "EndDraw falhou na Barra Superior",
                    format!("{err}"),
                );
            }
            let _ = GdiFlush();

            let size = SIZE {
                cx: self.surface.width,
                cy: self.surface.height,
            };
            let pos = POINT {
                x: self.bar_rect.left,
                y: self.bar_rect.top,
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
                    "topbar_render",
                    "UpdateLayeredWindow falhou na Barra Superior",
                    format!(
                        "{err} pos=({},{}) size={}x{}",
                        pos.x, pos.y, size.cx, size.cy
                    ),
                );
            } else if !self.first_render_logged {
                self.first_render_logged = true;
                shell_log::info(
                    "topbar_render",
                    format!(
                        "Primeiro frame da Barra Superior renderizado {}x{} scale={:.2} itens={}",
                        size.cx,
                        size.cy,
                        self.scale,
                        items.len()
                    ),
                );
            }
        }

        self.items = items;
    }

    fn font_semi_or_med(&self) -> &IDWriteTextFormat {
        &self.font_medium
    }

    /// Retorna `(largura, texto, tooltip, alert_level)` ou `None` quando o widget
    /// deve ficar oculto (ex.: periféricos desativados ou cotas sem leitura).
    fn build_widget_spec(&self, widget_id: &str) -> Option<(f32, String, String, u8)> {
        let s = self.scale;
        let pad = 8.0 * s;
        let (label, tip, alert) = match widget_id {
            "cpu" => {
                let text = match self.telemetry.cpu_percent {
                    Some(v) => format!("CPU {:.0}%", v),
                    None => "CPU —".to_owned(),
                };
                let tip = match self.telemetry.cpu_percent {
                    Some(v) => format!("Uso total de CPU: {v:.1}% (clique para Desempenho)"),
                    None => "CPU: aguardando leitura (clique para Desempenho)".to_owned(),
                };
                (text, tip, 0)
            }
            "ram" => {
                let text = match self.telemetry.ram_percent {
                    Some(v) => format!("RAM {:.0}%", v),
                    None => "RAM —".to_owned(),
                };
                let tip = self
                    .telemetry
                    .ram_detail
                    .clone()
                    .unwrap_or_else(|| "Memória RAM (clique para Desempenho)".to_owned());
                (text, tip, 0)
            }
            "gpu" => {
                let text = match self.telemetry.gpu_percent {
                    Some(v) => format!("GPU {:.0}%", v),
                    None => "GPU —".to_owned(),
                };
                let tip = match self.telemetry.gpu_percent {
                    Some(v) => format!("Uso de GPU: {v:.1}% (clique para Desempenho)"),
                    None => "GPU: telemetria indisponível neste adaptador".to_owned(),
                };
                (text, tip, 0)
            }
            "network" => {
                let text = match (self.telemetry.net_rx_bps, self.telemetry.net_tx_bps) {
                    (Some(rx), Some(tx)) => {
                        format!("↓{} ↑{}", format_rate_compact(rx), format_rate_compact(tx))
                    }
                    _ => "Rede —".to_owned(),
                };
                let tip = match (self.telemetry.net_rx_bps, self.telemetry.net_tx_bps) {
                    (Some(rx), Some(tx)) => format!(
                        "Rede ativa — Download: {}/s · Upload: {}/s",
                        format_rate_compact(rx),
                        format_rate_compact(tx)
                    ),
                    _ => "Rede: aguardando delta das interfaces ativas".to_owned(),
                };
                (text, tip, 0)
            }
            "peripherals" | "quotas" => return None,
            _ => return None,
        };
        let tw = self.measure_text(&label, &self.font_medium, 180.0 * s);
        Some((tw + pad * 2.0, label, tip, alert))
    }

    fn hit_at(&self, x: f32) -> Option<&HitItem> {
        self.items.iter().find(|it| x >= it.left && x < it.right)
    }

    fn update_hover(&mut self, x: f32) {
        let hit = self.hit_at(x);
        let next_action = hit.map(|h| h.action.clone());
        let next_tip = hit.map(|h| h.tooltip.clone()).unwrap_or_default();

        if next_tip != self.last_tooltip {
            self.last_tooltip = next_tip.clone();
            self.set_tooltip_text(&next_tip);
        }

        if next_action != self.hovered {
            self.hovered = next_action;
            self.build_layout_and_render();
        }
    }

    fn set_tooltip_text(&self, text: &str) {
        let Some(tt) = self.tooltip_hwnd else {
            return;
        };
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let mut ti = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd,
            uId: self.hwnd.0 as usize,
            lpszText: PWSTR(wide.as_mut_ptr()),
            ..Default::default()
        };
        unsafe {
            let _ = SendMessageW(
                tt,
                TTM_UPDATETIPTEXTW,
                Some(WPARAM(0)),
                Some(LPARAM((&mut ti as *mut TTTOOLINFOW) as isize)),
            );
        }
    }

    fn handle_left_click(&mut self, x: f32) {
        let Some(hit) = self.hit_at(x) else {
            return;
        };
        let action = hit.action.clone();
        let center_x = self.bar_rect.left + ((hit.left + hit.right) * 0.5).round() as i32;
        let bottom_y = self.bar_rect.bottom;

        match action {
            HitAction::PulseMenu => self.open_pulse_menu(),
            HitAction::ActiveApp => {
                // Foca ou traz o aplicativo ativo para a frente se minimizado.
            }
            HitAction::FilesButton => super::open_standalone_files_window(),
            HitAction::Widget(id) => match id.as_str() {
                "cpu" | "ram" | "gpu" | "network" => super::open_main_module("Desempenho"),
                "peripherals" => super::toggle_shell_popup_at(
                    "peripherals",
                    center_x,
                    bottom_y,
                    self.monitor,
                    self.scale,
                ),
                "quotas" => super::toggle_shell_popup_at(
                    "quotas",
                    center_x,
                    bottom_y,
                    self.monitor,
                    self.scale,
                ),
                "clock" => super::toggle_shell_popup_at(
                    "clock",
                    center_x,
                    bottom_y,
                    self.monitor,
                    self.scale,
                ),
                _ => {}
            },
            HitAction::TrayIcon(icon_id) => {
                tray_proxy::click_icon_by_id(
                    &icon_id,
                    TrayClickButton::Left,
                    POINT {
                        x: center_x,
                        y: bottom_y,
                    },
                );
            }
            // Clique esquerdo no Volume alterna mudo; botão direito abre o Painel de Áudio.
            HitAction::Volume => telemetry_feed::toggle_master_mute(),
            HitAction::Battery => {
                super::toggle_shell_popup_at("full", center_x, bottom_y, self.monitor, self.scale)
            }
            HitAction::Clock => {
                super::toggle_shell_popup_at("clock", center_x, bottom_y, self.monitor, self.scale)
            }
        }
    }

    fn handle_right_click(&mut self, x: f32) {
        let Some(hit) = self.hit_at(x) else {
            self.open_bar_context_menu(self.bar_rect.left + x.round() as i32);
            return;
        };
        let center_x = self.bar_rect.left + ((hit.left + hit.right) * 0.5).round() as i32;
        let bottom_y = self.bar_rect.bottom;

        match &hit.action {
            HitAction::TrayIcon(icon_id) => {
                tray_proxy::click_icon_by_id(
                    icon_id,
                    TrayClickButton::Right,
                    POINT {
                        x: center_x,
                        y: bottom_y,
                    },
                );
            }
            // Botão direito no Volume abre o Painel de Áudio dedicado do Pulse.
            HitAction::Volume => {
                super::toggle_shell_popup_at("audio", center_x, bottom_y, self.monitor, self.scale)
            }
            HitAction::Widget(id) if id == "quotas" => super::open_main_module("Assinaturas"),
            HitAction::Widget(id) if id == "peripherals" => {
                super::open_main_module("Recursos Beta")
            }
            HitAction::PulseMenu => self.open_pulse_menu(),
            _ => self.open_bar_context_menu(center_x),
        }
    }

    fn handle_double_click(&mut self, x: f32) {
        let Some(hit) = self.hit_at(x) else {
            return;
        };
        if let HitAction::TrayIcon(ref icon_id) = hit.action {
            let center_x = self.bar_rect.left + ((hit.left + hit.right) * 0.5).round() as i32;
            tray_proxy::click_icon_by_id(
                icon_id,
                TrayClickButton::Double,
                POINT {
                    x: center_x,
                    y: self.bar_rect.bottom,
                },
            );
        }
    }

    fn start_drag_candidate(&mut self, x: f32) {
        let Some(hit) = self.hit_at(x) else {
            self.drag_source = None;
            self.dragging = false;
            return;
        };
        let draggable = matches!(
            hit.action,
            HitAction::Widget(_) | HitAction::Clock | HitAction::Volume | HitAction::TrayIcon(_)
        );
        if draggable {
            self.drag_source = Some((hit.action.clone(), x));
            self.dragging = false;
            self.drag_x = x;
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
        } else {
            self.drag_source = None;
            self.dragging = false;
        }
    }

    fn finish_drag_or_click(&mut self, x: f32) {
        let was_dragging = self.dragging;
        let source = self.drag_source.take();
        self.dragging = false;
        unsafe {
            let _ = ReleaseCapture();
        }

        if was_dragging {
            if let Some((src_action, _)) = source {
                self.commit_drag_drop(src_action, x);
            }
            self.build_layout_and_render();
            return;
        }

        self.handle_left_click(x);
    }

    fn commit_drag_drop(&mut self, src: HitAction, drop_x: f32) {
        match src {
            HitAction::TrayIcon(src_id) => {
                // Encontra o ícone da bandeja mais próximo do ponto de soltura.
                let target_id = self
                    .items
                    .iter()
                    .filter_map(|it| match &it.action {
                        HitAction::TrayIcon(id) => {
                            let mid = (it.left + it.right) * 0.5;
                            Some((id.clone(), (drop_x - mid).abs()))
                        }
                        _ => None,
                    })
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(id, _)| id);
                if let Some(target_id) = target_id {
                    if tray_proxy::reorder_icon(&src_id, &target_id) {
                        self.refresh_tray_views();
                    }
                }
            }
            HitAction::Widget(src_w) => {
                self.reorder_topbar_widget(&src_w, drop_x);
            }
            HitAction::Volume => {
                self.reorder_topbar_widget("volume", drop_x);
            }
            HitAction::Clock => {
                self.reorder_topbar_widget("clock", drop_x);
            }
            _ => {}
        }
    }

    fn reorder_topbar_widget(&mut self, src_id: &str, drop_x: f32) {
        let target_id = self
            .items
            .iter()
            .filter_map(|it| {
                let id = match &it.action {
                    HitAction::Widget(w) => w.as_str(),
                    HitAction::Volume => "volume",
                    HitAction::Clock => "clock",
                    HitAction::TrayIcon(_) => "tray",
                    _ => return None,
                };
                let mid = (it.left + it.right) * 0.5;
                Some((id.to_owned(), (drop_x - mid).abs()))
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(id, _)| id);

        let Some(target_id) = target_id else {
            return;
        };
        if target_id == src_id {
            return;
        }
        let mut next = self.config.top_bar_widgets.clone();
        let Some(from_idx) = next.iter().position(|w| w == src_id) else {
            return;
        };
        let Some(to_idx) = next.iter().position(|w| w == &target_id) else {
            return;
        };
        let moved = next.remove(from_idx);
        next.insert(to_idx, moved);

        if let Ok(updated) = crate::beta_features::save_top_bar_widgets(next) {
            self.config = updated.clone();
            super::notify_config_changed(&updated);
        }
    }

    fn open_bar_context_menu(&mut self, screen_x: i32) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN_PULSE as usize, w!("Abrir Pulse"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_FILES as usize,
                w!("Abrir Arquivos (Explorer)"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_RUNNER as usize,
                w!("Abrir Pulse Runner"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_BETA as usize,
                w!("Personalizar Barra Superior e Dock…"),
            );
            let auto_flags = if self.config.dock_auto_hide {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(
                menu,
                auto_flags,
                MENU_TOGGLE_AUTOHIDE as usize,
                w!("Auto-ocultar Dock inferior"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_RESTORE_TASKBAR as usize,
                w!("Restaurar barra do Windows e desligar Pulse Shell"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_EXIT_PULSE as usize,
                w!("Sair do Pulse"),
            );

            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
                screen_x,
                self.bar_rect.bottom,
                Some(0),
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);

            match cmd.0 as u32 {
                MENU_OPEN_PULSE => super::open_main_window(),
                MENU_OPEN_FILES => super::open_standalone_files_window(),
                MENU_OPEN_RUNNER => super::open_launcher_from_shell(),
                MENU_OPEN_BETA => super::open_beta_settings_in_main(),
                MENU_TOGGLE_AUTOHIDE => {
                    if let Ok(updated) = crate::beta_features::toggle_dock_auto_hide() {
                        self.config = updated.clone();
                        super::notify_config_changed(&updated);
                        std::thread::spawn(move || {
                            let _ = super::apply_config(&updated);
                        });
                    }
                }
                MENU_RESTORE_TASKBAR => super::trigger_emergency_restore_from_shell(),
                MENU_EXIT_PULSE => super::exit_pulse_from_shell(),
                _ => {}
            }
        }
    }

    fn handle_wheel(&self, screen_x: i32, delta_y: i16) {
        let local_x = (screen_x - self.bar_rect.left) as f32;
        if let Some(hit) = self.hit_at(local_x) {
            if hit.action == HitAction::Volume {
                let steps = delta_y as f32 / WHEEL_DELTA as f32;
                telemetry_feed::adjust_master_volume_step(0.02 * steps);
            }
        }
    }

    fn open_pulse_menu(&self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN_PULSE as usize, w!("Abrir Pulse"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN_PERF as usize, w!("Desempenho"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_FILES as usize,
                w!("Arquivos (Explorer)"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_STORAGE as usize,
                w!("Armazenamento"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_QUOTAS as usize,
                w!("Assinaturas de IA"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_OPEN_BETA as usize,
                w!("Recursos Beta"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_LOCK as usize, w!("Bloquear tela"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SLEEP as usize, w!("Suspender"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_RESTART as usize, w!("Reiniciar…"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SHUTDOWN as usize, w!("Desligar…"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_RESTORE_TASKBAR as usize,
                w!("Restaurar barra do Windows e desligar Pulse Shell"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_EXIT_PULSE as usize,
                w!("Sair do Pulse"),
            );

            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
                self.bar_rect.left + (8.0 * self.scale).round() as i32,
                self.bar_rect.bottom,
                Some(0),
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);

            match cmd.0 as u32 {
                MENU_OPEN_PULSE => super::open_main_window(),
                MENU_OPEN_PERF => super::open_main_module("Desempenho"),
                MENU_OPEN_FILES => super::open_standalone_files_window(),
                MENU_OPEN_STORAGE => super::open_main_module("Armazenamento"),
                MENU_OPEN_QUOTAS => super::open_main_module("Assinaturas"),
                MENU_OPEN_BETA => super::open_beta_settings_in_main(),
                MENU_LOCK => power::lock_workstation(),
                MENU_SLEEP => power::sleep_system(),
                MENU_RESTART => power::restart_with_confirm(self.hwnd),
                MENU_SHUTDOWN => power::shutdown_with_confirm(self.hwnd),
                MENU_RESTORE_TASKBAR => super::trigger_emergency_restore_from_shell(),
                MENU_EXIT_PULSE => super::exit_pulse_from_shell(),
                _ => {}
            }
        }
    }

    fn refresh_clock(&mut self) {
        let (text, ms_to_next_minute) = format_clock_pt_br();
        let changed = text != self.clock_text;
        self.clock_text = text;
        unsafe {
            let _ = SetTimer(Some(self.hwnd), TIMER_CLOCK_ID, ms_to_next_minute, None);
        }
        if changed {
            self.build_layout_and_render();
        }
    }

    fn refresh_foreground_app(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_invalid() || fg == self.hwnd {
            return;
        }
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(fg, Some(&mut pid));
        }
        // Mantém nome "Pulse" apenas se a janela principal do Pulse estiver em foco;
        // se for a própria Dock/TopBar/TrayProxy, mantém o app anterior ou Pulse.
        if pid == std::process::id() {
            let title = read_window_title(fg);
            if (title == "Pulse" || title.starts_with("Pulse —")) && self.active_app_name != "Pulse"
            {
                self.active_app_name = "Pulse".to_owned();
                self.build_layout_and_render();
            }
            return;
        }

        let name = resolve_active_app_name(fg, pid);
        if name != self.active_app_name {
            self.active_app_name = name;
            self.build_layout_and_render();
        }
    }
}

fn run_topbar_thread(initial_config: BetaFeaturesConfig, ready: mpsc::Sender<Result<(), String>>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let res = run_topbar_loop(initial_config, &ready);
    if let Err(err) = res {
        let _ = ready.send(Err(err));
    }
    TOPBAR_RUNNING.store(false, Ordering::SeqCst);
    TOPBAR_HWND.store(0, Ordering::SeqCst);
    unsafe {
        CoUninitialize();
    }
}

fn run_topbar_loop(
    initial_config: BetaFeaturesConfig,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let d2d_factory: ID2D1Factory = unsafe {
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| format!("D2D1CreateFactory (TopBar) falhou: {e}"))?
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
            .map_err(|e| format!("CreateDCRenderTarget (TopBar) falhou: {e}"))?
    };
    unsafe {
        dc_target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    }

    let dwrite: IDWriteFactory = unsafe {
        DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(|e| format!("DWriteCreateFactory (TopBar) falhou: {e}"))?
    };

    let hinstance =
        unsafe { GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW falhou: {e}"))? };
    let class_name = w!("PulseTopBarClass");
    let wc = WNDCLASSW {
        style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(topbar_wndproc),
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..Default::default()
    };
    unsafe {
        let _ = RegisterClassW(&wc);
    }

    let mon = primary_monitor_rect();
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class_name,
            w!("Pulse Top Bar"),
            WS_POPUP,
            mon.left,
            mon.top,
            (mon.right - mon.left).max(1),
            32,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW (TopBar) falhou: {e}"))?
    };

    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
    let w = (mon.right - mon.left).max(1);
    let h = (BASE_HEIGHT_DIP * scale).round().max(24.0) as i32;
    let surface = DibSurface::new(w, h)?;
    let (font_bold, font_medium, font_glyph) = create_text_formats(&dwrite, scale)?;

    let tooltip_hwnd = create_tooltip_window(hwnd, hinstance.into());
    let (clock_text, ms_next) = format_clock_pt_br();

    let mut state = Box::new(TopBarState {
        hwnd,
        tooltip_hwnd,
        config: initial_config,
        scale,
        monitor: mon,
        bar_rect: RECT {
            left: mon.left,
            top: mon.top,
            right: mon.left + w,
            bottom: mon.top + h,
        },
        appbar_space: None,
        surface,
        dc_target,
        dwrite,
        font_bold,
        font_medium,
        font_glyph,
        pulse_logo_light: None,
        pulse_logo_dark: None,
        tray_bitmaps: Vec::new(),
        tray_views: Vec::new(),
        telemetry: telemetry_feed::latest_snapshot(),
        active_app_name: "Pulse".to_owned(),
        clock_text,
        items: Vec::new(),
        hovered: None,
        last_tooltip: String::new(),
        tracking_mouse: false,
        fullscreen: false,
        first_render_logged: false,
        drag_source: None,
        dragging: false,
        drag_x: 0.0,
    });

    state.update_fonts();
    state.sync_geometry();
    state.refresh_tray_views();
    state.refresh_foreground_app();
    state.build_layout_and_render();

    // Inicia reserva de WorkArea superior em thread dedicada (não bloqueia nem aqui nem no UI thread).
    match AppBarSpace::start(AppBarEdge::Top, h) {
        Ok(space) => state.appbar_space = Some(space),
        Err(err) => shell_log::error("topbar_appbar", "Falha ao iniciar AppBarSpace(top)", &err),
    }

    let state_ptr = Box::into_raw(state);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        let _ = SetTimer(Some(hwnd), TIMER_CLOCK_ID, ms_next, None);
        let _ = SetTimer(Some(hwnd), TIMER_RECONCILE_ID, RECONCILE_MS, None);
    }

    TOPBAR_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    TOPBAR_RUNNING.store(true, Ordering::SeqCst);
    telemetry_feed::bind_topbar(hwnd, WM_APP_TELEMETRY_UPDATED);

    let hooks = install_foreground_hooks();
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
        let _ = KillTimer(Some(hwnd), TIMER_CLOCK_ID);
        let _ = KillTimer(Some(hwnd), TIMER_RECONCILE_ID);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let mut boxed = Box::from_raw(state_ptr);
        if let Some(space) = boxed.appbar_space.take() {
            space.close();
        }
        if let Some(tt) = boxed.tooltip_hwnd.take() {
            let _ = DestroyWindow(tt);
        }
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(class_name, Some(hinstance.into()));
    }

    Ok(())
}

fn install_foreground_hooks() -> Vec<HWINEVENTHOOK> {
    let mut hooks = Vec::new();
    for (min, max) in [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW),
    ] {
        let hook = unsafe {
            SetWinEventHook(
                min,
                max,
                None,
                Some(fg_event_proc),
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

unsafe extern "system" fn fg_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    let raw = TOPBAR_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    if event == EVENT_OBJECT_SHOW {
        if !super::windows::is_native_hidden() || hwnd.is_invalid() {
            return;
        }
        if !GetPropW(hwnd, PROXY_PROP).is_invalid() {
            return;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return;
        }
        let mut buf = [0u16; 32];
        let len = GetClassNameW(hwnd, &mut buf).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..len]);
        if matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            let _ = PostMessageW(
                Some(HWND(raw as *mut _)),
                WM_APP_ENFORCE_TASKBAR_HIDE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        return;
    }
    let _ = PostMessageW(
        Some(HWND(raw as *mut _)),
        WM_APP_FOREGROUND_CHANGED,
        WPARAM(0),
        LPARAM(0),
    );
}

unsafe extern "system" fn topbar_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TopBarState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let state = &mut *ptr;

    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE),
        WM_LBUTTONDOWN => {
            let x = (lp.0 & 0xFFFF) as i16 as f32;
            state.start_drag_candidate(x);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lp.0 & 0xFFFF) as i16 as f32;
            if let Some((_, start_x)) = state.drag_source {
                if state.dragging || (x - start_x).abs() >= 6.0 * state.scale {
                    state.dragging = true;
                    state.drag_x = x;
                    state.build_layout_and_render();
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
            state.update_hover(x);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            state.tracking_mouse = false;
            if !state.dragging && state.hovered.take().is_some() {
                state.build_layout_and_render();
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if state.dragging || state.drag_source.is_some() {
                state.dragging = false;
                state.drag_source = None;
                state.build_layout_and_render();
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lp.0 & 0xFFFF) as i16 as f32;
            state.finish_drag_or_click(x);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let x = (lp.0 & 0xFFFF) as i16 as f32;
            state.handle_right_click(x);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let x = (lp.0 & 0xFFFF) as i16 as f32;
            state.handle_double_click(x);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let delta = ((wp.0 >> 16) as u16) as i16;
            state.handle_wheel(x, delta);
            LRESULT(0)
        }
        WM_TIMER => {
            match wp.0 {
                TIMER_CLOCK_ID => state.refresh_clock(),
                TIMER_RECONCILE_ID => {
                    state.refresh_foreground_app();
                    let on = super::dock_window::is_foreground_fullscreen_on_primary(state.hwnd);
                    if on != state.fullscreen {
                        state.fullscreen = on;
                        let _ =
                            ShowWindow(state.hwnd, if on { SW_HIDE } else { SW_SHOWNOACTIVATE });
                        if !on {
                            state.build_layout_and_render();
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_ENFORCE_TASKBAR_HIDE => {
            if super::windows::is_native_hidden() {
                watchdog::enforce_native_taskbar_hidden();
            }
            LRESULT(0)
        }
        WM_APP_APPLY_CONFIG => {
            if let Ok(cfg) = config_slot().lock() {
                state.config = cfg.clone();
            }
            state.build_layout_and_render();
            LRESULT(0)
        }
        WM_APP_TRAY_UPDATED => {
            state.refresh_tray_views();
            state.build_layout_and_render();
            LRESULT(0)
        }
        WM_APP_TELEMETRY_UPDATED => {
            state.telemetry = telemetry_feed::latest_snapshot();
            state.build_layout_and_render();
            LRESULT(0)
        }
        WM_APP_FOREGROUND_CHANGED => {
            state.refresh_foreground_app();
            LRESULT(0)
        }
        WM_TIMECHANGE | WM_POWERBROADCAST => {
            state.refresh_clock();
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            let _ = ReplyMessage(LRESULT(0));
            state.sync_geometry();
            state.build_layout_and_render();
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
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn create_tooltip_window(
    parent: HWND,
    hinst: windows::Win32::Foundation::HINSTANCE,
) -> Option<HWND> {
    unsafe {
        let tt = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            TOOLTIPS_CLASSW,
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(hinst),
            None,
        )
        .ok()?;
        let mut ti = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS,
            hwnd: parent,
            uId: parent.0 as usize,
            rect: RECT {
                left: 0,
                top: 0,
                right: 10000,
                bottom: 200,
            },
            lpszText: PWSTR::null(),
            ..Default::default()
        };
        let _ = SendMessageW(
            tt,
            TTM_ADDTOOLW,
            Some(WPARAM(0)),
            Some(LPARAM((&mut ti as *mut TTTOOLINFOW) as isize)),
        );
        Some(tt)
    }
}

fn create_text_formats(
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
    let icons = if has_font(w!("Segoe Fluent Icons")) {
        w!("Segoe Fluent Icons")
    } else {
        w!("Segoe MDL2 Assets")
    };

    let make = |fam: PCWSTR,
                weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
                size: f32,
                center: bool|
     -> Result<IDWriteTextFormat, String> {
        unsafe {
            let fmt = dwrite
                .CreateTextFormat(
                    fam,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * scale,
                    w!("pt-BR"),
                )
                .map_err(|e| format!("CreateTextFormat falhou: {e}"))?;
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = fmt.SetTextAlignment(if center {
                DWRITE_TEXT_ALIGNMENT_CENTER
            } else {
                DWRITE_TEXT_ALIGNMENT_LEADING
            });
            Ok(fmt)
        }
    };

    Ok((
        make(family, DWRITE_FONT_WEIGHT_BOLD, 12.5, false)?,
        make(family, DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0, false)?,
        make(
            icons,
            windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL,
            13.5,
            true,
        )?,
    ))
}

fn format_clock_pt_br() -> (String, u32) {
    let now = Local::now();
    let weekday = match now.weekday() {
        chrono::Weekday::Sun => "dom",
        chrono::Weekday::Mon => "seg",
        chrono::Weekday::Tue => "ter",
        chrono::Weekday::Wed => "qua",
        chrono::Weekday::Thu => "qui",
        chrono::Weekday::Fri => "sex",
        chrono::Weekday::Sat => "sáb",
    };
    let month = match now.month() {
        1 => "jan",
        2 => "fev",
        3 => "mar",
        4 => "abr",
        5 => "mai",
        6 => "jun",
        7 => "jul",
        8 => "ago",
        9 => "set",
        10 => "out",
        11 => "nov",
        _ => "dez",
    };
    let text = format!(
        "{:02}:{:02} · {} {} {}",
        now.hour(),
        now.minute(),
        weekday,
        now.day(),
        month
    );
    let secs_left = 60u32.saturating_sub(now.second()).max(1);
    (text, secs_left * 1000)
}

fn peripheral_glyph(kind: crate::peripheral_battery::PeripheralKind) -> u16 {
    match kind {
        crate::peripheral_battery::PeripheralKind::Mouse => 0xE962,
        crate::peripheral_battery::PeripheralKind::Keyboard => 0xE765,
        crate::peripheral_battery::PeripheralKind::Headset => 0xE7F6,
        crate::peripheral_battery::PeripheralKind::Controller => 0xE7FC,
        crate::peripheral_battery::PeripheralKind::Stylus => 0xED63,
        crate::peripheral_battery::PeripheralKind::Other => 0xE702,
    }
}

fn volume_glyph(percent: u8, muted: bool) -> u16 {
    if muted {
        0xE74F
    } else if percent == 0 {
        0xE992
    } else if percent < 34 {
        0xE993
    } else if percent < 67 {
        0xE994
    } else {
        0xE995
    }
}

fn battery_glyph(percent: u8, charging: bool) -> u16 {
    if charging {
        0xEBB5
    } else if percent <= 10 {
        0xEBA0
    } else if percent <= 30 {
        0xEBA2
    } else if percent <= 60 {
        0xEBA5
    } else if percent <= 85 {
        0xEBA8
    } else {
        0xEBAA
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

fn read_window_title(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..copied.max(0) as usize])
        .trim()
        .to_owned()
}

fn resolve_active_app_name(hwnd: HWND, pid: u32) -> String {
    let title = read_window_title(hwnd);
    if title == "Program Manager" {
        return "Área de Trabalho".to_owned();
    }
    let exe = resolve_host_exe_path(pid);
    if let Some(ref path) = exe {
        let stem = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .trim();
        if stem.eq_ignore_ascii_case("ApplicationFrameHost") {
            if let Some(aumid) = window_aumid(hwnd) {
                if let Some(friendly) = friendly_name_from_aumid(&aumid) {
                    return friendly;
                }
            }
            if !title.is_empty() {
                return title.chars().take(28).collect();
            }
        }
        if stem.eq_ignore_ascii_case("explorer") && title.is_empty() {
            return "Área de Trabalho".to_owned();
        }
        if !stem.is_empty() {
            let prettified = prettify_exe_name(stem);
            let friendly = (!prettified.eq_ignore_ascii_case(stem)).then_some(prettified.as_str());
            let official = crate::process_identity::official_name_for_path(path);
            let chosen =
                crate::process_identity::choose_display_name(stem, official.as_deref(), friendly);
            // Jogo com exe técnico: o título da janela costuma ser mais fiel
            // que o nome do arquivo quando o PE não traz um nome oficial.
            if chosen.eq_ignore_ascii_case(stem) && !title.is_empty() {
                return title.chars().take(28).collect();
            }
            return chosen.chars().take(28).collect();
        }
    }
    if !title.is_empty() {
        title.chars().take(28).collect()
    } else {
        "Área de Trabalho".to_owned()
    }
}

fn prettify_exe_name(stem: &str) -> String {
    match stem.to_ascii_lowercase().as_str() {
        "explorer" => "Explorador".to_owned(),
        "msedge" => "Microsoft Edge".to_owned(),
        "chrome" => "Google Chrome".to_owned(),
        "firefox" => "Firefox".to_owned(),
        "code" => "VS Code".to_owned(),
        "windowsterminal" => "Terminal".to_owned(),
        "notepad" => "Bloco de Notas".to_owned(),
        "whatsapp.root" => "WhatsApp".to_owned(),
        "pulse-windows" => "Pulse".to_owned(),
        _ => stem.to_owned(),
    }
}

fn window_aumid(hwnd: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let val = store.GetValue(&PKEY_AUMID).ok()?;
        let text = val.to_string();
        (!text.is_empty()).then_some(text)
    }
}

fn friendly_name_from_aumid(aumid: &str) -> Option<String> {
    let base = aumid.split('!').next()?.split('_').next()?;
    let short = base.rsplit('.').next().unwrap_or(base).trim();
    (!short.is_empty()).then(|| short.to_owned())
}

pub fn resolve_host_exe_path(pid: u32) -> Option<String> {
    let path = process_image_path(pid)?;
    let is_webview2 = Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("msedgewebview2.exe"));
    if !is_webview2 {
        return Some(path);
    }
    let parents = snapshot_parent_pids();
    let mut cur = pid;
    for _ in 0..5 {
        let Some(&parent) = parents.get(&cur) else {
            break;
        };
        if let Some(parent_path) = process_image_path(parent) {
            let still_wv2 = Path::new(&parent_path)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("msedgewebview2.exe"));
            if !still_wv2 {
                return Some(parent_path);
            }
        }
        cur = parent;
    }
    Some(path)
}

fn process_image_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if !ok || len == 0 {
            return None;
        }
        let s = OsString::from_wide(&buf[..len as usize])
            .to_string_lossy()
            .into_owned();
        (!s.is_empty()).then_some(s)
    }
}

fn snapshot_parent_pids() -> std::collections::HashMap<u32, u32> {
    let mut map = std::collections::HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return map;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    map
}
