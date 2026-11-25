//! Reserva de WorkArea via `SHAppBarMessage` (`ABE_TOP` e `ABE_BOTTOM`) em
//! threads dedicadas (`AppBarSpace`).
//!
//! Arquitetura anti-deadlock baseada no padrão MIT de `wisp/src/dock/space.rs`:
//! 1. Cada borda (`Top` e `Bottom`) vive em sua própria thread Win32 com janela
//!    invisível exclusiva para `ABM_NEW` / `ABM_QUERYPOS` / `ABM_SETPOS` / `ABM_REMOVE`.
//! 2. `AppBarSpace::start()` sinaliza `ready` **antes** de falar com o Explorer,
//!    nunca bloqueando a UI do Tauri nem o primeiro frame da TopBar/Dock.
//! 3. As mensagens são enviadas diretamente ao `HWND` real do `explorer.exe` via
//!    `SHAllocShared` + `SendMessageTimeoutW(..., SMTO_NORMAL | SMTO_ABORTIFHUNG)`
//!    para nunca cair no nosso proxy `Shell_TrayWnd`.
//! 4. `placed_for` impede loops infinitos de `ABM_SETPOS -> WM_SETTINGCHANGE -> ABM_SETPOS`
//!    entre a Barra Superior e a Dock.

use super::{shell_log, watchdog};
use std::{
    cell::Cell,
    mem::size_of,
    os::windows::io::AsRawHandle,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    core::w,
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, POINT, RECT, WAIT_OBJECT_0, WPARAM},
        Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY},
        System::{DataExchange::COPYDATASTRUCT, LibraryLoader::GetModuleHandleW},
        UI::{
            Shell::{
                SHAllocShared, SHAppBarMessage, SHFreeShared, SHLockShared, SHUnlockShared,
                ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABN_POSCHANGED,
                APPBARDATA,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                KillTimer, MsgWaitForMultipleObjectsEx, PeekMessageW, PostMessageW,
                PostQuitMessage, RegisterClassW, RegisterWindowMessageW, ReplyMessage,
                SendMessageTimeoutW, SetTimer, TranslateMessage, MSG, MWMO_INPUTAVAILABLE,
                PM_NOREMOVE, QS_SENDMESSAGE, SMTO_ABORTIFHUNG, SMTO_NORMAL, WM_APP, WM_CLOSE,
                WM_COPYDATA, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_SETTINGCHANGE, WM_TIMER,
                WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
            },
        },
    },
};

const WM_APPBAR_CALLBACK: u32 = WM_APP + 40;
const WM_SPACE_SET_THICKNESS: u32 = WM_APP + 41;
const WM_SPACE_CHECK: u32 = WM_APP + 42;

const CHECK_TIMER_ID: usize = 1;
const CHECK_TIMER_MS: u32 = 1500;
const CLOSE_WAIT: Duration = Duration::from_secs(3);
const SEND_TIMEOUT_MS: u32 = 3500;
const REREGISTER_BACKOFF_TICKS: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppBarEdge {
    Top,
    Bottom,
}

impl AppBarEdge {
    fn as_win32(self) -> u32 {
        match self {
            Self::Top => ABE_TOP,
            Self::Bottom => ABE_BOTTOM,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

/// Faixa desejada ao longo da borda do monitor.
pub fn compute_strip(edge: AppBarEdge, screen: RECT, thickness: i32) -> RECT {
    let t = thickness.max(1);
    match edge {
        AppBarEdge::Top => RECT {
            left: screen.left,
            top: screen.top,
            right: screen.right,
            bottom: screen.top + t,
        },
        AppBarEdge::Bottom => RECT {
            left: screen.left,
            top: screen.bottom - t,
            right: screen.right,
            bottom: screen.bottom,
        },
    }
}

/// Reajusta a espessura após `ABM_QUERYPOS` caso outra barra tenha empurrado a faixa.
pub fn keep_thickness(edge: AppBarEdge, mut rect: RECT, thickness: i32) -> RECT {
    let t = thickness.max(1);
    match edge {
        AppBarEdge::Top => rect.bottom = rect.top + t,
        AppBarEdge::Bottom => rect.top = rect.bottom - t,
    }
    rect
}

/// Formato fixo de `APPBARDATA` enviado pelo `SHAppBarMessage` ao `Shell_TrayWnd`
/// (compatível com callers 32-bit e 64-bit).
#[repr(C)]
#[derive(Clone, Copy)]
struct WireData {
    cb_size: u32,
    hwnd: u32,
    callback: u32,
    edge: u32,
    rc: RECT,
    lparam: i64,
}

#[repr(C)]
struct WireMessage {
    data: WireData,
    message: u32,
    shared: u64,
    shared_process: u32,
}

/// Envia uma mensagem AppBar diretamente para a `Shell_TrayWnd` real do Explorer,
/// alocando o bloco compartilhado dentro do processo do Explorer quando a
/// operação retorna retângulo (`ABM_QUERYPOS`, `ABM_SETPOS`).
pub fn send_appbar_message(message: u32, data: &mut APPBARDATA) -> usize {
    let Some((tray, explorer_pid)) = watchdog::find_explorer_primary_taskbar() else {
        return unsafe { SHAppBarMessage(message, data) };
    };

    let wire = WireData {
        cb_size: size_of::<WireData>() as u32,
        hwnd: data.hWnd.0 as usize as u32,
        callback: data.uCallbackMessage,
        edge: data.uEdge,
        rc: data.rc,
        lparam: data.lParam.0 as i64,
    };

    let returns_rect = matches!(message, ABM_QUERYPOS | ABM_SETPOS);
    let shared = if returns_rect {
        unsafe {
            SHAllocShared(
                Some((&wire as *const WireData).cast()),
                size_of::<WireData>() as u32,
                explorer_pid,
            )
        }
    } else {
        HANDLE::default()
    };

    let packet = WireMessage {
        data: wire,
        message,
        shared: shared.0 as usize as u64,
        shared_process: explorer_pid,
    };

    let copy = COPYDATASTRUCT {
        dwData: 0,
        cbData: size_of::<WireMessage>() as u32,
        lpData: (&packet as *const WireMessage).cast_mut().cast(),
    };

    let mut result = 0usize;
    unsafe {
        let sent = SendMessageTimeoutW(
            tray,
            WM_COPYDATA,
            WPARAM(data.hWnd.0 as usize),
            LPARAM(&copy as *const _ as isize),
            SMTO_NORMAL | SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            Some(&mut result),
        );
        if sent.0 == 0 {
            shell_log::warn(
                "appbar",
                format!(
                    "SendMessageTimeoutW falhou/timeout para msg={message} tray={:?}",
                    tray.0
                ),
            );
        }
        if !shared.is_invalid() && !shared.0.is_null() {
            let view = SHLockShared(shared, explorer_pid) as *const WireData;
            if !view.is_null() {
                data.rc = (*view).rc;
                let _ = SHUnlockShared(view.cast());
            }
            let _ = SHFreeShared(shared, explorer_pid);
        }
    }
    result
}

/// Controlador de uma faixa AppBar em thread dedicada (nunca bloqueia quem cria).
pub struct AppBarSpace {
    hwnd: HWND,
    edge: AppBarEdge,
    thread: Option<JoinHandle<()>>,
}

impl AppBarSpace {
    pub fn start(edge: AppBarEdge, thickness_px: i32) -> Result<Self, String> {
        let thickness = thickness_px.max(1);
        let (tx, rx) = mpsc::channel();
        let name = format!("pulse-appbar-{}", edge.label());
        let handle = thread::Builder::new()
            .name(name)
            .spawn(move || run_space_thread(edge, thickness, tx))
            .map_err(|e| format!("Falha ao iniciar thread AppBar ({}): {e}", edge.label()))?;

        match rx.recv() {
            Ok(Ok(raw)) => Ok(Self {
                hwnd: HWND(raw as *mut _),
                edge,
                thread: Some(handle),
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err(format!(
                    "Thread AppBar ({}) encerrou antes de criar janela.",
                    edge.label()
                ))
            }
        }
    }

    pub fn set_thickness(&self, thickness_px: i32) {
        let t = thickness_px.max(1);
        unsafe {
            let _ = PostMessageW(
                Some(self.hwnd),
                WM_SPACE_SET_THICKNESS,
                WPARAM(t as usize),
                LPARAM(0),
            );
        }
    }

    #[allow(dead_code)]
    pub fn check(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_SPACE_CHECK, WPARAM(0), LPARAM(0));
        }
    }

    pub fn close(mut self) {
        self.request_stop();
        let Some(handle) = self.thread.take() else {
            return;
        };
        let raw_handle = [HANDLE(handle.as_raw_handle())];
        let deadline = Instant::now() + CLOSE_WAIT;
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
        shell_log::info(
            "appbar",
            format!("AppBarSpace({}) encerrado", self.edge.label()),
        );
    }

    fn request_stop(&self) {
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl Drop for AppBarSpace {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.request_stop();
        }
    }
}

thread_local! {
    static EDGE: Cell<AppBarEdge> = const { Cell::new(AppBarEdge::Top) };
    static THICKNESS: Cell<i32> = const { Cell::new(32) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    static LAST_STRIP: Cell<RECT> = const { Cell::new(RECT { left: 0, top: 0, right: 0, bottom: 0 }) };
    static PLACED_FOR: Cell<Option<(RECT, i32)>> = const { Cell::new(None) };
    static EXPLORER_HWND: Cell<isize> = const { Cell::new(0) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static BACKOFF: Cell<u32> = const { Cell::new(0) };
}

fn run_space_thread(edge: AppBarEdge, thickness: i32, ready: mpsc::Sender<Result<isize, String>>) {
    EDGE.set(edge);
    THICKNESS.set(thickness);
    REGISTERED.set(false);
    LAST_STRIP.set(RECT::default());
    PLACED_FOR.set(None);
    BACKOFF.set(0);
    EXPLORER_HWND.set(
        watchdog::find_explorer_primary_taskbar()
            .map(|(h, _)| h.0 as isize)
            .unwrap_or(0),
    );

    let hwnd = match create_space_window(edge) {
        Ok(h) => h,
        Err(err) => {
            shell_log::error("appbar", "Falha ao criar janela AppBarSpace", &err);
            let _ = ready.send(Err(err));
            return;
        }
    };

    // Sinaliza ANTES de falar com o Explorer (regra anti-deadlock do Wisp).
    if ready.send(Ok(hwnd.0 as isize)).is_err() {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return;
    }

    register_and_place(hwnd);

    unsafe {
        let _ = SetTimer(Some(hwnd), CHECK_TIMER_ID, CHECK_TIMER_MS, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn create_space_window(edge: AppBarEdge) -> Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
        let class = match edge {
            AppBarEdge::Top => w!("PulseAppBarSpaceTop"),
            AppBarEdge::Bottom => w!("PulseAppBarSpaceBottom"),
        };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(space_wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW(AppBarSpace): {e}"))
    }
}

fn primary_monitor_and_work() -> Option<(RECT, RECT)> {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(hmon, &mut info)
            .as_bool()
            .then_some((info.rcMonitor, info.rcWork))
    }
}

fn base_data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: WM_APPBAR_CALLBACK,
        uEdge: EDGE.get().as_win32(),
        ..Default::default()
    }
}

fn is_workarea_kept() -> bool {
    let Some((mon, work)) = primary_monitor_and_work() else {
        return true;
    };
    let t = THICKNESS.get().max(1);
    match EDGE.get() {
        AppBarEdge::Top => work.top >= mon.top + t,
        AppBarEdge::Bottom => work.bottom <= mon.bottom - t,
    }
}

fn register_and_place(hwnd: HWND) {
    let edge = EDGE.get();
    let t0 = Instant::now();
    let mut data = base_data(hwnd);
    let ok = send_appbar_message(ABM_NEW, &mut data) != 0;
    REGISTERED.set(ok);
    if !ok {
        shell_log::warn(
            "appbar",
            format!(
                "ABM_NEW({}) retornou 0 após {}ms",
                edge.label(),
                t0.elapsed().as_millis()
            ),
        );
        return;
    }
    place(hwnd);
}

fn unregister(hwnd: HWND) {
    if !REGISTERED.get() {
        return;
    }
    let mut data = base_data(hwnd);
    send_appbar_message(ABM_REMOVE, &mut data);
    REGISTERED.set(false);
    PLACED_FOR.set(None);
}

fn place(hwnd: HWND) {
    let Some((mon, _)) = primary_monitor_and_work() else {
        return;
    };
    let edge = EDGE.get();
    let thickness = THICKNESS.get().max(1);
    let t0 = Instant::now();

    let mut data = base_data(hwnd);
    data.rc = compute_strip(edge, mon, thickness);
    send_appbar_message(ABM_QUERYPOS, &mut data);
    data.rc = keep_thickness(edge, data.rc, thickness);
    send_appbar_message(ABM_SETPOS, &mut data);

    LAST_STRIP.set(data.rc);
    PLACED_FOR.set(Some((mon, thickness)));
    shell_log::info(
        "appbar",
        format!(
            "ABM_SETPOS({}) thickness={} rect=({},{},{},{}) em {}ms",
            edge.label(),
            thickness,
            data.rc.left,
            data.rc.top,
            data.rc.right,
            data.rc.bottom,
            t0.elapsed().as_millis()
        ),
    );
}

fn place_if_changed(hwnd: HWND) {
    let Some((mon, _)) = primary_monitor_and_work() else {
        return;
    };
    let target = (mon, THICKNESS.get().max(1));
    if PLACED_FOR.get() != Some(target) {
        if REGISTERED.get() {
            place(hwnd);
        } else {
            register_and_place(hwnd);
        }
    }
}

fn check_and_repair(hwnd: HWND) {
    if REGISTERED.get() && is_workarea_kept() {
        BACKOFF.set(0);
        return;
    }
    if !REGISTERED.get() {
        let backoff = BACKOFF.get();
        if backoff == 0 {
            register_and_place(hwnd);
            BACKOFF.set(REREGISTER_BACKOFF_TICKS);
        } else {
            BACKOFF.set(backoff.saturating_sub(1));
        }
        return;
    }
    place(hwnd);
    if !is_workarea_kept() {
        let backoff = BACKOFF.get();
        if backoff == 0 {
            unregister(hwnd);
            register_and_place(hwnd);
            BACKOFF.set(REREGISTER_BACKOFF_TICKS);
        } else {
            BACKOFF.set(backoff.saturating_sub(1));
        }
    }
}

unsafe extern "system" fn space_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;

    let tb_created = TASKBAR_CREATED.get();
    if tb_created != 0 && msg == tb_created {
        // Só re-registra se o Explorer realmente reiniciou (HWND novo),
        // ignorando broadcasts do nosso próprio tray_proxy.
        let current_exp = watchdog::find_explorer_primary_taskbar()
            .map(|(h, _)| h.0 as isize)
            .unwrap_or(0);
        if current_exp != 0 && current_exp != EXPLORER_HWND.get() {
            EXPLORER_HWND.set(current_exp);
            shell_log::info("appbar", "Explorer reiniciou; re-registrando AppBar");
            REGISTERED.set(false);
            PLACED_FOR.set(None);
            register_and_place(hwnd);
        }
        return LRESULT(0);
    }

    match msg {
        WM_APPBAR_CALLBACK if wparam.0 as u32 == ABN_POSCHANGED => {
            if !is_workarea_kept() {
                place(hwnd);
            }
            LRESULT(0)
        }
        WM_SPACE_SET_THICKNESS => {
            let next = (wparam.0 as i32).max(1);
            if next != THICKNESS.get() {
                THICKNESS.set(next);
                place_if_changed(hwnd);
            }
            LRESULT(0)
        }
        WM_SPACE_CHECK => {
            place_if_changed(hwnd);
            check_and_repair(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == CHECK_TIMER_ID => {
            place_if_changed(hwnd);
            check_and_repair(hwnd);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            let _ = ReplyMessage(LRESULT(0));
            place_if_changed(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = KillTimer(Some(hwnd), CHECK_TIMER_ID);
            unregister(hwnd);
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    #[test]
    fn strip_and_keep_thickness_preserve_requested_depth() {
        let top = compute_strip(AppBarEdge::Top, SCREEN, 32);
        assert_eq!((top.top, top.bottom), (0, 32));
        let pushed_top = RECT {
            top: 20,
            bottom: 32,
            ..SCREEN
        };
        assert_eq!(keep_thickness(AppBarEdge::Top, pushed_top, 32).bottom, 52);

        let bottom = compute_strip(AppBarEdge::Bottom, SCREEN, 72);
        assert_eq!((bottom.top, bottom.bottom), (1008, 1080));
        let pushed_bottom = RECT {
            top: 950,
            bottom: 1032,
            ..SCREEN
        };
        assert_eq!(
            keep_thickness(AppBarEdge::Bottom, pushed_bottom, 72).top,
            960
        );
    }
}
