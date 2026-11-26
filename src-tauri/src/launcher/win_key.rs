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
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Shutdown::LockWorkStation;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEINPUT, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
        LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, WH_KEYBOARD_LL,
        WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
        WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
        WM_XBUTTONDOWN,
    };

    const WM_PULSE_WIN_TAP: u32 = WM_USER + 0x41;
    const WM_PULSE_WIN_LOCK: u32 = WM_USER + 0x42;
    const WM_PULSE_WIN_TRACE: u32 = WM_USER + 0x43;

    // Códigos de trace (wParam de WM_PULSE_WIN_TRACE; lParam = vk).
    const TRACE_RETAIN: usize = 1;
    const TRACE_TAP: usize = 2;
    const TRACE_TAP_DIRTY: usize = 3;
    const TRACE_CHORD: usize = 4;
    const TRACE_MOUSE_CHORD: usize = 5;
    const TRACE_LOCK: usize = 6;

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

    pub struct Hook {
        hook_thread_id: u32,
        dispatch_thread_id: u32,
        hook_handle: Option<std::thread::JoinHandle<()>>,
        dispatch_handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Hook {
        pub fn install(on_tap: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
            // Thread de despacho recebe os eventos via PostThreadMessageW para
            // manter o callback do hook livre de alocações, I/O e bloqueios.
            let (dispatch_tx, dispatch_rx) = std::sync::mpsc::channel::<u32>();
            let dispatch_handle = std::thread::Builder::new()
                .name("pulse-win-key-dispatch".into())
                .spawn(move || unsafe {
                    let mut msg = MSG::default();
                    let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
                    let _ = dispatch_tx.send(GetCurrentThreadId());
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        match msg.message {
                            WM_PULSE_WIN_TAP => on_tap(),
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

            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32>>();
            let hook_handle = match std::thread::Builder::new()
                .name("pulse-win-key".into())
                .spawn(move || unsafe {
                    let installed = (|| -> Result<(HHOOK, HHOOK)> {
                        let module = GetModuleHandleW(None)?;
                        let keyboard = SetWindowsHookExW(
                            WH_KEYBOARD_LL,
                            Some(keyboard_proc),
                            Some(module.into()),
                            0,
                        )?;
                        match SetWindowsHookExW(
                            WH_MOUSE_LL,
                            Some(mouse_proc),
                            Some(module.into()),
                            0,
                        ) {
                            Ok(mouse) => Ok((keyboard, mouse)),
                            Err(error) => {
                                let _ = UnhookWindowsHookEx(keyboard);
                                Err(error)
                            }
                        }
                    })();

                    let (keyboard, mouse) = match installed {
                        Ok(hooks) => hooks,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    };

                    // Estado inicial com as teclas já pressionadas ao ativar o hook.
                    let arbiter = Arbiter::seeded(|vk| GetAsyncKeyState(vk as i32) < 0);
                    ARBITER.with(|state| state.set(arbiter));
                    DISPATCH_THREAD_ID.with(|slot| slot.set(dispatch_thread_id));

                    let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = DispatchMessageW(&msg);
                    }
                    let _ = UnhookWindowsHookEx(keyboard);
                    let _ = UnhookWindowsHookEx(mouse);
                }) {
                Ok(handle) => handle,
                Err(error) => {
                    unsafe {
                        let _ =
                            PostThreadMessageW(dispatch_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                    }
                    let _ = dispatch_handle.join();
                    return Err(windows::core::Error::new(
                        windows::core::HRESULT(-1),
                        error.to_string(),
                    ));
                }
            };

            let stop_after_failure =
                |hook_handle: std::thread::JoinHandle<()>,
                 dispatch_handle: std::thread::JoinHandle<()>| {
                    unsafe {
                        let _ =
                            PostThreadMessageW(dispatch_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                    }
                    let _ = hook_handle.join();
                    let _ = dispatch_handle.join();
                };

            match ready_rx.recv() {
                Ok(Ok(hook_thread_id)) => Ok(Self {
                    hook_thread_id,
                    dispatch_thread_id,
                    hook_handle: Some(hook_handle),
                    dispatch_handle: Some(dispatch_handle),
                }),
                Ok(Err(error)) => {
                    stop_after_failure(hook_handle, dispatch_handle);
                    Err(error)
                }
                Err(_) => {
                    stop_after_failure(hook_handle, dispatch_handle);
                    Err(windows::core::Error::new(
                        windows::core::HRESULT(-1),
                        "O hook da tecla Windows parou antes de concluir a inicialização.",
                    ))
                }
            }
        }
    }

    impl Drop for Hook {
        fn drop(&mut self) {
            unsafe {
                let _ = PostThreadMessageW(self.hook_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                let _ = PostThreadMessageW(self.dispatch_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(handle) = self.hook_handle.take() {
                let _ = handle.join();
            }
            if let Some(handle) = self.dispatch_handle.take() {
                let _ = handle.join();
            }
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
