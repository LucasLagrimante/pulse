//! Proxy user-space de `Shell_TrayWnd` para capturar ícones da bandeja do
//! sistema na Barra Superior do Pulse (sem injeção de DLL).
//!
//! Baseado nos padrões MIT do `wisp/src/tray_host/proxy.rs` e `ArcDockWin/src/tray.rs`:
//! - Cria janela `Shell_TrayWnd` marcada com `PROXY_PROP` acima da do Explorer
//! - Copia `HICON` sincronamente durante `WM_COPYDATA` com `CopyIcon`
//! - Chama `ReplyMessage(1)` quando o remetente está na mesma thread do Explorer
//!   para evitar deadlock antes de encaminhar ao `explorer.exe`
//! - Encaminha ao `explorer.exe` via `SendMessageTimeoutW(..., SMTO_ABORTIFHUNG)`
//! - Despacha cliques esquerdo/direito/duplo com `AllowSetForegroundWindow`

use super::{
    icons::{hicon_to_icon_image, IconImage},
    shell_log,
    watchdog::{find_explorer_primary_taskbar, PROXY_PROP},
};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    ffi::c_void,
    mem::size_of,
    sync::{
        atomic::{AtomicIsize, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
};
use windows::{
    core::{w, GUID, PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        System::{
            DataExchange::COPYDATASTRUCT,
            LibraryLoader::GetModuleHandleW,
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, ChangeWindowMessageFilterEx, CopyIcon, CreateWindowExW,
            DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowW,
            GetMessageW, GetWindowRect, GetWindowThreadProcessId, InSendMessageEx, IsWindow,
            KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
            RemovePropW, ReplyMessage, SendMessageTimeoutW, SetForegroundWindow, SetPropW,
            SetTimer, SetWindowPos, TranslateMessage, UnregisterClassW, HICON, HWND_BROADCAST,
            HWND_TOPMOST, ISMEX_NOTIFY, ISMEX_SEND, MSG, MSGFLT_ALLOW, SMTO_ABORTIFHUNG,
            SMTO_NORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
            WM_APP, WM_CLOSE, WM_COMMAND, WM_COPYDATA, WM_DESTROY, WM_LBUTTONDBLCLK,
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
            WM_SYSCOMMAND, WM_TIMER, WM_USER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

const TRAY_SIGNATURE: u32 = 0x3475_3423;
const TAG_NOTIFY: usize = 1;
const TAG_GET_RECT: usize = 3;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;

const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x01;

const NIN_SELECT: u32 = WM_USER;
const WM_CONTEXTMENU: u32 = 0x007B;
const SC_TASKLIST: usize = 0xF130;
const NOTIFYICON_VERSION_4: u32 = 4;

const TIMER_CHECK: usize = 1;
const CHECK_MS: u32 = 1000;
const TIMER_REBROADCAST: usize = 2;
const INITIAL_BROADCAST_MS: u32 = 300;
const FORWARD_TIMEOUT_MS: u32 = 2500;
const OTHER_TIMEOUT_MS: u32 = 1500;

const WM_APP_TRAY_STOP: u32 = WM_APP + 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayClickButton {
    Left,
    Right,
    Double,
}

#[derive(Clone)]
pub struct TrayIconView {
    pub id: String,
    pub tip: String,
    pub image: Option<IconImage>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NotifyIconData32 {
    cb_size: u32,
    hwnd: u32,
    uid: u32,
    flags: u32,
    callback: u32,
    hicon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: GUID,
    balloon_icon: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ShellTrayData {
    signature: u32,
    message: u32,
    nid: NotifyIconData32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IconIdentifier {
    signature: u32,
    message: u32,
    cb_size: u32,
    padding: u32,
    hwnd: u32,
    uid: u32,
    guid: GUID,
}

#[derive(Clone)]
struct TrayEntry {
    hwnd: u32,
    uid: u32,
    pid: u32,
    is_explorer: bool,
    exe_key: String,
    guid: Option<GUID>,
    callback: u32,
    version: u32,
    hidden: bool,
    tip: String,
    image: Option<IconImage>,
}

/// O SndVolSSO do Explorer é o ícone de volume nativo. Nossa Barra Superior
/// já desenha um widget de volume próprio com o mesmo painel, então exibir os
/// dois duplica o controle. A identificação exige `is_explorer` **e** uma tip
/// de volume, para não ocultar rede, Segurança nem ícones de terceiros.
fn is_shell_audio_tip(tip: &str) -> bool {
    let lower = tip.to_ascii_lowercase();
    lower.contains('%')
        || lower.contains("headset")
        || lower.contains("speaker")
        || lower.contains("alto-falante")
        || lower.contains("volume")
        || lower.contains("audio")
        || lower.contains("áudio")
        || lower.contains("mudo")
        || lower.contains("muted")
}

impl TrayEntry {
    /// Ícone nativo de volume do Windows, já substituído pelo widget do Pulse.
    fn is_suppressed(&self) -> bool {
        self.is_explorer && is_shell_audio_tip(&self.tip)
    }

    fn stable_id(&self) -> String {
        match self.guid {
            Some(g) => format!("{g:?}"),
            None => format!("{:x}:{}", self.hwnd, self.uid),
        }
    }

    /// Chave persistente entre reinícios (GUID ou executável + uid).
    fn persist_key(&self) -> String {
        if let Some(g) = self.guid {
            format!("guid:{g:?}")
        } else if !self.exe_key.is_empty() {
            format!("exe:{}:{}", self.exe_key, self.uid)
        } else {
            self.stable_id()
        }
    }
}

fn tray_order_file() -> std::path::PathBuf {
    crate::storage::paths::pulse_local_dir().join("tray-icon-order.json")
}

fn load_persisted_tray_order() -> Vec<String> {
    std::fs::read_to_string(tray_order_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

fn save_persisted_tray_order(order: &[String]) {
    let file = tray_order_file();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(order) {
        let _ = std::fs::write(file, json);
    }
}

struct SharedTrayData {
    entries: Mutex<Vec<TrayEntry>>,
    custom_order: Mutex<Vec<String>>,
    icon_rects: Mutex<Vec<(String, RECT)>>,
    topbar_rect: Mutex<Option<RECT>>,
    topbar_hwnd: AtomicIsize,
    notify_msg: u32,
}

type ProxyThreadSlot = Mutex<Option<(JoinHandle<()>, isize)>>;

static SHARED: OnceLock<Arc<SharedTrayData>> = OnceLock::new();
static PROXY_THREAD: OnceLock<ProxyThreadSlot> = OnceLock::new();

fn shared_state() -> &'static Arc<SharedTrayData> {
    SHARED.get_or_init(|| {
        Arc::new(SharedTrayData {
            entries: Mutex::new(Vec::new()),
            custom_order: Mutex::new(load_persisted_tray_order()),
            icon_rects: Mutex::new(Vec::new()),
            topbar_rect: Mutex::new(None),
            topbar_hwnd: AtomicIsize::new(0),
            notify_msg: WM_APP + 61,
        })
    })
}

fn proxy_slot() -> &'static ProxyThreadSlot {
    PROXY_THREAD.get_or_init(|| Mutex::new(None))
}

/// Atualiza o `HWND` da Barra Superior que receberá notificações de atualização
/// e o retângulo dela na tela (para posicionar menus de apps como G HUB).
pub fn bind_topbar(hwnd: HWND, rect: RECT, notify_msg: u32) {
    let shared = shared_state();
    shared.topbar_hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
    if let Ok(mut r) = shared.topbar_rect.lock() {
        *r = Some(rect);
    }
    let _ = notify_msg;
}

pub fn set_topbar_notify_msg(notify_msg: u32) {
    // Armazenado no atomic ou usado via constante fixa; mantemos compatibilidade
    // usando `WM_APP_TRAY_CHANGED` = WM_APP + 61.
    let _ = notify_msg;
}

pub fn update_icon_rects(rects: Vec<(String, RECT)>) {
    if let Ok(mut guard) = shared_state().icon_rects.lock() {
        *guard = rects;
    }
}

/// Move um ícone da bandeja para a posição de outro (drag-and-drop na Barra Superior)
/// e persiste a ordem em `tray-icon-order.json`.
pub fn reorder_icon(from_id: &str, to_id: &str) -> bool {
    if from_id == to_id {
        return false;
    }
    let ordered_pairs: Vec<(String, String)> = {
        let Ok(guard) = shared_state().entries.lock() else {
            return false;
        };
        let mut items: Vec<(String, String)> = guard
            .iter()
            .filter(|e| !e.hidden && !e.is_suppressed() && e.image.is_some())
            .map(|e| (e.stable_id(), e.persist_key()))
            .collect();
        if let Ok(order) = shared_state().custom_order.lock() {
            if !order.is_empty() {
                items.sort_by_key(|(_, pk)| {
                    order.iter().position(|k| k == pk).unwrap_or(usize::MAX)
                });
            }
        }
        items
    };

    let Some(from_idx) = ordered_pairs.iter().position(|(id, _)| id == from_id) else {
        return false;
    };
    let Some(to_idx) = ordered_pairs.iter().position(|(id, _)| id == to_id) else {
        return false;
    };
    let mut keys: Vec<String> = ordered_pairs.into_iter().map(|(_, pk)| pk).collect();
    let moved = keys.remove(from_idx);
    keys.insert(to_idx, moved);

    if let Ok(mut order) = shared_state().custom_order.lock() {
        *order = keys.clone();
        save_persisted_tray_order(&keys);
        true
    } else {
        false
    }
}

pub fn visible_icons() -> Vec<TrayIconView> {
    let Ok(guard) = shared_state().entries.lock() else {
        return Vec::new();
    };
    let mut items: Vec<(String, TrayIconView)> = guard
        .iter()
        .filter(|e| !e.hidden && !e.is_suppressed() && e.image.is_some())
        .map(|e| {
            (
                e.persist_key(),
                TrayIconView {
                    id: e.stable_id(),
                    tip: e.tip.clone(),
                    image: e.image.clone(),
                },
            )
        })
        .collect();
    if let Ok(order) = shared_state().custom_order.lock() {
        if !order.is_empty() {
            items.sort_by_key(|(pk, _)| order.iter().position(|k| k == pk).unwrap_or(usize::MAX));
        }
    }
    items.into_iter().map(|(_, v)| v).collect()
}

pub fn start() {
    let mut guard = proxy_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((ref handle, _)) = *guard {
        if !handle.is_finished() {
            return;
        }
    }
    if let Some((old, _)) = guard.take() {
        let _ = old.join();
    }

    let (tx, rx) = mpsc::channel();
    if let Ok(handle) = thread::Builder::new()
        .name("pulse-tray-proxy".to_owned())
        .spawn(move || run_proxy_thread(tx))
    {
        if let Ok(control_hwnd) = rx.recv() {
            *guard = Some((handle, control_hwnd));
        } else {
            let _ = handle.join();
        }
    }
}

pub fn stop() {
    let taken = proxy_slot().lock().ok().and_then(|mut guard| guard.take());
    if let Some((handle, control_raw)) = taken {
        if control_raw != 0 {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(control_raw as *mut _)),
                    WM_APP_TRAY_STOP,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        let _ = handle.join();
    }
    if let Ok(mut entries) = shared_state().entries.lock() {
        entries.clear();
    }
    if let Ok(mut rects) = shared_state().icon_rects.lock() {
        rects.clear();
    }
}

pub fn click_icon_by_id(icon_id: &str, button: TrayClickButton, anchor: POINT) {
    let target = {
        let Ok(guard) = shared_state().entries.lock() else {
            return;
        };
        guard
            .iter()
            .find(|e| {
                !e.hidden && !e.is_suppressed() && e.image.is_some() && e.stable_id() == icon_id
            })
            .map(|e| {
                (
                    e.hwnd,
                    e.uid,
                    e.pid,
                    e.is_explorer,
                    e.exe_key.clone(),
                    e.callback,
                    e.version,
                    e.tip.clone(),
                )
            })
    };
    let Some((raw_hwnd, uid, saved_pid, is_explorer, exe_key, callback, version, tip)) = target
    else {
        return;
    };
    let hwnd = hwnd_from_u32(raw_hwnd);
    let mut pid = 0u32;
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || GetWindowThreadProcessId(hwnd, Some(&mut pid)) == 0 {
            return;
        }
        if saved_pid != 0 && pid != saved_pid {
            return;
        }
    }

    if callback == 0 {
        let tip_lower = tip.to_ascii_lowercase();
        // Fallback direto para Windows Security (SecurityHealthSystray.exe), que
        // frequentemente mantém callback == 0 quando o proxy assume após o logon.
        if exe_key.ends_with("\\securityhealthsystray.exe")
            || tip_lower.contains("windows security")
            || tip_lower.contains("segurança do windows")
        {
            let _ = tauri_plugin_opener::open_url("windowsdefender:", None::<&str>);
            shell_log::info(
                "tray_click",
                format!("Aberto Windows Security via windowsdefender: ('{tip}')"),
            );
            return;
        }

        // Ícones internos do Explorer (SndVolSSO / Rede) não possuem callback externo:
        // redireciona volume para o Painel de Áudio do Pulse e rede para Configurações.
        if is_explorer {
            if is_shell_audio_tip(&tip) {
                let bar_rect = shared_state()
                    .topbar_rect
                    .lock()
                    .ok()
                    .and_then(|r| *r)
                    .unwrap_or(RECT {
                        left: 0,
                        top: 0,
                        right: 1920,
                        bottom: 32,
                    });
                let monitor = RECT {
                    left: bar_rect.left,
                    top: bar_rect.top,
                    right: bar_rect.right,
                    bottom: bar_rect.top + 1080,
                };
                super::toggle_shell_popup_at("audio", anchor.x, bar_rect.bottom, monitor, 1.0);
                return;
            }
            if tip_lower.contains("internet")
                || tip_lower.contains("network")
                || tip_lower.contains("rede")
                || tip_lower.contains("wi-fi")
                || tip_lower.contains("ethernet")
            {
                let _ = tauri_plugin_opener::open_url("ms-settings:network-status", None::<&str>);
                return;
            }
            shell_log::info(
                "tray_click",
                format!("Ícone do Explorer sem callback '{tip}' (hwnd={raw_hwnd:#x})"),
            );
            return;
        }

        // Ícone chegou por NIM_MODIFY sem NIF_MESSAGE: pede re-registro a todas as
        // janelas top-level do processo dono (muitos apps ouvem TaskbarCreated na main window).
        if pid != 0 {
            let msg = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
            if msg != 0 {
                post_taskbar_created_to_pid(pid, msg, hwnd);
            }
            shell_log::info(
                "tray_click",
                format!(
                    "Callback ausente para '{tip}' (exe={exe_key} hwnd={raw_hwnd:#x} pid={pid}); solicitado TaskbarCreated"
                ),
            );
        }
        return;
    }

    unsafe {
        if pid != 0 {
            let _ = AllowSetForegroundWindow(pid);
        }
        let _ = SetForegroundWindow(hwnd);
    }

    // Formato v4 (NOTIFYICON_VERSION_4) usa NIN_SELECT/WM_CONTEXTMENU + (x,y)/(msg,uid);
    // versões clássicas (v0..v3) usam apenas eventos de mouse + (uid, msg), evitando
    // dupla ativação em apps Qt (como Flameshot) quando version < 4.
    let msgs: &[u32] = if version >= NOTIFYICON_VERSION_4 {
        match button {
            TrayClickButton::Left => &[WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
            TrayClickButton::Right => &[WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
            TrayClickButton::Double => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
        }
    } else {
        match button {
            TrayClickButton::Left => &[WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP],
            TrayClickButton::Right => &[WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP],
            TrayClickButton::Double => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
        }
    };

    let mut failed = false;
    for &m in msgs {
        let (wp, lp) = if version >= NOTIFYICON_VERSION_4 {
            let wp = (anchor.x as u16 as usize) | ((anchor.y as u16 as usize) << 16);
            let lp = (m as u16 as u32 | ((uid as u16 as u32) << 16)) as i32 as isize;
            (wp, lp)
        } else {
            (uid as usize, m as isize)
        };
        unsafe {
            if PostMessageW(Some(hwnd), callback, WPARAM(wp), LPARAM(lp)).is_err() {
                failed = true;
            }
        }
    }
    if failed {
        shell_log::info(
            "tray_click",
            format!(
                "PostMessageW falhou para '{tip}' (exe={exe_key} hwnd={raw_hwnd:#x} cb={callback:#x} ver={version} uid={uid})"
            ),
        );
    }
}

/// Envia `TaskbarCreated` para o `hwnd` do ícone e para todas as janelas top-level
/// pertencentes ao mesmo `pid`, pois apps como Qt/Win32 frequentemente escutam
/// `TaskbarCreated` na janela principal e não na janela oculta de notificação.
fn post_taskbar_created_to_pid(target_pid: u32, msg: u32, primary_hwnd: HWND) {
    struct Ctx {
        pid: u32,
        msg: u32,
        primary: isize,
    }
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let ctx = &*(lparam.0 as *const Ctx);
        let mut wpid = 0u32;
        if GetWindowThreadProcessId(hwnd, Some(&mut wpid)) != 0
            && wpid == ctx.pid
            && hwnd.0 as isize != ctx.primary
        {
            let _ = PostMessageW(Some(hwnd), ctx.msg, WPARAM(0), LPARAM(0));
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = PostMessageW(Some(primary_hwnd), msg, WPARAM(0), LPARAM(0));
        let mut ctx = Ctx {
            pid: target_pid,
            msg,
            primary: primary_hwnd.0 as isize,
        };
        let _ = EnumWindows(Some(enum_proc), LPARAM((&mut ctx as *mut Ctx) as isize));
    }
}

thread_local! {
    static PROXY_HWND: Cell<isize> = const { Cell::new(0) };
    static TASKBAR_CREATED_MSG: Cell<u32> = const { Cell::new(0) };
    static BROADCASTING: Cell<bool> = const { Cell::new(false) };
    static EXPLORER_HWND: Cell<isize> = const { Cell::new(0) };
    static PURGE_TICKS: Cell<u32> = const { Cell::new(0) };
    static REBROADCAST_PENDING: RefCell<bool> = const { RefCell::new(false) };
    static ASKED_CALLBACKS: RefCell<HashSet<(u32, u32)>> = RefCell::new(HashSet::new());
}

fn process_image_path_lower(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let proc_h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            proc_h,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(proc_h);
        if !ok || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase())
    }
}

fn process_exe_lower(pid: u32) -> String {
    process_image_path_lower(pid).unwrap_or_default()
}

/// O `msrdc.exe` interno do WSLg registra ícones de Remote Desktop na bandeja
/// que o Windows 11 esconde no overflow (`^`). O menu deles ("Open Remote Desktop")
/// inclusive falha buscando `C:\Program Files\WSL\msrdcw.exe`, e assusta o usuário
/// na Barra Superior. Filtramos apenas `msrdc.exe` dentro da árvore do WSL.
fn is_wslg_msrdc_path(lower_path: &str) -> bool {
    lower_path.ends_with("\\msrdc.exe")
        && (lower_path.contains("\\wsl\\") || lower_path.contains("windowssubsystemforlinux"))
}

fn is_wslg_msrdc_process(pid: u32) -> bool {
    process_image_path_lower(pid).is_some_and(|p| is_wslg_msrdc_path(&p))
}

fn ask_missing_callbacks() {
    let msg = TASKBAR_CREATED_MSG.get();
    if msg == 0 {
        return;
    }
    let missing: Vec<(u32, u32)> = {
        let Ok(guard) = shared_state().entries.lock() else {
            return;
        };
        ASKED_CALLBACKS.with(|asked| {
            let mut asked = asked.borrow_mut();
            guard
                .iter()
                .filter(|e| e.callback == 0 && e.hwnd != 0 && e.pid != 0 && !e.is_explorer)
                .filter(|e| asked.insert((e.pid, e.hwnd)))
                .map(|e| (e.hwnd, e.pid))
                .collect()
        })
    };
    for (raw_hwnd, saved_pid) in missing {
        let target = hwnd_from_u32(raw_hwnd);
        let mut cur_pid = 0u32;
        unsafe {
            if IsWindow(Some(target)).as_bool()
                && GetWindowThreadProcessId(target, Some(&mut cur_pid)) != 0
                && cur_pid == saved_pid
            {
                post_taskbar_created_to_pid(saved_pid, msg, target);
            }
        }
    }
}

fn hwnd_from_u32(h: u32) -> HWND {
    HWND(h as i32 as isize as *mut c_void)
}

fn explorer_tray() -> Option<HWND> {
    let cached = HWND(EXPLORER_HWND.get() as *mut c_void);
    if !cached.is_invalid() && unsafe { IsWindow(Some(cached)).as_bool() } {
        return Some(cached);
    }
    let (h, _) = find_explorer_primary_taskbar()?;
    EXPLORER_HWND.set(h.0 as isize);
    Some(h)
}

fn run_proxy_thread(ready: mpsc::Sender<isize>) {
    let Ok(hinst) = (unsafe { GetModuleHandleW(None) }) else {
        let _ = ready.send(0);
        return;
    };

    let control_class = w!("PulseTrayControlClass");
    let tray_class = w!("Shell_TrayWnd");

    unsafe {
        let wc_ctrl = WNDCLASSW {
            lpfnWndProc: Some(control_wndproc),
            hInstance: hinst.into(),
            lpszClassName: control_class,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc_ctrl);

        let wc_tray = WNDCLASSW {
            lpfnWndProc: Some(proxy_wndproc),
            hInstance: hinst.into(),
            lpszClassName: tray_class,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc_tray);
    }

    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED_MSG.set(taskbar_created);

    let Ok(control) = (unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            control_class,
            w!("Pulse Tray Control"),
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
    }) else {
        let _ = ready.send(0);
        return;
    };

    let initial_rect = shared_state()
        .topbar_rect
        .lock()
        .ok()
        .and_then(|r| *r)
        .unwrap_or(RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 32,
        });

    let Ok(proxy) = (unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            tray_class,
            PCWSTR::null(),
            WS_POPUP,
            initial_rect.left,
            initial_rect.top,
            (initial_rect.right - initial_rect.left).max(1),
            (initial_rect.bottom - initial_rect.top).max(1),
            None,
            None,
            Some(hinst.into()),
            None,
        )
    }) else {
        unsafe {
            let _ = DestroyWindow(control);
        }
        let _ = ready.send(0);
        return;
    };

    let exp_init = explorer_tray();
    PROXY_HWND.set(proxy.0 as isize);
    unsafe {
        let _ = SetPropW(proxy, PROXY_PROP, Some(HANDLE_ONE));
        let _ = ChangeWindowMessageFilterEx(proxy, WM_COPYDATA, MSGFLT_ALLOW, None);
        if taskbar_created != 0 {
            let _ = ChangeWindowMessageFilterEx(control, taskbar_created, MSGFLT_ALLOW, None);
        }
        make_topmost(proxy);
        let _ = SetTimer(Some(control), TIMER_CHECK, CHECK_MS, None);
        // Dispara o primeiro TaskbarCreated assíncrono já dentro do loop GetMessageW.
        let _ = SetTimer(Some(control), TIMER_REBROADCAST, INITIAL_BROADCAST_MS, None);
    }
    REBROADCAST_PENDING.with(|p| *p.borrow_mut() = true);
    shell_log::info(
        "tray_proxy",
        format!(
            "Proxy Shell_TrayWnd pronto proxy={:?} explorer={:?}",
            proxy.0,
            exp_init.map(|h| h.0)
        ),
    );

    let _ = ready.send(control.0 as isize);

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = KillTimer(Some(control), TIMER_CHECK);
        let _ = KillTimer(Some(control), TIMER_REBROADCAST);
        let _ = RemovePropW(proxy, PROXY_PROP);
        let _ = DestroyWindow(proxy);
        let _ = DestroyWindow(control);
        let _ = UnregisterClassW(tray_class, Some(hinst.into()));
        let _ = UnregisterClassW(control_class, Some(hinst.into()));
    }
    PROXY_HWND.set(0);
    EXPLORER_HWND.set(0);
    // Devolve ícones ao Explorer verdadeiro de forma não-bloqueante.
    broadcast_taskbar_created();
    shell_log::info("tray_proxy", "Proxy Shell_TrayWnd encerrado");
}

const HANDLE_ONE: windows::Win32::Foundation::HANDLE =
    windows::Win32::Foundation::HANDLE(std::ptr::dangling_mut::<c_void>());

fn broadcast_taskbar_created() {
    let msg = TASKBAR_CREATED_MSG.get();
    if msg == 0 {
        return;
    }
    BROADCASTING.set(true);
    unsafe {
        let _ = PostMessageW(Some(HWND_BROADCAST), msg, WPARAM(0), LPARAM(0));
    }
    BROADCASTING.set(false);
}

fn make_topmost(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

fn sync_proxy_rect(proxy: HWND) {
    let want = shared_state().topbar_rect.lock().ok().and_then(|r| *r);
    let Some(want) = want else {
        return;
    };
    unsafe {
        let mut now = RECT::default();
        if GetWindowRect(proxy, &mut now).is_ok() && now == want {
            return;
        }
        let _ = SetWindowPos(
            proxy,
            None,
            want.left,
            want.top,
            (want.right - want.left).max(1),
            (want.bottom - want.top).max(1),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn forward_copy_to_explorer(wp: WPARAM, tag: usize, bytes: &[u8]) -> Option<LRESULT> {
    let target = explorer_tray()?;
    let copy = COPYDATASTRUCT {
        dwData: tag,
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr().cast_mut().cast(),
    };
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            target,
            WM_COPYDATA,
            wp,
            LPARAM(&copy as *const _ as isize),
            SMTO_NORMAL | SMTO_ABORTIFHUNG,
            FORWARD_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    (ok.0 != 0).then_some(LRESULT(result as isize))
}

fn forward_other_to_explorer(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let target = explorer_tray()?;
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            target,
            msg,
            wp,
            lp,
            SMTO_NORMAL | SMTO_ABORTIFHUNG,
            OTHER_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    (ok.0 != 0).then_some(LRESULT(result as isize))
}

fn notify_topbar() {
    let shared = shared_state();
    let raw = shared.topbar_hwnd.load(Ordering::Relaxed);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(raw as *mut _)),
                shared.notify_msg,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

unsafe extern "system" fn control_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let taskbar_created = TASKBAR_CREATED_MSG.get();
    match msg {
        WM_TIMER if wp.0 == TIMER_CHECK => {
            let proxy = HWND(PROXY_HWND.get() as *mut _);
            if !proxy.is_invalid() {
                if FindWindowW(w!("Shell_TrayWnd"), None).ok() != Some(proxy) {
                    make_topmost(proxy);
                }
                sync_proxy_rect(proxy);
            }
            let ticks = PURGE_TICKS.get().wrapping_add(1);
            PURGE_TICKS.set(ticks);
            // Após 3s do broadcast inicial, solicita TaskbarCreated individualmente
            // para ícones que chegaram via NIM_MODIFY sem NIF_MESSAGE (callback == 0).
            if ticks >= 3 {
                ask_missing_callbacks();
            }
            if ticks.is_multiple_of(5) {
                let mut changed = false;
                if let Ok(mut guard) = shared_state().entries.lock() {
                    let before = guard.len();
                    guard.retain(|e| IsWindow(Some(hwnd_from_u32(e.hwnd))).as_bool());
                    changed = guard.len() != before;
                }
                if changed {
                    notify_topbar();
                }
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_REBROADCAST => {
            let _ = KillTimer(Some(hwnd), TIMER_REBROADCAST);
            let proxy = HWND(PROXY_HWND.get() as *mut _);
            if !proxy.is_invalid() {
                make_topmost(proxy);
            }
            REBROADCAST_PENDING.with(|p| *p.borrow_mut() = false);
            broadcast_taskbar_created();
            LRESULT(0)
        }
        WM_APP_TRAY_STOP | WM_CLOSE => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if taskbar_created != 0 && m == taskbar_created => {
            // Só re-anexa se o HWND do Explorer realmente mudou (reinício do explorer.exe).
            let current_exp = find_explorer_primary_taskbar().map(|(h, _)| h.0 as isize);
            if !BROADCASTING.get()
                && current_exp.is_some()
                && current_exp != Some(EXPLORER_HWND.get())
            {
                EXPLORER_HWND.set(current_exp.unwrap_or(0));
                let _ = SetTimer(Some(hwnd), TIMER_REBROADCAST, 1400, None);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn proxy_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let Some(cds) = (lp.0 as *const COPYDATASTRUCT).as_ref() else {
                return LRESULT(0);
            };
            // Copia os bytes imediatamente enquanto o remetente bloqueia em SendMessage.
            let bytes = if cds.lpData.is_null() || cds.cbData == 0 || cds.cbData > (1 << 20) {
                Vec::new()
            } else {
                std::slice::from_raw_parts(cds.lpData.cast::<u8>(), cds.cbData as usize).to_vec()
            };
            let tag = cds.dwData;

            if tag == TAG_GET_RECT {
                if let Some(reply) = handle_get_rect(cds) {
                    return LRESULT(reply);
                }
            }

            if tag == TAG_NOTIFY {
                let parsed = parse_tray_copydata(cds);
                let own_res = if let Some((op, nid, img)) = parsed {
                    if let Some(exp) = explorer_tray() {
                        let sender_tid = GetWindowThreadProcessId(hwnd_from_u32(nid.hwnd), None);
                        let exp_tid = GetWindowThreadProcessId(exp, None);
                        if sender_tid != 0 && sender_tid == exp_tid {
                            let _ = ReplyMessage(LRESULT(1));
                        }
                    }
                    let res = apply_notify(op, &nid, img);
                    Some(res)
                } else {
                    None
                };
                let fwd = forward_copy_to_explorer(wp, tag, &bytes);
                if let Some((ok, changed)) = own_res {
                    if changed {
                        notify_topbar();
                    }
                    return fwd.unwrap_or(LRESULT(isize::from(ok)));
                }
                return fwd.unwrap_or(LRESULT(0));
            }

            // Se o próprio Explorer enviou um WM_COPYDATA síncrono para FindWindow("Shell_TrayWnd"),
            // libera a thread do Explorer antes de encaminhar para evitar auto-deadlock.
            if let Some(exp) = explorer_tray() {
                let sender_hwnd = HWND(wp.0 as *mut c_void);
                if !sender_hwnd.is_invalid() {
                    let sender_tid = GetWindowThreadProcessId(sender_hwnd, None);
                    let exp_tid = GetWindowThreadProcessId(exp, None);
                    if sender_tid != 0 && sender_tid == exp_tid {
                        let _ = ReplyMessage(LRESULT(0));
                    }
                }
            }
            forward_copy_to_explorer(wp, tag, &bytes).unwrap_or(LRESULT(0))
        }
        WM_COMMAND => {
            if (InSendMessageEx(None) & (ISMEX_SEND | ISMEX_NOTIFY)) == ISMEX_SEND {
                forward_other_to_explorer(msg, wp, lp)
                    .unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
            } else {
                if let Some(exp) = explorer_tray() {
                    let _ = PostMessageW(Some(exp), msg, wp, lp);
                }
                LRESULT(0)
            }
        }
        m if (WM_USER..0xC000).contains(&m) => {
            if m == WM_USER + 372 {
                if let Some(exp) = explorer_tray() {
                    let _ = PostMessageW(Some(exp), msg, wp, lp);
                }
                LRESULT(0)
            } else if (InSendMessageEx(None) & (ISMEX_SEND | ISMEX_NOTIFY)) == ISMEX_SEND {
                forward_other_to_explorer(msg, wp, lp).unwrap_or(LRESULT(0))
            } else {
                if let Some(exp) = explorer_tray() {
                    let _ = PostMessageW(Some(exp), msg, wp, lp);
                }
                LRESULT(0)
            }
        }
        m if m == WM_SYSCOMMAND && (wp.0 & 0xFFF0) == SC_TASKLIST => {
            if (InSendMessageEx(None) & (ISMEX_SEND | ISMEX_NOTIFY)) == ISMEX_SEND {
                forward_other_to_explorer(msg, wp, lp)
                    .unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
            } else {
                if let Some(exp) = explorer_tray() {
                    let _ = PostMessageW(Some(exp), msg, wp, lp);
                }
                LRESULT(0)
            }
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn parse_tray_copydata(
    cds: &COPYDATASTRUCT,
) -> Option<(u32, NotifyIconData32, Option<Option<IconImage>>)> {
    if cds.lpData.is_null() || (cds.cbData as usize) < 8 + 24 {
        return None;
    }
    let mut data: ShellTrayData = std::mem::zeroed();
    let n = (cds.cbData as usize).min(size_of::<ShellTrayData>());
    std::ptr::copy_nonoverlapping(cds.lpData.cast::<u8>(), (&raw mut data).cast::<u8>(), n);
    if data.signature != TRAY_SIGNATURE {
        return None;
    }

    let nid = data.nid;
    // Ignora o próprio ícone de bandeja do Pulse e o msrdc.exe interno do WSLg na Top Bar.
    let mut sender_pid = 0u32;
    GetWindowThreadProcessId(hwnd_from_u32(nid.hwnd), Some(&mut sender_pid));
    if sender_pid == std::process::id() || is_wslg_msrdc_process(sender_pid) {
        return None;
    }

    let image = if matches!(data.message, NIM_ADD | NIM_MODIFY) && (nid.flags & NIF_ICON) != 0 {
        if nid.hicon == 0 {
            Some(None)
        } else {
            let raw_icon = HICON(nid.hicon as i32 as isize as *mut c_void);
            match CopyIcon(raw_icon) {
                Ok(copied) => {
                    let img = hicon_to_icon_image(copied);
                    let _ = DestroyIcon(copied);
                    Some(img)
                }
                Err(_) => Some(hicon_to_icon_image(raw_icon)),
            }
        }
    } else {
        None
    };

    Some((data.message, nid, image))
}

fn apply_notify(op: u32, nid: &NotifyIconData32, image: Option<Option<IconImage>>) -> (bool, bool) {
    let Ok(mut entries) = shared_state().entries.lock() else {
        return (false, false);
    };
    let has_guid = (nid.flags & NIF_GUID) != 0 && nid.guid != GUID::zeroed();
    let pos = if has_guid {
        entries.iter().position(|e| e.guid == Some(nid.guid))
    } else {
        entries
            .iter()
            .position(|e| e.hwnd == nid.hwnd && e.uid == nid.uid)
    };

    let mut sender_pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd_from_u32(nid.hwnd), Some(&mut sender_pid));
    }
    let sender_exe = process_exe_lower(sender_pid);
    let sender_is_explorer = sender_exe.ends_with("\\explorer.exe");

    match op {
        NIM_ADD | NIM_MODIFY => {
            let idx = match pos {
                Some(i) => i,
                None if op == NIM_MODIFY => {
                    // Ícone já existia antes do proxy e enviou MODIFY: registra-o agora.
                    entries.push(TrayEntry {
                        hwnd: nid.hwnd,
                        uid: nid.uid,
                        pid: sender_pid,
                        is_explorer: sender_is_explorer,
                        exe_key: sender_exe.clone(),
                        guid: has_guid.then_some(nid.guid),
                        callback: 0,
                        version: 0,
                        hidden: false,
                        tip: String::new(),
                        image: None,
                    });
                    entries.len() - 1
                }
                None => {
                    entries.push(TrayEntry {
                        hwnd: nid.hwnd,
                        uid: nid.uid,
                        pid: sender_pid,
                        is_explorer: sender_is_explorer,
                        exe_key: sender_exe.clone(),
                        guid: has_guid.then_some(nid.guid),
                        callback: 0,
                        version: 0,
                        hidden: false,
                        tip: String::new(),
                        image: None,
                    });
                    entries.len() - 1
                }
            };
            let entry = &mut entries[idx];
            entry.hwnd = nid.hwnd;
            entry.uid = nid.uid;
            if sender_pid != 0 {
                entry.pid = sender_pid;
                entry.is_explorer = sender_is_explorer;
                if !sender_exe.is_empty() {
                    entry.exe_key = sender_exe;
                }
            }
            if has_guid {
                entry.guid = Some(nid.guid);
            }
            if (nid.flags & NIF_MESSAGE) != 0 {
                entry.callback = nid.callback;
            }
            if let Some(img) = image {
                entry.image = img;
            }
            if (nid.flags & NIF_TIP) != 0 {
                let len = nid
                    .tip
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(nid.tip.len());
                entry.tip = String::from_utf16_lossy(&nid.tip[..len]).trim().to_owned();
            }
            if (nid.flags & NIF_STATE) != 0 && (nid.state_mask & NIS_HIDDEN) != 0 {
                entry.hidden = (nid.state & NIS_HIDDEN) != 0;
            }
            (true, true)
        }
        NIM_DELETE => match pos {
            Some(i) => {
                let removed = entries.remove(i);
                (true, !removed.hidden)
            }
            None => (false, false),
        },
        NIM_SETVERSION => match pos {
            Some(i) => {
                entries[i].version = nid.version;
                (true, false)
            }
            None => (false, false),
        },
        NIM_SETFOCUS => (true, false),
        _ => (false, false),
    }
}

unsafe fn handle_get_rect(cds: &COPYDATASTRUCT) -> Option<isize> {
    if cds.lpData.is_null() || (cds.cbData as usize) < size_of::<IconIdentifier>() {
        return None;
    }
    let id = std::ptr::read_unaligned(cds.lpData.cast::<IconIdentifier>());
    if id.signature != TRAY_SIGNATURE {
        return None;
    }
    let by_guid = id.guid != GUID::zeroed();

    let target_id = {
        let entries = shared_state().entries.lock().ok()?;
        entries
            .iter()
            .find(|e| {
                !e.hidden
                    && !e.is_suppressed()
                    && e.image.is_some()
                    && if by_guid {
                        e.guid == Some(id.guid)
                    } else {
                        e.hwnd == id.hwnd && e.uid == id.uid
                    }
            })
            .map(TrayEntry::stable_id)?
    };

    let rect = shared_state().icon_rects.lock().ok().and_then(|rects| {
        rects
            .iter()
            .find(|(id_str, _)| id_str == &target_id)
            .map(|(_, r)| *r)
    })?;

    let (x, y) = match id.message {
        1 => (rect.left, rect.top),
        2 => (rect.right - rect.left, rect.bottom - rect.top),
        _ => return None,
    };
    Some(((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as isize)
}

#[cfg(test)]
mod tests {
    use super::is_wslg_msrdc_path;

    #[test]
    fn filters_only_wslg_internal_msrdc() {
        assert!(is_wslg_msrdc_path(r"c:\program files\wsl\msrdc.exe"));
        assert!(is_wslg_msrdc_path(
            r"c:\program files\windowsapps\microsoftcorporationii.windowssubsystemforlinux_2.4.0\msrdc.exe"
        ));
        assert!(!is_wslg_msrdc_path(
            r"c:\program files\remote desktop\msrdcw.exe"
        ));
        assert!(!is_wslg_msrdc_path(r"c:\users\dev\downloads\msrdc.exe"));
    }
}
