//! Interceptação segura de toque limpo na tecla Windows (`WH_KEYBOARD_LL` + `WH_MOUSE_LL`).
//!
//! Executa em thread dedicada (`pulse-win-key`) sem alocar memória nem gravar
//! logs dentro dos callbacks de baixo nível. Ao detectar um toque solitário na
//! tecla Windows (sem outras teclas ou cliques de mouse simultâneos), injeta o
//! par virtual `0xE8` (máscara recomendada para suprimir o Menu Iniciar) e
//! preserva integralmente todos os atalhos nativos (`Win+E`, `Win+R`, `Win+V`,
//! `Win+L`, `Win+D`, `Win+Shift+S`, `Win+clique`, `Win+roda do mouse`).

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use std::sync::{Arc, Mutex, OnceLock};

const VK_LWIN_CODE: u32 = 0x5B;
const VK_RWIN_CODE: u32 = 0x5C;

/// Máquina de estados pura para detectar toque limpo na tecla Windows.
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
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
        LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
        WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT,
        WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER, WM_XBUTTONDOWN,
    };

    const WM_PULSE_WIN_TAP: u32 = WM_USER + 0x41;

    thread_local! {
        static TAP: Cell<Tap> = Cell::new(Tap::default());
        static DISPATCH_THREAD_ID: Cell<u32> = const { Cell::new(0) };
    }

    pub struct Hook {
        hook_thread_id: u32,
        dispatch_thread_id: u32,
        hook_handle: Option<std::thread::JoinHandle<()>>,
        dispatch_handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Hook {
        pub fn install(on_tap: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
            // Thread de despacho recebe o evento via PostThreadMessageW para manter
            // o callback do hook de teclado 100% livre de alocações e bloqueios.
            let (dispatch_tx, dispatch_rx) = std::sync::mpsc::channel::<u32>();
            let dispatch_handle = std::thread::Builder::new()
                .name("pulse-win-key-dispatch".into())
                .spawn(move || unsafe {
                    let mut msg = MSG::default();
                    let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
                    let tid = GetCurrentThreadId();
                    let _ = dispatch_tx.send(tid);
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        if msg.message == WM_PULSE_WIN_TAP {
                            on_tap();
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

                    // Inicializa o estado com teclas já pressionadas ao ativar o hook.
                    let mut tap = Tap::default();
                    for vk in 8..256 {
                        if !matches!(vk, 0x10..=0x12) && GetAsyncKeyState(vk) < 0 {
                            tap.key(vk as u32, true, false);
                        }
                    }
                    tap.candidate = None;
                    TAP.with(|state| state.set(tap));
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

            match ready_rx.recv() {
                Ok(Ok(hook_thread_id)) => Ok(Self {
                    hook_thread_id,
                    dispatch_thread_id,
                    hook_handle: Some(hook_handle),
                    dispatch_handle: Some(dispatch_handle),
                }),
                Ok(Err(error)) => {
                    unsafe {
                        let _ =
                            PostThreadMessageW(dispatch_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                    }
                    let _ = hook_handle.join();
                    let _ = dispatch_handle.join();
                    Err(error)
                }
                Err(_) => {
                    unsafe {
                        let _ =
                            PostThreadMessageW(dispatch_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                    }
                    let _ = hook_handle.join();
                    let _ = dispatch_handle.join();
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
        if code == HC_ACTION as i32 {
            let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let message = wparam.0 as u32;
            if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) {
                let tap = TAP.with(|state| {
                    let mut tap = state.get();
                    let matched = tap.key(
                        event.vkCode,
                        matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN),
                        event.flags.contains(LLKHF_INJECTED),
                    );
                    state.set(tap);
                    matched
                });
                if tap {
                    // Enfileira máscara virtual 0xE8 (down+up) seguida do key-up injetado da
                    // tecla Windows e CONSOME o WM_KEYUP físico original (LRESULT(1)).
                    // Sem consumir o WM_KEYUP físico, o CallNextHookEx entregaria o Win-up ao
                    // StartMenuExperienceHost ANTES do 0xE8 entrar na fila, abrindo o Menu Iniciar.
                    let vk_win = VIRTUAL_KEY(event.vkCode as u16);
                    let make_key = |vk, flags| INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: vk,
                                dwFlags: flags,
                                ..Default::default()
                            },
                        },
                    };
                    let inputs = [
                        make_key(VIRTUAL_KEY(0xE8), Default::default()),
                        make_key(VIRTUAL_KEY(0xE8), KEYEVENTF_KEYUP),
                        make_key(vk_win, KEYEVENTF_KEYUP),
                    ];
                    if SendInput(&inputs, size_of::<INPUT>() as i32) == 3 {
                        DISPATCH_THREAD_ID.with(|slot| {
                            let tid = slot.get();
                            if tid != 0 {
                                let _ =
                                    PostThreadMessageW(tid, WM_PULSE_WIN_TAP, WPARAM(0), LPARAM(0));
                            }
                        });
                        return LRESULT(1);
                    }
                }
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32
            && matches!(
                wparam.0 as u32,
                WM_LBUTTONDOWN
                    | WM_RBUTTONDOWN
                    | WM_MBUTTONDOWN
                    | WM_XBUTTONDOWN
                    | WM_MOUSEWHEEL
                    | WM_MOUSEHWHEEL
            )
        {
            let event = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            if event.flags & LLMHF_INJECTED == 0 {
                TAP.with(|state| {
                    let mut tap = state.get();
                    tap.click();
                    state.set(tap);
                });
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
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

    fn sequence(events: &[(u32, bool, bool)], expected: &[usize]) {
        let mut tap = Tap::default();
        let taps: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(i, &(vk, down, injected))| tap.key(vk, down, injected).then_some(i))
            .collect();
        assert_eq!(taps, expected);
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
    fn win_auto_repeat_toggles_once() {
        sequence(
            &[
                (L, true, false),
                (L, true, false),
                (L, true, false),
                (L, false, false),
                (L, false, false),
            ],
            &[3],
        );
    }

    #[test]
    fn injected_events_do_not_start_cancel_or_complete_taps() {
        sequence(
            &[
                (L, true, true),
                (E, true, true),
                (E, false, true),
                (L, false, true),
            ],
            &[],
        );
        sequence(
            &[
                (L, true, false),
                (E, true, true),
                (E, false, true),
                (L, false, true),
                (L, false, false),
            ],
            &[4],
        );
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
    fn ignores_out_of_range_virtual_keys() {
        let mut tap = Tap::default();
        assert!(!tap.key(300, true, false));
        assert!(!tap.key(300, false, false));
        assert!(!tap.key(L, true, false));
        assert!(tap.key(L, false, false));
    }
}
