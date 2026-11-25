//! Watchdog e controle reversível da barra de tarefas nativa do Windows + WorkArea.
//!
//! Quando o Pulse Shell (`customDock`) é ativado, salva um snapshot do estado
//! original da taskbar (`ABM_GETSTATE`, regiões e WorkArea de cada monitor) e
//! inicia um processo sentinela (`pulse-windows.exe --dock-watchdog <pid>`)
//! bloqueado no kernel em `WaitForMultipleObjects`.
//!
//! Se o processo principal cair (inclusive `panic = "abort"` ou encerramento
//! forçado), o sentinela restaura imediatamente:
//! 1. `Shell_TrayWnd` + `Shell_SecondaryTrayWnd` (região e visibilidade)
//! 2. Estado original de auto-ocultação (`ABM_SETSTATE`)
//! 3. Área útil de trabalho de cada monitor (`SystemParametersInfoW(SPI_SETWORKAREA)`
//!    + broadcast `WM_SETTINGCHANGE`) para nunca deixar espaço morto na tela.

use super::{appbar, shell_log, DOCK_WATCHDOG_ARG};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    mem::size_of,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows::{
    core::{w, BOOL, HSTRING, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT, WAIT_OBJECT_0, WPARAM},
        Graphics::Gdi::{
            CreateRectRgn, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, GetWindowRgnBox,
            SetWindowRgn, HDC, HMONITOR, MONITORINFO, NULLREGION, SIMPLEREGION,
        },
        System::Threading::{
            CreateEventW, OpenEventW, OpenProcess, SetEvent, WaitForMultipleObjects, INFINITE,
            PROCESS_SYNCHRONIZE, SYNCHRONIZATION_SYNCHRONIZE,
        },
        UI::{
            Shell::{ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA},
            WindowsAndMessaging::{
                FindWindowExW, GetPropW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
                SendNotifyMessageW, ShowWindow, SystemParametersInfoW, HWND_BROADCAST,
                SPIF_SENDCHANGE, SPI_SETWORKAREA, SW_HIDE, SW_SHOWNA,
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_SETTINGCHANGE,
            },
        },
    },
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DISARM_EVENT_ARG: &str = "--disarm-event";
/// Propriedade marcada na janela `Shell_TrayWnd` do nosso tray proxy para nunca
/// confundi-la com a barra verdadeira do `explorer.exe`.
pub const PROXY_PROP: PCWSTR = w!("Pulse.TrayProxy");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl From<RECT> for SavedRect {
    fn from(r: RECT) -> Self {
        Self {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

impl From<SavedRect> for RECT {
    fn from(r: SavedRect) -> Self {
        Self {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MonitorSnapshot {
    bounds: SavedRect,
    work: SavedRect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BarSnapshot {
    hwnd: usize,
    class: String,
    rect: SavedRect,
    region: Option<SavedRect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellRecoverySnapshot {
    pub original_taskbar_state: u32,
    monitors: Vec<MonitorSnapshot>,
    bars: Vec<BarSnapshot>,
}

struct WatchdogHandle {
    child: Child,
    disarm_event: Option<HANDLE>,
}

// `HANDLE` do Win32 não é `Send` por conter `*mut c_void`, mas o kernel event
// handle pode ser sinalizado e fechado com segurança de outra thread sob o Mutex.
unsafe impl Send for WatchdogHandle {}

static WATCHDOG_CHILD: OnceLock<Mutex<Option<WatchdogHandle>>> = OnceLock::new();

fn watchdog_slot() -> &'static Mutex<Option<WatchdogHandle>> {
    WATCHDOG_CHILD.get_or_init(|| Mutex::new(None))
}

fn recovery_file() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join("shell-recovery.json")
}

/// Ponto de entrada do processo sentinela: aguarda o término do PID pai (ou o
/// evento de desarme limpo) e restaura taskbar + WorkArea se o pai morreu.
pub fn run_watchdog_loop(parent_pid: u32, extra_args: &[String]) {
    let disarm_name = extra_args
        .windows(2)
        .find(|w| w[0] == DISARM_EVENT_ARG)
        .map(|w| w[1].as_str());

    let mut handles: Vec<HANDLE> = Vec::with_capacity(2);
    let disarm_handle = disarm_name.and_then(|name| unsafe {
        OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)).ok()
    });
    if let Some(ev) = disarm_handle {
        handles.push(ev);
    }

    let proc_handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid).ok() };
    if let Some(ph) = proc_handle {
        handles.push(ph);
    }

    let disarmed_cleanly = if !handles.is_empty() {
        let wait = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        disarm_handle.is_some() && wait == WAIT_OBJECT_0
    } else {
        false
    };

    for h in handles {
        unsafe {
            let _ = CloseHandle(h);
        }
    }

    if !disarmed_cleanly {
        restore_native_taskbar_and_workarea();
    }
}

/// Retorna o `HWND` verdadeiro da `Shell_TrayWnd` do `explorer.exe` (ignorando
/// o nosso proxy `Shell_TrayWnd` caso esteja ativo no processo).
pub fn find_explorer_primary_taskbar() -> Option<(HWND, u32)> {
    let own_pid = std::process::id();
    let mut prev: Option<HWND> = None;
    while let Ok(hwnd) = unsafe { FindWindowExW(None, prev, w!("Shell_TrayWnd"), None) } {
        let is_proxy = unsafe { !GetPropW(hwnd, PROXY_PROP).is_invalid() };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if !is_proxy && pid != 0 && pid != own_pid {
            return Some((hwnd, pid));
        }
        prev = Some(hwnd);
    }
    None
}

/// Lista todas as janelas da barra de tarefas do Explorer (primária + monitores secundários).
pub fn explorer_taskbar_windows() -> Vec<(HWND, String)> {
    let own_pid = std::process::id();
    let mut out = Vec::new();
    for (class_w, class_name) in [
        (w!("Shell_TrayWnd"), "Shell_TrayWnd"),
        (w!("Shell_SecondaryTrayWnd"), "Shell_SecondaryTrayWnd"),
    ] {
        let mut prev: Option<HWND> = None;
        while let Ok(hwnd) = unsafe { FindWindowExW(None, prev, class_w, None) } {
            let is_proxy = unsafe { !GetPropW(hwnd, PROXY_PROP).is_invalid() };
            let mut pid = 0u32;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            if !is_proxy && pid != own_pid {
                out.push((hwnd, class_name.to_owned()));
            }
            prev = Some(hwnd);
        }
    }
    out
}

fn enumerate_monitors() -> Vec<MonitorSnapshot> {
    unsafe extern "system" fn cb(hmon: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM) -> BOOL {
        let list = &mut *(lparam.0 as *mut Vec<MonitorSnapshot>);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut info).as_bool() {
            list.push(MonitorSnapshot {
                bounds: info.rcMonitor.into(),
                work: info.rcWork.into(),
            });
        }
        BOOL(1)
    }

    let mut list = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

fn read_window_region(hwnd: HWND) -> Option<SavedRect> {
    let mut rect = RECT::default();
    if unsafe { GetWindowRgnBox(hwnd, &mut rect) } == SIMPLEREGION {
        Some(rect.into())
    } else {
        None
    }
}

fn is_clipped(hwnd: HWND) -> bool {
    let mut rect = RECT::default();
    unsafe { GetWindowRgnBox(hwnd, &mut rect) == NULLREGION }
}

fn clip_window(hwnd: HWND) {
    if !is_clipped(hwnd) {
        unsafe {
            let hrgn = CreateRectRgn(0, 0, 0, 0);
            if !hrgn.is_invalid() && SetWindowRgn(hwnd, Some(hrgn), false) == 0 {
                let _ = DeleteObject(hrgn.into());
            }
        }
    }
}

fn unclip_window(hwnd: HWND, original: Option<SavedRect>) {
    if !is_clipped(hwnd) {
        return;
    }
    unsafe {
        match original {
            Some(r) => {
                let hrgn = CreateRectRgn(r.left, r.top, r.right, r.bottom);
                if !hrgn.is_invalid() {
                    if SetWindowRgn(hwnd, Some(hrgn), true) == 0 {
                        let _ = DeleteObject(hrgn.into());
                        let _ = SetWindowRgn(hwnd, None, true);
                    }
                } else {
                    let _ = SetWindowRgn(hwnd, None, true);
                }
            }
            None => {
                let _ = SetWindowRgn(hwnd, None, true);
            }
        }
    }
}

fn get_explorer_taskbar_state(primary: HWND) -> u32 {
    let mut abd = APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: primary,
        ..Default::default()
    };
    appbar::send_appbar_message(ABM_GETSTATE, &mut abd) as u32
}

fn set_explorer_taskbar_state(primary: HWND, state: u32) {
    let mut abd = APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: primary,
        lParam: LPARAM(state as isize),
        ..Default::default()
    };
    let _ = appbar::send_appbar_message(ABM_SETSTATE, &mut abd);
}

/// Captura o estado original da barra de tarefas e da WorkArea dos monitores,
/// gravando em `shell-recovery.json` apenas se ainda não houver um snapshot ativo
/// (evita sobrescrever o estado do usuário caso o app reinicie após um crash).
pub fn capture_and_save_snapshot() {
    let path = recovery_file();
    if path.exists() {
        return;
    }
    let bars_now = explorer_taskbar_windows();
    let Some((primary, _)) = find_explorer_primary_taskbar() else {
        return;
    };
    let original_taskbar_state = get_explorer_taskbar_state(primary);
    let monitors = enumerate_monitors();
    let bars = bars_now
        .into_iter()
        .filter_map(|(hwnd, class)| {
            let mut rect = RECT::default();
            unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
            Some(BarSnapshot {
                hwnd: hwnd.0 as usize,
                class,
                rect: rect.into(),
                region: read_window_region(hwnd),
            })
        })
        .collect();

    let snapshot = ShellRecoverySnapshot {
        original_taskbar_state,
        monitors,
        bars,
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
        let _ = fs::write(path, json);
    }
}

fn load_recovery_snapshot() -> Option<ShellRecoverySnapshot> {
    let content = fs::read_to_string(recovery_file()).ok()?;
    serde_json::from_str(&content).ok()
}

/// Se uma sessão anterior morreu antes de restaurar a barra de tarefas e o Pulse
/// subiu com o Pulse Shell desligado, devolve a taskbar e a WorkArea ao usuário.
pub fn recover_orphaned_session_if_needed() {
    if recovery_file().exists() {
        restore_native_taskbar_and_workarea();
    }
}

pub fn ensure_watchdog_running() -> bool {
    let Ok(mut guard) = watchdog_slot().lock() else {
        return false;
    };
    if let Some(handle) = guard.as_mut() {
        if matches!(handle.child.try_wait(), Ok(None)) {
            return true;
        }
        if let Some(ev) = handle.disarm_event.take() {
            unsafe {
                let _ = CloseHandle(ev);
            }
        }
    }

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let pid = std::process::id();
    let event_name = format!("Local\\Pulse.ShellWatchdog.{pid}");
    let disarm_event = unsafe { CreateEventW(None, true, false, &HSTRING::from(&event_name)).ok() };

    let mut cmd = Command::new(exe);
    cmd.args([DOCK_WATCHDOG_ARG, &pid.to_string()]);
    if disarm_event.is_some() {
        cmd.args([DISARM_EVENT_ARG, &event_name]);
    }
    cmd.creation_flags(CREATE_NO_WINDOW);

    match cmd.spawn() {
        Ok(child) => {
            *guard = Some(WatchdogHandle {
                child,
                disarm_event,
            });
            true
        }
        Err(_) => {
            if let Some(ev) = disarm_event {
                unsafe {
                    let _ = CloseHandle(ev);
                }
            }
            false
        }
    }
}

pub fn stop_watchdog() {
    let taken = watchdog_slot()
        .lock()
        .ok()
        .and_then(|mut guard| guard.take());
    let Some(mut handle) = taken else {
        return;
    };
    if let Some(ev) = handle.disarm_event {
        unsafe {
            let _ = SetEvent(ev);
        }
        let deadline = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < deadline {
            if matches!(handle.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        unsafe {
            let _ = CloseHandle(ev);
        }
    }
    if matches!(handle.child.try_wait(), Ok(None)) {
        let _ = handle.child.kill();
        let _ = handle.child.wait();
    }
}

pub fn is_watchdog_active() -> bool {
    let Ok(mut guard) = watchdog_slot().lock() else {
        return false;
    };
    if let Some(handle) = guard.as_mut() {
        matches!(handle.child.try_wait(), Ok(None))
    } else {
        false
    }
}

/// Oculta ou restaura a barra de tarefas nativa do Windows (primária e secundárias)
/// sem encerrar `explorer.exe`.
pub fn set_native_taskbar_hidden(hidden: bool) {
    if hidden {
        shell_log::info("watchdog", "Ocultando barra nativa do Windows");
        capture_and_save_snapshot();
        if let Some((primary, _)) = find_explorer_primary_taskbar() {
            let current = get_explorer_taskbar_state(primary);
            if current & ABS_AUTOHIDE == 0 {
                set_explorer_taskbar_state(primary, current | ABS_AUTOHIDE);
            }
        }
        let bars = explorer_taskbar_windows();
        for (hwnd, _) in &bars {
            unsafe {
                if IsWindowVisible(*hwnd).as_bool() {
                    let _ = ShowWindow(*hwnd, SW_HIDE);
                }
            }
            clip_window(*hwnd);
        }
        shell_log::info(
            "watchdog",
            format!("Barra nativa ocultada ({} janela(s))", bars.len()),
        );
    } else {
        restore_native_taskbar_and_workarea();
    }
}

/// Reaplica ocultação caso o Explorer tente reexibir `Shell_TrayWnd` em
/// notificações, atalho Win+T ou mudança de resolução.
pub fn enforce_native_taskbar_hidden() {
    for (hwnd, _) in explorer_taskbar_windows() {
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
        clip_window(hwnd);
    }
}

/// Restaura a barra nativa do Windows, o estado original (`ABM_SETSTATE`) e a
/// WorkArea de cada monitor para não deixar espaço morto após desligamento ou crash.
pub fn restore_native_taskbar_and_workarea() {
    shell_log::info("watchdog", "Restaurando barra nativa do Windows e WorkArea");
    let snapshot = load_recovery_snapshot();

    for (hwnd, class) in explorer_taskbar_windows() {
        let saved_region = snapshot.as_ref().and_then(|snap| {
            snap.bars
                .iter()
                .find(|b| b.hwnd == hwnd.0 as usize || b.class == class)
                .and_then(|b| b.region)
        });
        unclip_window(hwnd, saved_region);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
    }

    if let Some((primary, _)) = find_explorer_primary_taskbar() {
        let target_state = snapshot
            .as_ref()
            .map(|snap| snap.original_taskbar_state)
            .unwrap_or(0);
        set_explorer_taskbar_state(primary, target_state);
    }

    restore_workareas(snapshot.as_ref());

    let path = recovery_file();
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}

fn restore_workareas(snapshot: Option<&ShellRecoverySnapshot>) {
    let current = enumerate_monitors();
    if let Some(snap) = snapshot {
        for saved in &snap.monitors {
            if current
                .iter()
                .any(|m| m.bounds == saved.bounds && m.work != saved.work)
            {
                let mut rect: RECT = saved.work.into();
                unsafe {
                    let _ = SystemParametersInfoW(
                        SPI_SETWORKAREA,
                        0,
                        Some(&mut rect as *mut _ as *mut _),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    );
                }
            }
        }
    } else {
        // Sem snapshot salvo, devolve ao menos a área total dos monitores caso
        // uma AppBar órfã tenha deixado faixa presa no topo ou na base.
        for mon in &current {
            if mon.work != mon.bounds {
                let mut rect: RECT = mon.bounds.into();
                unsafe {
                    let _ = SystemParametersInfoW(
                        SPI_SETWORKAREA,
                        0,
                        Some(&mut rect as *mut _ as *mut _),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    );
                }
            }
        }
    }

    // Notifica janelas sem bloquear caso algum app esteja congelado.
    unsafe {
        let _ = SendNotifyMessageW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(SPIF_SENDCHANGE.0 as usize),
            LPARAM(0),
        );
    }
}
