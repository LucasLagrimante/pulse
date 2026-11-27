//! Interceptação da tecla Windows para o Pulse Runner (`WH_KEYBOARD_LL` + `WH_MOUSE_LL`).
//!
//! O Menu Iniciar abre quando o shell vê `Win-down` seguido de `Win-up` sem
//! nada no meio. Por isso o keydown físico da tecla Windows é SEMPRE retido no
//! hook, sem prazo de expiração:
//! - soltou sem apertar mais nada → down e up são engolidos e o runner abre;
//!   o shell nunca viu a tecla, então o Iniciar não tem como abrir;
//! - apertou outra tecla (ou clicou/rolou o mouse) → o keydown retido é
//!   reinjetado junto com o evento atual, na mesma `SendInput`, e o atalho
//!   nativo funciona (`Win+E`, `Win+R`, `Win+V`, `Win+D`, `Win+Shift+S`,
//!   `Ctrl+Win+setas`, `Win+clique`, `Win+roda`);
//! - `Win+L` chama `LockWorkStation` diretamente (o shell não bloqueia a
//!   sessão a partir de uma tecla Windows injetada).
//!
//! Uma versão anterior soltava o keydown retido depois de 220ms: qualquer
//! toque mais lento virava `Win-down` injetado + `Win-up` físico e o Iniciar
//! abria. Não reintroduzir prazo de retenção.
//!
//! Os callbacks não alocam nem fazem I/O: no Windows 7+ um hook que estoura o
//! `LowLevelHooksTimeout` é removido em silêncio e, a partir daí, toda tecla
//! Windows abre o Iniciar. Trace e ações vão para a thread de despacho.

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use std::sync::{Arc, Mutex, OnceLock};

const VK_LWIN_CODE: u32 = 0x5B;
const VK_RWIN_CODE: u32 = 0x5C;
const VK_L_CODE: u32 = 0x4C;

/// Sem eventos da tecla retida por mais que isso, a retenção é considerada
/// órfã (keyup engolido por outro hook/jogo). Segurar a tecla gera auto-repeat
/// a cada ~33ms depois do atraso inicial (máx. 1s), o que mantém a retenção viva.
const STALE_MS: u32 = 1500;

/// Máquina de estados das teclas físicas pressionadas.
///
/// Todas as teclas físicas participam (inclusive modificadores pressionados
/// antes da tecla Windows). Duas teclas Windows sobrepostas cancelam o toque
/// até que ambas sejam soltas.
#[derive(Clone, Copy, Default)]
pub struct Tap {
    held: [u64; 4],
    candidate: Option<u32>,
}

impl Tap {
    /// Verifica se uma tecla está marcada como pressionada na máquina de estados.
    pub fn is_held(&self, vk: u32) -> bool {
        if vk >= 256 {
            return false;
        }
        let slot = vk as usize / 64;
        let bit = 1u64 << (vk % 64);
        self.held[slot] & bit != 0
    }

    /// Reconstrói o estado das teclas físicas a partir do estado assíncrono do
    /// Windows. Usado no `keydown` da tecla Windows para descartar bits órfãos:
    /// jogos, AHK e outros hooks podem engolir um `keyup` e deixar `held` sujo.
    /// As próprias teclas Windows são excluídas: o evento atual ainda não foi
    /// processado pelo sistema quando o callback do hook é chamado.
    pub fn resync_from<F: Fn(u32) -> bool>(&mut self, is_down: F) {
        let mut held = [0u64; 4];
        for vk in 8..256u32 {
            if matches!(vk, VK_LWIN_CODE | VK_RWIN_CODE) {
                continue;
            }
            if is_down(vk) {
                held[vk as usize / 64] |= 1u64 << (vk % 64);
            }
        }
        self.held = held;
        self.candidate = None;
    }

    pub fn has_held_keys(&self) -> bool {
        self.held != [0; 4]
    }

    pub fn key(&mut self, vk: u32, down: bool, injected: bool) -> bool {
        if injected || vk >= 256 {
            return false;
        }
        let slot = vk as usize / 64;
        let bit = 1u64 << (vk % 64);
        let was_down = self.held[slot] & bit != 0;
        let is_win = vk == VK_LWIN_CODE || vk == VK_RWIN_CODE;

        if down {
            if !was_down {
                self.candidate = if is_win && self.held == [0; 4] {
                    Some(vk)
                } else {
                    None
                };
                self.held[slot] |= bit;
            }
            false
        } else {
            self.held[slot] &= !bit;
            let triggered = was_down && self.candidate == Some(vk);
            if is_win {
                self.candidate = None;
            }
            triggered
        }
    }

    pub fn click(&mut self) {
        self.candidate = None;
    }

    /// Cancela a decisão de tap quando o keydown retido é encaminhado (atalho).
    pub fn cancel_candidate(&mut self) {
        self.candidate = None;
    }
}

/// Keydown da tecla Windows retido no hook (o shell ainda não o viu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingWin {
    pub vk: u32,
    pub scan: u32,
    pub extended: bool,
    /// Nenhuma outra tecla estava pressionada quando a tecla Windows desceu.
    clean: bool,
    last_seen: u32,
}

impl PendingWin {
    fn is_stale(&self, now: u32) -> bool {
        now.wrapping_sub(self.last_seen) > STALE_MS
    }
}

/// Evento de teclado já extraído do `KBDLLHOOKSTRUCT`.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub vk: u32,
    pub scan: u32,
    pub extended: bool,
    pub down: bool,
    pub injected: bool,
    /// `KBDLLHOOKSTRUCT::time` (ms, contador que dá a volta).
    pub time: u32,
}

/// O que o hook deve fazer com o evento atual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDecision {
    /// Encaminha o evento sem mudança.
    Pass,
    /// Engole o keydown da tecla Windows e passa a retê-lo.
    Retain,
    /// Engole o evento sem efeito (auto-repeat da tecla retida, keyup de
    /// tecla Windows que desceu junto com outro modificador).
    Swallow,
    /// Toque limpo: engole o keyup e abre o runner.
    Tap,
    /// Atalho `Win+tecla`: reinjeta o keydown retido seguido do evento atual,
    /// na mesma `SendInput`, e engole o original.
    Chord(PendingWin),
    /// `Win+L`: engole a tecla e bloqueia a sessão via `LockWorkStation`.
    Lock,
}

/// Arbitragem da tecla Windows: decide, evento a evento, entre runner e
/// atalho nativo, sem nunca deixar o shell ver um toque isolado da tecla.
#[derive(Clone, Copy, Default)]
pub struct Arbiter {
    tap: Tap,
    pending: Option<PendingWin>,
    /// Último evento físico de tecla Windows já encaminhada (após um atalho).
    win_seen: u32,
}

impl Arbiter {
    /// Estado inicial com as teclas que já estavam pressionadas ao instalar o hook.
    pub fn seeded<F: Fn(u32) -> bool>(is_down: F) -> Self {
        let mut tap = Tap::default();
        tap.resync_from(is_down);
        Self {
            tap,
            ..Self::default()
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Descarta uma retenção órfã (keyup perdido). Nada foi enviado ao shell,
    /// então basta esquecer a tecla.
    fn drop_stale(&mut self, now: u32) {
        if let Some(p) = self.pending {
            if p.is_stale(now) {
                self.pending = None;
                let _ = self.tap.key(p.vk, false, false);
            }
        }
    }

    pub fn key<F: Fn(u32) -> bool>(&mut self, ev: KeyEvent, is_down: F) -> KeyDecision {
        if ev.injected || ev.vk >= 256 {
            return KeyDecision::Pass;
        }
        let is_win = ev.vk == VK_LWIN_CODE || ev.vk == VK_RWIN_CODE;
        self.drop_stale(ev.time);

        match (is_win, ev.down, self.pending) {
            // Auto-repeat da tecla retida: continua retendo.
            (true, true, Some(mut p)) if p.vk == ev.vk => {
                p.last_seen = ev.time;
                self.pending = Some(p);
                KeyDecision::Swallow
            }
            // A outra tecla Windows enquanto uma está retida: as duas viram
            // modificador e seguem para o shell.
            (true, true, Some(p)) => {
                self.pending = None;
                self.tap.cancel_candidate();
                let _ = self.tap.key(ev.vk, true, false);
                self.win_seen = ev.time;
                KeyDecision::Chord(p)
            }
            (true, true, None) => {
                // Tecla Windows já encaminhada num atalho e ainda pressionada:
                // auto-repeat dela segue para o shell.
                if self.tap.is_held(ev.vk) && ev.time.wrapping_sub(self.win_seen) <= STALE_MS {
                    self.win_seen = ev.time;
                    return KeyDecision::Pass;
                }
                if self.tap.has_held_keys() {
                    self.tap.resync_from(&is_down);
                }
                let clean = !self.tap.has_held_keys();
                let _ = self.tap.key(ev.vk, true, false);
                self.pending = Some(PendingWin {
                    vk: ev.vk,
                    scan: ev.scan,
                    extended: ev.extended,
                    clean,
                    last_seen: ev.time,
                });
                KeyDecision::Retain
            }
            (true, false, Some(p)) if p.vk == ev.vk => {
                self.pending = None;
                let _ = self.tap.key(ev.vk, false, false);
                if p.clean {
                    KeyDecision::Tap
                } else {
                    KeyDecision::Swallow
                }
            }
            (true, false, _) => {
                let _ = self.tap.key(ev.vk, false, false);
                self.win_seen = ev.time;
                KeyDecision::Pass
            }
            (false, true, Some(p)) => {
                self.pending = None;
                self.tap.cancel_candidate();
                let _ = self.tap.key(ev.vk, true, false);
                self.win_seen = ev.time;
                if ev.vk == VK_L_CODE {
                    // A tecla Windows nunca foi entregue ao shell; o keyup de
                    // `L` e o da tecla Windows passam sem efeito.
                    let _ = self.tap.key(p.vk, false, false);
                    KeyDecision::Lock
                } else {
                    KeyDecision::Chord(p)
                }
            }
            (false, _, _) => {
                let _ = self.tap.key(ev.vk, ev.down, false);
                KeyDecision::Pass
            }
        }
    }

    /// Clique ou roda do mouse físico. Se havia tecla Windows retida, devolve-a
    /// para ser reinjetada antes do evento do mouse (`Win+clique`).
    pub fn mouse(&mut self, time: u32) -> Option<PendingWin> {
        self.tap.click();
        self.drop_stale(time);
        let pending = self.pending.take()?;
        self.win_seen = time;
        Some(pending)
    }
}

static HOOK_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn is_enabled() -> bool {
    HOOK_ENABLED.load(Ordering::Acquire)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::cell::Cell;
    use std::mem::size_of;
    use windows::core::Result;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Shutdown::LockWorkStation;
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentThread, GetCurrentThreadId, OpenEventW, OpenProcess,
        QueryFullProcessImageNameW, SetEvent, SetThreadPriority, WaitForMultipleObjects,
        WaitForSingleObject, INFINITE, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, SYNCHRONIZATION_SYNCHRONIZE, THREAD_PRIORITY_TIME_CRITICAL,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEINPUT, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
        LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, WH_KEYBOARD_LL,
        WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
        WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
        WM_XBUTTONDOWN,
    };

    const WM_PULSE_WIN_TAP: u32 = WM_USER + 0x41;
    const WM_PULSE_WIN_LOCK: u32 = WM_USER + 0x42;
    const WM_PULSE_WIN_TRACE: u32 = WM_USER + 0x43;
    const WM_PULSE_HOOK_READY: u32 = WM_USER + 0x44;

    // Códigos de trace (wParam de WM_PULSE_WIN_TRACE; lParam = vk).
    const TRACE_RETAIN: usize = 1;
    const TRACE_TAP: usize = 2;
    const TRACE_TAP_DIRTY: usize = 3;
    const TRACE_CHORD: usize = 4;
    const TRACE_MOUSE_CHORD: usize = 5;
    const TRACE_LOCK: usize = 6;
    const TRACE_ELEVATED_START: usize = 7;
    const TRACE_START_FOREGROUND: usize = 8;

    thread_local! {
        static ARBITER: Cell<Arbiter> = Cell::new(Arbiter::default());
        static DISPATCH_THREAD_ID: Cell<u32> = const { Cell::new(0) };
    }

    fn trace_label(kind: usize) -> &'static str {
        match kind {
            TRACE_RETAIN => "DOWN_RETAINED",
            TRACE_TAP => "WIN_TAP -> runner",
            TRACE_TAP_DIRTY => "WIN_UP_IGNORED (modificador já pressionado)",
            TRACE_CHORD => "CHORD (Win reinjetada + tecla)",
            TRACE_MOUSE_CHORD => "MOUSE_CHORD (Win reinjetada + mouse)",
            TRACE_LOCK => "WIN_L -> LockWorkStation",
            TRACE_ELEVATED_START => "ELEVATED_START -> runner (janela elevada; hook sem acesso)",
            TRACE_START_FOREGROUND => {
                "START_FOREGROUND (Iniciar/Pesquisa nativos em primeiro plano)"
            }
            _ => "?",
        }
    }

    /// Diagnóstico: grava as decisões do hook em logs/win-key-trace.log.
    /// Roda só na thread de despacho, nunca dentro do callback do hook.
    fn trace(kind: usize, vk: isize) {
        use std::io::Write;
        let path = crate::storage::paths::pulse_local_dir()
            .join("logs")
            .join("win-key-trace.log");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let _ = writeln!(file, "{millis} {} vk={vk}", trace_label(kind));
        }
    }

    fn post_dispatch(message: u32, wparam: usize, lparam: isize) {
        let tid = DISPATCH_THREAD_ID.with(|slot| slot.get());
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, message, WPARAM(wparam), LPARAM(lparam));
            }
        }
    }

    fn key_input(vk: u32, scan: u32, extended: bool) -> INPUT {
        let mut ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(vk as u16),
            wScan: scan as u16,
            ..Default::default()
        };
        if extended {
            ki.dwFlags = KEYEVENTF_EXTENDEDKEY;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki },
        }
    }

    fn win_down_input(pending: PendingWin) -> INPUT {
        key_input(pending.vk, pending.scan, pending.extended)
    }

    /// Reproduz o botão/roda do mouse sem mover o cursor (ele já está na
    /// posição do evento original, pois os movimentos não são retidos).
    fn mouse_input(message: u32, mouse_data: u32) -> Option<INPUT> {
        let high_word = mouse_data >> 16;
        let (flags, data) = match message {
            WM_LBUTTONDOWN => (MOUSEEVENTF_LEFTDOWN, 0),
            WM_RBUTTONDOWN => (MOUSEEVENTF_RIGHTDOWN, 0),
            WM_MBUTTONDOWN => (MOUSEEVENTF_MIDDLEDOWN, 0),
            WM_XBUTTONDOWN => (MOUSEEVENTF_XDOWN, high_word),
            // Delta da roda é com sinal: estende o sinal da palavra alta.
            WM_MOUSEWHEEL => (MOUSEEVENTF_WHEEL, high_word as u16 as i16 as i32 as u32),
            WM_MOUSEHWHEEL => (MOUSEEVENTF_HWHEEL, high_word as u16 as i16 as i32 as u32),
            _ => return None,
        };
        Some(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: data,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        })
    }

    fn send(inputs: &[INPUT]) {
        unsafe {
            let _ = SendInput(inputs, size_of::<INPUT>() as i32);
        }
    }

    /// Contorno do UIPI: com uma janela elevada em foco (ex.: "Administrator:
    /// PowerShell") o `WH_KEYBOARD_LL` de um processo comum não recebe a tecla,
    /// e o Iniciar nativo abre. Um `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`
    /// (passivo, sem tocar no teclado) percebe o Iniciar ganhando foco logo
    /// após uma janela elevada e abre o runner por cima; o Iniciar se fecha
    /// sozinho ao perder o foco.
    mod elevated {
        use super::*;
        use std::cell::RefCell;
        use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
        use windows::Win32::Security::{
            GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
        };
        use windows::Win32::System::Threading::{
            OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowThreadProcessId, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
            WINEVENT_SKIPOWNPROCESS,
        };

        const START_MENU_HOSTS: &[&str] = &[
            "startmenuexperiencehost.exe",
            "searchhost.exe",
            "searchapp.exe",
        ];

        thread_local! {
            static ON_TAP: RefCell<Option<Arc<dyn Fn() + Send + Sync>>> = const { RefCell::new(None) };
            /// A última janela em foco (fora do Pulse) era elevada.
            static LAST_ELEVATED: Cell<bool> = const { Cell::new(false) };
        }

        pub struct Watch(HWINEVENTHOOK);

        impl Drop for Watch {
            fn drop(&mut self) {
                unsafe {
                    let _ = UnhookWinEvent(self.0);
                }
                ON_TAP.with(|slot| slot.borrow_mut().take());
            }
        }

        /// Instala o observador na thread atual (que precisa bombear mensagens).
        pub fn watch(on_tap: Arc<dyn Fn() + Send + Sync>) -> Option<Watch> {
            ON_TAP.with(|slot| *slot.borrow_mut() = Some(on_tap));
            LAST_ELEVATED.with(|last| last.set(false));
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(on_foreground),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                crate::debug::record_failure(
                    "launcher_win_key_elevated",
                    0,
                    crate::debug::Failure::new(
                        "Não foi possível observar o foco para janelas elevadas",
                    )
                    .stage("SetWinEventHook"),
                );
                return None;
            }
            Some(Watch(hook))
        }

        unsafe extern "system" fn on_foreground(
            _hook: HWINEVENTHOOK,
            _event: u32,
            hwnd: HWND,
            _id_object: i32,
            _id_child: i32,
            _thread: u32,
            _time: u32,
        ) {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return;
            }
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return;
            };
            if is_start_menu_host(process) {
                let _ = CloseHandle(process);
                // Diagnóstico: registra toda vez que o Iniciar ganha o foco,
                // para cruzar com os WIN_TAP do mesmo log.
                trace(TRACE_START_FOREGROUND, 0);
                if LAST_ELEVATED.with(|last| last.replace(false)) {
                    trace(TRACE_ELEVATED_START, 0);
                    let on_tap = ON_TAP.with(|slot| slot.borrow().clone());
                    if let Some(on_tap) = on_tap {
                        on_tap();
                    }
                }
                return;
            }
            let elevated = is_elevated(process);
            let _ = CloseHandle(process);
            LAST_ELEVATED.with(|last| last.set(elevated));
        }

        unsafe fn is_start_menu_host(process: HANDLE) -> bool {
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .is_err()
            {
                return false;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
            START_MENU_HOSTS
                .iter()
                .any(|host| name.eq_ignore_ascii_case(host))
        }

        /// Processo elevado (ou com token inacessível para nós, o que, para
        /// uma janela de primeiro plano da sessão, também indica elevação).
        unsafe fn is_elevated(process: HANDLE) -> bool {
            let mut token = HANDLE::default();
            if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
                return true;
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut returned = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
            .is_ok();
            let _ = CloseHandle(token);
            ok && elevation.TokenIsElevated != 0
        }
    }

    /// Argumento do processo auxiliar que segura a tecla Windows.
    ///
    /// O hook LL roda num processo separado do Pulse: com o hook no mesmo
    /// processo da janela em foco (runner/janela principal, WebView2), o Iniciar
    /// do Windows abria mesmo com o toque engolido. Com o hook em outro processo
    /// (como fazem WindhawkPal/AutoHotkey) isso não acontece.
    pub const HOOK_HOST_ARG: &str = "--win-key-hook";

    /// Processo que segura os hooks LL.
    enum Host {
        /// Filho comum (integridade média): não vê teclas de janelas elevadas.
        Child(std::process::Child),
        /// Cópia protegida iniciada elevada pela tarefa agendada. O Pulse comum
        /// não pode encerrá-la; ela sai quando este evento é sinalizado.
        Task { stop_event: isize },
    }

    impl Host {
        fn stop(self) {
            match self {
                Host::Child(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Host::Task { stop_event } => unsafe {
                    let event = HANDLE(stop_event as *mut _);
                    let _ = SetEvent(event);
                    let _ = CloseHandle(event);
                },
            }
        }
    }

    pub struct Hook {
        dispatch_thread_id: u32,
        dispatch_handle: Option<std::thread::JoinHandle<()>>,
        host: Option<Host>,
    }

    impl Hook {
        pub fn install(on_tap: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
            // Thread de despacho recebe os eventos do processo auxiliar via
            // PostThreadMessageW (trace, toque, Win+L) e observa o foco.
            let (dispatch_tx, dispatch_rx) = std::sync::mpsc::channel::<u32>();
            let (host_ready_tx, host_ready_rx) = std::sync::mpsc::channel::<()>();
            let dispatch_handle = std::thread::Builder::new()
                .name("pulse-win-key-dispatch".into())
                .spawn(move || unsafe {
                    let mut msg = MSG::default();
                    let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
                    let elevated_watch = elevated::watch(on_tap.clone());
                    let _ = dispatch_tx.send(GetCurrentThreadId());
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        match msg.message {
                            WM_PULSE_WIN_TAP => on_tap(),
                            WM_PULSE_HOOK_READY => {
                                let _ = host_ready_tx.send(());
                            }
                            WM_PULSE_WIN_LOCK => {
                                if let Err(error) = LockWorkStation() {
                                    crate::debug::record_failure(
                                        "launcher_win_key_lock",
                                        0,
                                        crate::debug::Failure::new(
                                            "Win+L não conseguiu bloquear a sessão",
                                        )
                                        .stage("LockWorkStation")
                                        .detail(error.to_string()),
                                    );
                                }
                            }
                            WM_PULSE_WIN_TRACE => trace(msg.wParam.0, msg.lParam.0),
                            _ => {}
                        }
                    }
                    drop(elevated_watch);
                })
                .map_err(|error| {
                    windows::core::Error::new(windows::core::HRESULT(-1), error.to_string())
                })?;

            let dispatch_thread_id = match dispatch_rx.recv() {
                Ok(tid) => tid,
                Err(_) => {
                    let _ = dispatch_handle.join();
                    return Err(windows::core::Error::new(
                        windows::core::HRESULT(-1),
                        "Thread de despacho da tecla Windows encerrou inesperadamente.",
                    ));
                }
            };

            let stop_dispatch = |dispatch_handle: std::thread::JoinHandle<()>| {
                unsafe {
                    let _ = PostThreadMessageW(dispatch_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
                let _ = dispatch_handle.join();
            };

            // Elevado (tarefa agendada) quando configurado e em dia; senão, ou se
            // a tarefa não responder, o processo comum.
            if super::super::win_key_admin::status()
                == super::super::win_key_admin::WinKeyAdminState::Ready
            {
                match start_task_host(dispatch_thread_id) {
                    Ok(stop_event) => {
                        let host = Host::Task { stop_event };
                        if host_ready_rx
                            .recv_timeout(std::time::Duration::from_secs(4))
                            .is_ok()
                        {
                            return Ok(Self {
                                dispatch_thread_id,
                                dispatch_handle: Some(dispatch_handle),
                                host: Some(host),
                            });
                        }
                        host.stop();
                        crate::debug::record_failure(
                            "launcher_win_key_admin",
                            4000,
                            crate::debug::Failure::new(
                                "O processo da tecla como administrador não respondeu; usando o processo comum",
                            )
                            .stage("task_ready"),
                        );
                    }
                    Err(error) => crate::debug::record_failure(
                        "launcher_win_key_admin",
                        0,
                        crate::debug::Failure::new(
                            "Não foi possível iniciar o processo da tecla como administrador; usando o processo comum",
                        )
                        .stage("run_task")
                        .detail(error),
                    ),
                }
            }

            let host = match spawn_host(dispatch_thread_id) {
                Ok(child) => child,
                Err(error) => {
                    stop_dispatch(dispatch_handle);
                    return Err(windows::core::Error::new(
                        windows::core::HRESULT(-1),
                        format!("Não foi possível iniciar o processo da tecla Windows: {error}"),
                    ));
                }
            };

            match host_ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(()) => Ok(Self {
                    dispatch_thread_id,
                    dispatch_handle: Some(dispatch_handle),
                    host: Some(Host::Child(host)),
                }),
                Err(_) => {
                    let mut host = host;
                    let status = host.try_wait().ok().flatten();
                    let _ = host.kill();
                    let _ = host.wait();
                    stop_dispatch(dispatch_handle);
                    Err(windows::core::Error::new(
                        windows::core::HRESULT(-1),
                        match status {
                            Some(status) => format!(
                                "O processo da tecla Windows encerrou antes de instalar o hook ({status})."
                            ),
                            None => "O processo da tecla Windows não instalou o hook em 5s."
                                .to_string(),
                        },
                    ))
                }
            }
        }
    }

    impl Drop for Hook {
        fn drop(&mut self) {
            // Encerrar o processo remove os hooks LL junto.
            if let Some(host) = self.host.take() {
                host.stop();
            }
            unsafe {
                let _ = PostThreadMessageW(self.dispatch_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(handle) = self.dispatch_handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn spawn_host(dispatch_thread_id: u32) -> std::io::Result<std::process::Child> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let exe = std::env::current_exe()?;
        std::process::Command::new(exe)
            .args([
                HOOK_HOST_ARG,
                &dispatch_thread_id.to_string(),
                &std::process::id().to_string(),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    }

    /// Arquivo com que o Pulse passa `<thread de despacho> <pid> <evento>` à
    /// tarefa agendada (que não recebe argumentos variáveis).
    fn handoff_path() -> std::path::PathBuf {
        crate::storage::paths::pulse_local_dir().join("win-key-hook.handoff")
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Cria o evento de parada, grava o handoff e dispara a tarefa agendada.
    fn start_task_host(dispatch_thread_id: u32) -> std::result::Result<isize, String> {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let name = format!("Local\\PulseWinKeyHookStop-{pid}-{seq}");
        let name_w = wide(&name);
        let event = unsafe { CreateEventW(None, true, false, PCWSTR(name_w.as_ptr())) }
            .map_err(|error| format!("CreateEventW: {error}"))?;
        let stop_event = event.0 as isize;
        let fail = |error: String| {
            unsafe {
                let _ = CloseHandle(event);
            }
            error
        };
        let path = handoff_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, format!("{dispatch_thread_id} {pid} {name}"))
            .map_err(|error| fail(format!("handoff: {error}")))?;
        super::super::win_key_admin::run_task().map_err(fail)?;
        Ok(stop_event)
    }

    /// O processo é uma instância do Pulse (`pulse-windows.exe`).
    fn is_pulse_process(pid: u32) -> bool {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut buffer = [0u16; 1024];
            let mut len = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buffer.as_mut_ptr()),
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(process);
            ok && String::from_utf16_lossy(&buffer[..len as usize])
                .rsplit('\\')
                .next()
                .is_some_and(|file| file.eq_ignore_ascii_case("pulse-windows.exe"))
        }
    }

    /// Modo processo auxiliar. Comum: `--win-key-hook <thread de despacho> <pid do Pulse>`.
    /// Elevado (tarefa agendada): `--win-key-hook-task`, lendo o handoff.
    /// Instala os hooks LL, avisa o Pulse e bombeia mensagens até o Pulse sair.
    pub fn run_host_if_requested() -> bool {
        let args: Vec<String> = std::env::args().collect();
        if args
            .iter()
            .any(|arg| arg == super::super::win_key_admin::TASK_HOST_ARG)
        {
            let Ok(handoff) = std::fs::read_to_string(handoff_path()) else {
                return true;
            };
            let mut parts = handoff.split_whitespace();
            let (Some(tid), Some(pid), Some(event)) = (
                parts.next().and_then(|v| v.parse::<u32>().ok()),
                parts.next().and_then(|v| v.parse::<u32>().ok()),
                parts.next(),
            ) else {
                return true;
            };
            if !is_pulse_process(pid) {
                return true;
            }
            let event_w = wide(event);
            let Ok(stop) = (unsafe {
                OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(event_w.as_ptr()))
            }) else {
                return true;
            };
            // O Pulse já desistiu desta instância (timeout): não instala nada.
            if unsafe { WaitForSingleObject(stop, 0) } == WAIT_OBJECT_0 {
                return true;
            }
            run_host(tid, pid, Some(stop.0 as isize));
            return true;
        }
        let Some(pos) = args.iter().position(|arg| arg == HOOK_HOST_ARG) else {
            return false;
        };
        let parse = |offset: usize| args.get(pos + offset).and_then(|v| v.parse::<u32>().ok());
        if let (Some(dispatch_thread_id), Some(parent_pid)) = (parse(1), parse(2)) {
            run_host(dispatch_thread_id, parent_pid, None);
        }
        true
    }

    fn run_host(dispatch_thread_id: u32, parent_pid: u32, stop_event: Option<isize>) {
        unsafe {
            // Sai junto com o Pulse (inclusive se ele cair sem chamar Drop) ou
            // quando o Pulse sinaliza o evento de parada (modo elevado).
            let Ok(parent) = OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) else {
                return;
            };
            let parent_raw = parent.0 as isize;
            std::thread::spawn(move || {
                let parent = HANDLE(parent_raw as *mut _);
                match stop_event {
                    Some(stop) => {
                        let _ = WaitForMultipleObjects(
                            &[parent, HANDLE(stop as *mut _)],
                            false,
                            INFINITE,
                        );
                    }
                    None => {
                        let _ = WaitForSingleObject(parent, INFINITE);
                    }
                }
                std::process::exit(0);
            });

            // O Windows ignora a resposta de um hook LL que demora
            // (LowLevelHooksTimeout): a thread só decide e repassa, em prioridade máxima.
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
            let Ok(module) = GetModuleHandleW(None) else {
                return;
            };
            let Ok(keyboard) =
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(module.into()), 0)
            else {
                return;
            };
            let Ok(mouse) =
                SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(module.into()), 0)
            else {
                let _ = UnhookWindowsHookEx(keyboard);
                return;
            };

            // Estado inicial com as teclas já pressionadas ao ativar o hook.
            let arbiter = Arbiter::seeded(|vk| GetAsyncKeyState(vk as i32) < 0);
            ARBITER.with(|state| state.set(arbiter));
            DISPATCH_THREAD_ID.with(|slot| slot.set(dispatch_thread_id));
            post_dispatch(WM_PULSE_HOOK_READY, 0, 0);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(keyboard);
            let _ = UnhookWindowsHookEx(mouse);
        }
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code != HC_ACTION as i32 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let message = wparam.0 as u32;
        if !matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let key = KeyEvent {
            vk: event.vkCode,
            scan: event.scanCode,
            extended: event.flags.contains(LLKHF_EXTENDED),
            down: matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN),
            injected: event.flags.contains(LLKHF_INJECTED),
            time: event.time,
        };
        let decision = ARBITER.with(|state| {
            let mut arbiter = state.get();
            let decision = arbiter.key(key, |vk| GetAsyncKeyState(vk as i32) < 0);
            state.set(arbiter);
            decision
        });
        let vk = key.vk as isize;

        match decision {
            KeyDecision::Pass => CallNextHookEx(None, code, wparam, lparam),
            KeyDecision::Swallow => LRESULT(1),
            KeyDecision::Retain => {
                post_dispatch(WM_PULSE_WIN_TRACE, TRACE_RETAIN, vk);
                LRESULT(1)
            }
            KeyDecision::Tap => {
                post_dispatch(WM_PULSE_WIN_TAP, 0, 0);
                post_dispatch(WM_PULSE_WIN_TRACE, TRACE_TAP, vk);
                LRESULT(1)
            }
            KeyDecision::Chord(pending) => {
                // Tecla Windows retida e a tecla atual na mesma SendInput: a
                // ordem chega intacta ao shell e o caractere não vaza no app.
                send(&[
                    win_down_input(pending),
                    key_input(key.vk, key.scan, key.extended),
                ]);
                post_dispatch(WM_PULSE_WIN_TRACE, TRACE_CHORD, vk);
                LRESULT(1)
            }
            KeyDecision::Lock => {
                post_dispatch(WM_PULSE_WIN_LOCK, 0, 0);
                post_dispatch(WM_PULSE_WIN_TRACE, TRACE_LOCK, vk);
                LRESULT(1)
            }
        }
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let message = wparam.0 as u32;
        if code != HC_ACTION as i32
            || !matches!(
                message,
                WM_LBUTTONDOWN
                    | WM_RBUTTONDOWN
                    | WM_MBUTTONDOWN
                    | WM_XBUTTONDOWN
                    | WM_MOUSEWHEEL
                    | WM_MOUSEHWHEEL
            )
        {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let event = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if event.flags & LLMHF_INJECTED != 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let pending = ARBITER.with(|state| {
            let mut arbiter = state.get();
            let pending = arbiter.mouse(event.time);
            state.set(arbiter);
            pending
        });
        let Some(pending) = pending else {
            return CallNextHookEx(None, code, wparam, lparam);
        };
        post_dispatch(WM_PULSE_WIN_TRACE, TRACE_MOUSE_CHORD, pending.vk as isize);
        match mouse_input(message, event.mouseData) {
            Some(mouse) => {
                send(&[win_down_input(pending), mouse]);
                LRESULT(1)
            }
            None => {
                send(&[win_down_input(pending)]);
                CallNextHookEx(None, code, wparam, lparam)
            }
        }
    }

    static ACTIVE_HOOK: OnceLock<Mutex<Option<Hook>>> = OnceLock::new();

    fn slot() -> &'static Mutex<Option<Hook>> {
        ACTIVE_HOOK.get_or_init(|| Mutex::new(None))
    }

    pub fn set_enabled(
        enabled: bool,
        on_tap: Arc<dyn Fn() + Send + Sync>,
    ) -> std::result::Result<(), String> {
        let mut guard = slot().lock().unwrap_or_else(|e| e.into_inner());
        if !enabled {
            *guard = None;
            HOOK_ENABLED.store(false, Ordering::Release);
            return Ok(());
        }
        if guard.is_some() {
            HOOK_ENABLED.store(true, Ordering::Release);
            return Ok(());
        }
        match Hook::install(on_tap) {
            Ok(hook) => {
                *guard = Some(hook);
                HOOK_ENABLED.store(true, Ordering::Release);
                Ok(())
            }
            Err(err) => {
                HOOK_ENABLED.store(false, Ordering::Release);
                Err(format!("Falha ao instalar hook da tecla Windows: {err}"))
            }
        }
    }
}

/// Ativa ou desativa dinamicamente o hook da tecla Windows.
pub fn set_enabled(enabled: bool, on_tap: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
    #[cfg(windows)]
    {
        platform::set_enabled(enabled, Arc::new(on_tap))
    }
    #[cfg(not(windows))]
    {
        let _ = on_tap;
        HOOK_ENABLED.store(enabled, Ordering::Release);
        Ok(())
    }
}

/// Se o binário foi iniciado como processo auxiliar da tecla Windows
/// (`--win-key-hook`), roda o hook até o Pulse sair e devolve `true`.
pub fn run_hook_host_if_requested() -> bool {
    #[cfg(windows)]
    {
        platform::run_host_if_requested()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: u32 = VK_LWIN_CODE;
    const R: u32 = VK_RWIN_CODE;
    const E: u32 = b'E' as u32;
    const S: u32 = b'S' as u32;
    const LSHIFT: u32 = 0xA0;
    const LCTRL: u32 = 0xA2;

    fn sequence(events: &[(u32, bool, bool)], expected: &[usize]) {
        let mut tap = Tap::default();
        let taps: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(i, &(vk, down, injected))| tap.key(vk, down, injected).then_some(i))
            .collect();
        assert_eq!(taps, expected);
    }

    fn ev(vk: u32, down: bool, time: u32) -> KeyEvent {
        KeyEvent {
            vk,
            scan: 0,
            extended: vk == L || vk == R,
            down,
            injected: false,
            time,
        }
    }

    /// Roda a sequência no árbitro sem nenhuma tecla física extra pressionada.
    fn arbitrate(arbiter: &mut Arbiter, events: &[(u32, bool, u32)]) -> Vec<KeyDecision> {
        events
            .iter()
            .map(|&(vk, down, time)| arbiter.key(ev(vk, down, time), |_| false))
            .collect()
    }

    fn is_chord(decision: &KeyDecision, win: u32) -> bool {
        matches!(decision, KeyDecision::Chord(p) if p.vk == win)
    }

    #[test]
    fn quick_tap_never_reaches_the_shell_and_opens_runner() {
        let mut a = Arbiter::default();
        let d = arbitrate(&mut a, &[(L, true, 0), (L, false, 80)]);
        assert_eq!(d, [KeyDecision::Retain, KeyDecision::Tap]);
        assert!(!a.has_pending());
    }

    #[test]
    fn slow_tap_with_auto_repeat_still_opens_runner() {
        // Regressão: a versão com prazo de 220ms soltava o keydown e o Iniciar abria.
        let mut a = Arbiter::default();
        let mut events = vec![(L, true, 0)];
        let mut t = 500;
        while t <= 3000 {
            events.push((L, true, t));
            t += 33;
        }
        events.push((L, false, t));
        let d = arbitrate(&mut a, &events);
        assert_eq!(d[0], KeyDecision::Retain);
        assert!(d[1..d.len() - 1].iter().all(|x| *x == KeyDecision::Swallow));
        assert_eq!(*d.last().unwrap(), KeyDecision::Tap);
    }

    #[test]
    fn long_hold_without_repeat_still_never_forwards_win_alone() {
        let mut a = Arbiter::default();
        let d = arbitrate(&mut a, &[(L, true, 0), (L, false, 900)]);
        assert_eq!(d, [KeyDecision::Retain, KeyDecision::Tap]);
    }

    #[test]
    fn win_plus_key_reinjects_win_before_the_key() {
        let mut a = Arbiter::default();
        let d = arbitrate(
            &mut a,
            &[
                (L, true, 0),
                (E, true, 120),
                (E, false, 180),
                (L, false, 260),
            ],
        );
        assert_eq!(d[0], KeyDecision::Retain);
        assert!(is_chord(&d[1], L));
        assert_eq!(&d[2..], [KeyDecision::Pass, KeyDecision::Pass]);
    }

    #[test]
    fn win_shift_s_and_ctrl_win_arrows_keep_working() {
        let mut a = Arbiter::default();
        let d = arbitrate(
            &mut a,
            &[
                (L, true, 0),
                (LSHIFT, true, 50),
                (S, true, 90),
                (S, false, 140),
                (LSHIFT, false, 150),
                (L, false, 200),
            ],
        );
        assert!(is_chord(&d[1], L));
        assert!(d[2..].iter().all(|x| *x == KeyDecision::Pass));

        // Ctrl já pressionado antes da tecla Windows: ainda retém, e a seta reinjeta.
        let mut a = Arbiter::default();
        assert_eq!(a.key(ev(LCTRL, true, 0), |_| false), KeyDecision::Pass);
        assert_eq!(
            a.key(ev(L, true, 40), |vk| vk == LCTRL),
            KeyDecision::Retain
        );
        assert!(is_chord(&a.key(ev(0x27, true, 90), |vk| vk == LCTRL), L));
    }

    #[test]
    fn win_released_with_modifier_held_is_swallowed_without_runner() {
        let mut a = Arbiter::default();
        assert_eq!(a.key(ev(LCTRL, true, 0), |_| false), KeyDecision::Pass);
        assert_eq!(
            a.key(ev(L, true, 40), |vk| vk == LCTRL),
            KeyDecision::Retain
        );
        assert_eq!(
            a.key(ev(L, false, 90), |vk| vk == LCTRL),
            KeyDecision::Swallow
        );
    }

    #[test]
    fn win_l_locks_the_session_directly() {
        let mut a = Arbiter::default();
        let d = arbitrate(
            &mut a,
            &[
                (L, true, 0),
                (VK_L_CODE, true, 60),
                (VK_L_CODE, false, 100),
                (L, false, 130),
            ],
        );
        assert_eq!(
            d,
            [
                KeyDecision::Retain,
                KeyDecision::Lock,
                KeyDecision::Pass,
                KeyDecision::Pass
            ]
        );
    }

    #[test]
    fn mouse_click_while_retained_reinjects_win() {
        let mut a = Arbiter::default();
        assert_eq!(a.key(ev(L, true, 0), |_| false), KeyDecision::Retain);
        assert_eq!(a.mouse(50).map(|p| p.vk), Some(L));
        // O keyup da tecla Windows segue para o shell (fecha o Win+clique).
        assert_eq!(a.key(ev(L, false, 120), |_| false), KeyDecision::Pass);
        // Clique sem tecla retida não injeta nada.
        assert_eq!(a.mouse(200), None);
    }

    #[test]
    fn second_win_key_while_retained_forwards_both() {
        let mut a = Arbiter::default();
        let d = arbitrate(
            &mut a,
            &[(L, true, 0), (R, true, 40), (L, false, 90), (R, false, 120)],
        );
        assert_eq!(d[0], KeyDecision::Retain);
        assert!(is_chord(&d[1], L));
        assert_eq!(&d[2..], [KeyDecision::Pass, KeyDecision::Pass]);
        // Depois disso, um toque limpo volta a abrir o runner.
        let d = arbitrate(&mut a, &[(L, true, 400), (L, false, 450)]);
        assert_eq!(d, [KeyDecision::Retain, KeyDecision::Tap]);
    }

    #[test]
    fn orphaned_retention_is_dropped_instead_of_turning_keys_into_shortcuts() {
        // Keyup da tecla Windows engolido por outro hook: o "E" digitado muito
        // depois não pode virar Win+E.
        let mut a = Arbiter::default();
        assert_eq!(a.key(ev(L, true, 0), |_| false), KeyDecision::Retain);
        assert_eq!(a.key(ev(E, true, 5000), |_| false), KeyDecision::Pass);
        assert!(!a.has_pending());
        let d = arbitrate(
            &mut a,
            &[(E, false, 5050), (L, true, 6000), (L, false, 6060)],
        );
        assert_eq!(
            d,
            [KeyDecision::Pass, KeyDecision::Retain, KeyDecision::Tap]
        );
    }

    #[test]
    fn injected_events_pass_untouched() {
        let mut a = Arbiter::default();
        let injected = KeyEvent {
            injected: true,
            ..ev(L, true, 0)
        };
        assert_eq!(a.key(injected, |_| false), KeyDecision::Pass);
        assert!(!a.has_pending());
    }

    #[test]
    fn seeded_state_ignores_win_keys_already_down() {
        let mut a = Arbiter::seeded(|vk| vk == L);
        let d = arbitrate(&mut a, &[(L, true, 0), (L, false, 60)]);
        assert_eq!(d, [KeyDecision::Retain, KeyDecision::Tap]);
    }

    #[test]
    fn lone_and_repeated_taps_on_either_side() {
        sequence(
            &[
                (L, true, false),
                (L, false, false),
                (R, true, false),
                (R, false, false),
                (L, true, false),
                (L, false, false),
            ],
            &[1, 3, 5],
        );
    }

    #[test]
    fn chords_cancel_even_after_other_key_released() {
        // Win+E em várias ordens de soltura
        sequence(
            &[
                (L, true, false),
                (E, true, false),
                (E, false, false),
                (L, false, false),
            ],
            &[],
        );
        sequence(
            &[
                (L, true, false),
                (E, true, false),
                (L, false, false),
                (E, false, false),
            ],
            &[],
        );
        sequence(
            &[
                (E, true, false),
                (L, true, false),
                (E, false, false),
                (L, false, false),
            ],
            &[],
        );
        // Win+Shift+S (captura de tela do Windows)
        sequence(
            &[
                (L, true, false),
                (LSHIFT, true, false),
                (S, true, false),
                (S, false, false),
                (LSHIFT, false, false),
                (L, false, false),
            ],
            &[],
        );
    }

    #[test]
    fn overlapping_win_keys_cancel_both_taps() {
        for (a, b) in [(L, R), (R, L)] {
            sequence(
                &[
                    (a, true, false),
                    (b, true, false),
                    (a, false, false),
                    (b, false, false),
                    (a, true, false),
                    (a, false, false),
                ],
                &[5],
            );
        }
    }

    #[test]
    fn mouse_click_cancels_until_next_tap() {
        let mut tap = Tap::default();
        assert!(!tap.key(L, true, false));
        tap.click();
        assert!(!tap.key(L, false, false));
        assert!(!tap.key(L, true, false));
        assert!(tap.key(L, false, false));
    }

    #[test]
    fn resync_discards_orphaned_keyups_and_keeps_real_modifiers() {
        let mut tap = Tap::default();
        // Shift realmente pressionado + um keyup de "E" engolido por outro hook.
        assert!(!tap.key(LSHIFT, true, false));
        assert!(!tap.key(E, true, false));
        assert!(tap.has_held_keys());
        tap.resync_from(|vk| vk == LSHIFT);
        assert!(tap.is_held(LSHIFT));
        assert!(!tap.is_held(E));
        tap.resync_from(|_| false);
        assert!(!tap.has_held_keys());
    }

    #[test]
    fn resync_never_marks_the_windows_keys_as_held() {
        let mut tap = Tap::default();
        tap.resync_from(|vk| vk == L || vk == R);
        assert!(!tap.is_held(L));
        assert!(!tap.is_held(R));
        assert!(!tap.has_held_keys());
    }

    #[test]
    fn ignores_out_of_range_virtual_keys() {
        let mut a = Arbiter::default();
        assert_eq!(a.key(ev(300, true, 0), |_| false), KeyDecision::Pass);
        let d = arbitrate(&mut a, &[(L, true, 10), (L, false, 60)]);
        assert_eq!(d, [KeyDecision::Retain, KeyDecision::Tap]);
    }
}
