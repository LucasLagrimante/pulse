//! Pulse Runner — launcher ultra-rápido acionado pela tecla Windows.
//!
//! Expõe o controle do hook de teclado (`win_key`), indexação em segundo plano
//! (`indexer`), calculadora instantânea (`calc`), posicionamento da janela
//! `"launcher"` e comandos Tauri para consulta e execução segura.

pub mod calc;
pub mod game_guard;
pub mod indexer;
pub mod win_key;

use indexer::{LauncherActionResult, LauncherResponse};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};

const LAUNCHER_WINDOW_LABEL: &str = "launcher";
const EVENT_LAUNCHER_OPENED: &str = "pulse://launcher-opened";
const DEFAULT_LAUNCHER_WIDTH: f64 = 680.0;
const DEFAULT_LAUNCHER_HEIGHT: f64 = 520.0;

static APP_HANDLE: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();

fn app_handle_slot() -> &'static Mutex<Option<AppHandle>> {
    APP_HANDLE.get_or_init(|| Mutex::new(None))
}

fn clone_app_handle() -> Option<AppHandle> {
    app_handle_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned())
}

/// Inicializa o módulo do Pulse Runner, guarda o `AppHandle` e aquece o índice
/// de aplicativos em segundo plano de forma escalonada (sem travar o boot).
pub fn init(app: AppHandle) {
    if let Ok(mut slot) = app_handle_slot().lock() {
        *slot = Some(app);
    }

    tauri::async_runtime::spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(450));
        let _ = indexer::rebuild_app_index();
    });
}

/// O usuário quer o runner na tecla Windows (preferência, antes da pausa por jogo).
static WIN_KEY_WANTED: AtomicBool = AtomicBool::new(false);
/// Serializa as reconciliações (preferência × jogo protegido).
static WIN_KEY_RECONCILE: Mutex<()> = Mutex::new(());

fn win_key_wanted() -> bool {
    WIN_KEY_WANTED.load(Ordering::Acquire)
}

fn on_win_key_tap() {
    // Um toque na tecla Windows também revela a Dock (quando ela está em
    // auto-hide ou sobre um jogo em tela cheia).
    crate::custom_dock::reveal_dock_from_shortcut();
    if let Some(app) = clone_app_handle() {
        tauri::async_runtime::spawn(async move {
            toggle_launcher_window(&app);
        });
    }
}

/// Liga ou desliga o runner na tecla Windows conforme a preferência do usuário.
/// Enquanto um jogo protegido por anti-cheat estiver aberto (`game_guard`), o
/// hook fica desinstalado e a tecla volta a abrir o Iniciar nativo.
pub fn apply_win_key_hook(enabled: bool) {
    WIN_KEY_WANTED.store(enabled, Ordering::Release);
    if enabled {
        game_guard::start(win_key_wanted, reconcile_win_key_hook);
    }
    reconcile_win_key_hook();
}

/// Aplica o estado efetivo do hook: preferência ligada e nenhum jogo protegido aberto.
fn reconcile_win_key_hook() {
    let _lock = WIN_KEY_RECONCILE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let paused_by = game_guard::active();
    let enabled = win_key_wanted() && paused_by.is_none();
    if win_key::is_enabled() == enabled {
        return;
    }
    let started = Instant::now();
    let result = win_key::set_enabled(enabled, on_win_key_tap);

    if result.is_ok() && win_key_wanted() {
        let message = match paused_by {
            Some(process) => {
                format!("Runner na tecla Windows pausado: jogo protegido aberto ({process})")
            }
            None => "Runner na tecla Windows retomado".to_string(),
        };
        #[cfg(windows)]
        crate::custom_dock::shell_log::info("launcher_game_guard", message);
        #[cfg(not(windows))]
        let _ = message;
    }

    if let Err(error) = result {
        crate::debug::record_failure(
            "launcher_win_key_hook",
            started.elapsed().as_millis(),
            crate::debug::Failure::new("Não foi possível atualizar o atalho da tecla Windows")
                .stage("set_enabled")
                .detail(error),
        );
    }
}

/// Encerra o hook da tecla Windows com segurança ao fechar o aplicativo.
pub fn shutdown() {
    let _ = win_key::set_enabled(false, || {});
}

/// Alterna a visibilidade da janela `"launcher"` centralizada no monitor ativo.
pub fn toggle_launcher_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LAUNCHER_WINDOW_LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide_launcher_window(app);
        return;
    }

    // Enquanto o runner estiver aberto a Dock fica visível, mesmo sobre um
    // jogo em tela cheia; ao fechar ela volta ao auto-hide.
    crate::custom_dock::set_dock_hold(true);
    position_launcher_centered(&window);
    let _ = window.unminimize();
    let _ = window.set_always_on_top(true);
    let _ = window.show();
    #[cfg(windows)]
    {
        if let Ok(raw_hwnd) = window.hwnd() {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{
                AllowSetForegroundWindow, BringWindowToTop, SetForegroundWindow, ASFW_ANY,
            };
            let hwnd = HWND(raw_hwnd.0);
            unsafe {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }
    let _ = window.set_focus();
    let _ = window.emit("pulse://foreground-changed", true);
    let _ = window.emit(EVENT_LAUNCHER_OPENED, ());
}

/// Oculta a janela `"launcher"` e notifica perda de foco.
pub fn hide_launcher_window(app: &AppHandle) {
    crate::custom_dock::set_dock_hold(false);
    if let Some(window) = app.get_webview_window(LAUNCHER_WINDOW_LABEL) {
        let _ = window.emit("pulse://foreground-changed", false);
        let _ = window.hide();
    }
}

fn position_launcher_centered(window: &WebviewWindow) {
    let monitor = window
        .cursor_position()
        .ok()
        .and_then(|pos| window.monitor_from_point(pos.x, pos.y).ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());

    let Some(monitor) = monitor else {
        let _ = window.center();
        return;
    };

    let scale = monitor.scale_factor();
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let work_area = monitor.work_area();
    let (win_w, win_h) = window
        .outer_size()
        .map(|sz| (f64::from(sz.width), f64::from(sz.height)))
        .unwrap_or((
            DEFAULT_LAUNCHER_WIDTH * scale,
            DEFAULT_LAUNCHER_HEIGHT * scale,
        ));

    let area_x = f64::from(work_area.position.x);
    let area_y = f64::from(work_area.position.y);
    let area_w = f64::from(work_area.size.width);
    let area_h = f64::from(work_area.size.height);

    // Centraliza horizontalmente e posiciona no terço superior (~18% da altura útil).
    let x = area_x + ((area_w - win_w) / 2.0).max(0.0);
    let preferred_y = area_y + (area_h * 0.18);
    let max_y = (area_y + area_h - win_h).max(area_y);
    let y = preferred_y.clamp(area_y, max_y);

    let _ = window.set_position(tauri::Position::Physical(PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    )));
}

#[tauri::command]
pub async fn launcher_query(query: String, category: Option<String>) -> LauncherResponse {
    tauri::async_runtime::spawn_blocking(move || indexer::query_sync(&query, category.as_deref()))
        .await
        .unwrap_or_else(|_| LauncherResponse {
            query: String::new(),
            category: "all".to_owned(),
            calc: None,
            recent: Vec::new(),
            items: Vec::new(),
            indexed_app_count: indexer::indexed_app_count(),
            is_indexing: indexer::is_indexing(),
            search_elapsed_ms: 0,
        })
}

#[tauri::command]
pub async fn launcher_execute(
    app: AppHandle,
    item_id: String,
    kind: String,
    target: String,
    title: String,
) -> Result<LauncherActionResult, String> {
    let started = Instant::now();

    if kind == "calc" {
        let value = target.clone();
        let copy_res = tauri::async_runtime::spawn_blocking(move || copy_to_clipboard(&value))
            .await
            .map_err(|e| format!("Falha ao copiar resultado: {e}"))?;
        if let Err(err) = copy_res {
            crate::debug::record_failure(
                "launcher_execute",
                started.elapsed().as_millis(),
                crate::debug::Failure::new(
                    "Não foi possível copiar o cálculo na área de transferência nativa",
                )
                .stage("clipboard")
                .detail(err),
            );
        }
        return Ok(LauncherActionResult {
            ok: true,
            action: "copied".to_owned(),
            message: format!("Resultado copiado: {target}"),
            close_launcher: false,
        });
    }

    if kind == "module" {
        indexer::record_usage(&item_id, &kind, &title, &target);
        hide_launcher_window(&app);
        let res = if target == "Arquivos" {
            crate::tray::show_files_window(app, None)
        } else {
            crate::tray::show_main_module(app, target.clone())
        };
        crate::debug::record_result(
            "launcher_execute",
            started.elapsed().as_millis(),
            &res,
            "Não foi possível abrir o módulo do Pulse",
        );
        res?;
        return Ok(LauncherActionResult {
            ok: true,
            action: "navigated".to_owned(),
            message: format!("Módulo aberto: {title}"),
            close_launcher: true,
        });
    }

    if kind == "system" {
        indexer::record_usage(&item_id, &kind, &title, &target);
        hide_launcher_window(&app);
        let action_target = target.clone();
        let res =
            tauri::async_runtime::spawn_blocking(move || execute_system_action(&action_target))
                .await
                .map_err(|e| format!("Falha ao executar ação do sistema: {e}"))?;
        crate::debug::record_result(
            "launcher_execute",
            started.elapsed().as_millis(),
            &res,
            "Não foi possível executar a ação do sistema",
        );
        res?;
        return Ok(LauncherActionResult {
            ok: true,
            action: "executed".to_owned(),
            message: format!("Ação executada: {title}"),
            close_launcher: true,
        });
    }

    // Aplicativos, arquivos e pastas
    indexer::record_usage(&item_id, &kind, &title, &target);
    hide_launcher_window(&app);

    let launch_target = target.clone();
    let launch_kind = kind.clone();
    let res =
        tauri::async_runtime::spawn_blocking(move || launch_item(&launch_kind, &launch_target))
            .await
            .map_err(|e| format!("Falha na tarefa de abertura do item: {e}"))?;

    crate::debug::record_result(
        "launcher_execute",
        started.elapsed().as_millis(),
        &res,
        "Não foi possível abrir o item selecionado",
    );
    res?;

    Ok(LauncherActionResult {
        ok: true,
        action: "launched".to_owned(),
        message: format!("Aberto: {title}"),
        close_launcher: true,
    })
}

#[tauri::command]
pub async fn launcher_refresh_index() -> Result<usize, String> {
    let started = Instant::now();
    let res = tauri::async_runtime::spawn_blocking(indexer::rebuild_app_index)
        .await
        .map_err(|e| format!("Falha ao reindexar aplicativos: {e}"));
    crate::debug::record_result(
        "launcher_refresh_index",
        started.elapsed().as_millis(),
        &res,
        "Não foi possível atualizar o índice do Pulse Runner",
    );
    res
}

#[tauri::command]
pub fn launcher_hide(app: AppHandle) {
    hide_launcher_window(&app);
}

#[tauri::command]
pub fn launcher_toggle(app: AppHandle) {
    toggle_launcher_window(&app);
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_exec::set_clipboard_text(text)
    }
    #[cfg(not(windows))]
    {
        // Em Linux/macOS o frontend também garante cópia via `navigator.clipboard`.
        let _ = text;
        Ok(())
    }
}

fn launch_item(kind: &str, target: &str) -> Result<(), String> {
    if target.trim().is_empty() {
        return Err("Alvo de abertura vazio.".to_owned());
    }

    #[cfg(windows)]
    {
        let _ = kind;
        windows_exec::open_target(target)
    }

    #[cfg(target_os = "linux")]
    {
        if kind == "app" {
            let mut parts = split_command_line(target);
            if parts.is_empty() {
                return Err("Comando de aplicativo inválido.".to_owned());
            }
            let program = parts.remove(0);
            std::process::Command::new(&program)
                .args(&parts)
                .spawn()
                .map_err(|e| format!("Falha ao iniciar '{program}': {e}"))?;
            Ok(())
        } else {
            std::process::Command::new("xdg-open")
                .arg(target)
                .spawn()
                .map_err(|e| format!("Falha ao abrir '{target}': {e}"))?;
            Ok(())
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        if kind == "app" {
            cmd.arg("-a").arg(target);
        } else {
            cmd.arg(target);
        }
        cmd.spawn()
            .map_err(|e| format!("Falha ao abrir '{target}': {e}"))?;
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (kind, target);
        Err("Abertura de itens não suportada nesta plataforma.".to_owned())
    }
}

fn execute_system_action(action: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_exec::execute_system_action(action)
    }

    #[cfg(target_os = "linux")]
    {
        let try_any = |candidates: &[(&str, &[&str])]| -> Result<(), String> {
            let mut last_err = String::new();
            for (cmd, args) in candidates {
                match std::process::Command::new(cmd).args(*args).spawn() {
                    Ok(_) => return Ok(()),
                    Err(e) => last_err = format!("{cmd}: {e}"),
                }
            }
            Err(format!("Nenhum comando disponível ({last_err})."))
        };

        match action {
            "system:lock" => try_any(&[
                ("loginctl", &["lock-session"]),
                ("xdg-screensaver", &["lock"]),
            ]),
            "system:sleep" => try_any(&[("systemctl", &["suspend"])]),
            "system:restart" => Err(
                "Para segurança, use o menu de energia do ambiente gráfico no Linux para reiniciar."
                    .to_owned(),
            ),
            "system:shutdown" => Err(
                "Para segurança, use o menu de energia do ambiente gráfico no Linux para desligar."
                    .to_owned(),
            ),
            "system:settings" => try_any(&[
                ("gnome-control-center", &[]),
                ("systemsettings", &[]),
                ("xfce4-settings-manager", &[]),
            ]),
            "system:terminal" => try_any(&[
                ("x-terminal-emulator", &[]),
                ("gnome-terminal", &[]),
                ("konsole", &[]),
                ("alacritty", &[]),
                ("kitty", &[]),
            ]),
            "system:taskmgr" => try_any(&[
                ("gnome-system-monitor", &[]),
                ("plasma-systemmonitor", &[]),
                ("xfce4-taskmanager", &[]),
            ]),
            other => Err(format!("Ação de sistema desconhecida: {other}")),
        }
    }

    #[cfg(target_os = "macos")]
    {
        let run = |cmd: &str, args: &[&str]| -> Result<(), String> {
            std::process::Command::new(cmd)
                .args(args)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("Falha ao executar {cmd}: {e}"))
        };
        match action {
            "system:lock" => run("pmset", &["displaysleepnow"]),
            "system:sleep" => run("pmset", &["sleepnow"]),
            "system:restart" | "system:shutdown" => Err(
                "Para segurança, confirme reinicialização ou desligamento pelo menu Apple."
                    .to_owned(),
            ),
            "system:settings" => run("open", &["x-apple.systempreferences:"]),
            "system:terminal" => run("open", &["-a", "Terminal"]),
            "system:taskmgr" => run("open", &["-a", "Activity Monitor"]),
            other => Err(format!("Ação de sistema desconhecida: {other}")),
        }
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = action;
        Err("Ações de sistema não suportadas nesta plataforma.".to_owned())
    }
}

#[cfg(target_os = "linux")]
fn split_command_line(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in input.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(windows)]
mod windows_exec {
    use std::mem::size_of;
    use std::os::windows::process::CommandExt;
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::LockWorkStation;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        SHParseDisplayName, ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST,
        SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_ICONWARNING, MB_YESNO, SW_SHOWNORMAL,
    };

    const CF_UNICODETEXT: u32 = 13;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn open_target(target: &str) -> Result<(), String> {
        if target.starts_with("shell:AppsFolder\\") {
            return open_shell_apps_folder_item(target);
        }
        shell_execute(target, None)
    }

    fn open_shell_apps_folder_item(parsing_name: &str) -> Result<(), String> {
        let com_init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let result = (|| -> Result<(), String> {
            let name = HSTRING::from(parsing_name);
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            unsafe { SHParseDisplayName(&name, None, &mut pidl, 0, None) }
                .map_err(|e| format!("Falha ao resolver aplicativo do shell: {e}"))?;
            let mut info = SHELLEXECUTEINFOW {
                cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_FLAG_NO_UI,
                lpIDList: pidl.cast(),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let exec_res = unsafe { ShellExecuteExW(&mut info) }
                .map_err(|e| format!("Falha ao abrir aplicativo: {e}"));
            unsafe { CoTaskMemFree(Some(pidl.cast())) };
            exec_res
        })();
        if com_init {
            unsafe { CoUninitialize() };
        }
        result
    }

    fn shell_execute(file: &str, params: Option<&str>) -> Result<(), String> {
        let file_h = HSTRING::from(file);
        let params_h = params.map(HSTRING::from);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file_h.as_ptr()),
            lpParameters: params_h
                .as_ref()
                .map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }.map_err(|e| format!("Falha ao abrir '{file}': {e}"))
    }

    pub fn set_clipboard_text(text: &str) -> Result<(), String> {
        let utf16: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe {
            OpenClipboard(None).map_err(|e| format!("OpenClipboard falhou: {e}"))?;
            let outcome = (|| -> Result<(), String> {
                EmptyClipboard().map_err(|e| format!("EmptyClipboard falhou: {e}"))?;
                let mem = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * 2)
                    .map_err(|e| format!("GlobalAlloc falhou: {e}"))?;
                let ptr = GlobalLock(mem) as *mut u16;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(mem));
                    return Err("GlobalLock retornou ponteiro nulo.".to_owned());
                }
                std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
                let _ = GlobalUnlock(mem);
                if let Err(err) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))) {
                    let _ = GlobalFree(Some(mem));
                    return Err(format!("SetClipboardData falhou: {err}"));
                }
                Ok(())
            })();
            let _ = CloseClipboard();
            outcome
        }
    }

    pub fn execute_system_action(action: &str) -> Result<(), String> {
        match action {
            "system:lock" => unsafe {
                LockWorkStation().map_err(|e| format!("Não foi possível bloquear a tela: {e}"))
            },
            "system:sleep" => {
                let ok = unsafe { SetSuspendState(false, false, false) };
                if ok {
                    Ok(())
                } else {
                    Err("O sistema recusou a solicitação de suspensão.".to_owned())
                }
            }
            "system:restart" => {
                let answer = unsafe {
                    MessageBoxW(
                        None,
                        w!("Deseja reiniciar o computador agora? Aplicativos com alterações não salvas podem solicitar confirmação."),
                        w!("Reiniciar Windows — Pulse Runner"),
                        MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
                    )
                };
                if answer == IDYES {
                    spawn_shutdown(&["/r", "/t", "0"])?;
                }
                Ok(())
            }
            "system:shutdown" => {
                let answer = unsafe {
                    MessageBoxW(
                        None,
                        w!("Deseja desligar o computador agora? Aplicativos com alterações não salvas podem solicitar confirmação."),
                        w!("Desligar Windows — Pulse Runner"),
                        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                    )
                };
                if answer == IDYES {
                    spawn_shutdown(&["/s", "/t", "0"])?;
                }
                Ok(())
            }
            "system:settings" => shell_execute("ms-settings:", None),
            "system:terminal" => shell_execute("wt.exe", None)
                .or_else(|_| shell_execute("powershell.exe", None))
                .or_else(|_| shell_execute("cmd.exe", None)),
            "system:taskmgr" => shell_execute("taskmgr.exe", None),
            other => Err(format!("Ação de sistema desconhecida: {other}")),
        }
    }

    fn spawn_shutdown(args: &[&str]) -> Result<(), String> {
        std::process::Command::new("shutdown.exe")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Falha ao acionar shutdown.exe: {e}"))
    }
}
