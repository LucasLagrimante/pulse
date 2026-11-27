//! Pulse Runner — launcher ultra-rápido acionado pela tecla Windows.
//!
//! Expõe o controle do hook de teclado (`win_key`), indexação em segundo plano
//! (`indexer`), calculadora instantânea (`calc`), posicionamento da janela
//! `"launcher"` e comandos Tauri para consulta e execução segura.

pub mod calc;
pub mod game_guard;
pub mod indexer;
pub mod win_key;
pub mod win_key_admin;

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

/// Diagnóstico temporário do conflito com o Iniciar: chaves lidas a cada toque
/// de `%LOCALAPPDATA%\Pulse\runner-diag.txt` (sem o arquivo, tudo ligado).
#[derive(Clone, Copy, Debug, Default)]
struct RunnerDiag {
    /// `sem-runner`: o toque é engolido e nada mais acontece.
    no_runner: bool,
    /// `sem-foco`: o runner aparece sem ser ativado.
    no_focus: bool,
    /// `sem-dock`: o toque não revela nem segura a Dock.
    no_dock: bool,
    /// `sem-anterior`: não guarda nem devolve o foco à janela anterior.
    no_previous: bool,
}

impl RunnerDiag {
    fn parse(text: &str) -> Self {
        let text = text.to_lowercase();
        Self {
            no_runner: text.contains("sem-runner"),
            no_focus: text.contains("sem-foco"),
            no_dock: text.contains("sem-dock"),
            no_previous: text.contains("sem-anterior"),
        }
    }

    fn load() -> Self {
        let path = crate::storage::paths::pulse_local_dir().join("runner-diag.txt");
        std::fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }
}

fn on_win_key_tap() {
    let diag = RunnerDiag::load();
    #[cfg(windows)]
    crate::custom_dock::shell_log::info("launcher_diag", format!("Toque Win com {diag:?}"));
    // Um toque na tecla Windows também revela a Dock (quando ela está em
    // auto-hide ou sobre um jogo em tela cheia).
    if !diag.no_dock {
        crate::custom_dock::reveal_dock_from_shortcut();
    }
    mark_win_tap();
    if diag.no_runner {
        return;
    }
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

/// Reinstala o hook (troca entre o processo comum e o elevado) se ele estiver ativo.
pub fn restart_win_key_hook() {
    {
        let _lock = WIN_KEY_RECONCILE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !win_key::is_enabled() {
            return;
        }
        let _ = win_key::set_enabled(false, || {});
    }
    reconcile_win_key_hook();
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
        dismiss_launcher_window(app);
        return;
    }

    let diag = RunnerDiag::load();
    #[cfg(windows)]
    if !diag.no_previous {
        focus::remember_previous();
    }

    // Enquanto o runner estiver aberto a Dock fica visível, mesmo sobre um
    // jogo em tela cheia; ao fechar ela volta ao auto-hide.
    if !diag.no_dock {
        crate::custom_dock::set_dock_hold(true);
    }
    position_launcher_centered(&window);
    let _ = window.unminimize();
    let _ = window.set_always_on_top(true);
    if diag.no_focus {
        show_launcher_without_activation(&window);
    } else {
        let _ = window.show();
        bring_launcher_to_front(&window);
    }
    let _ = window.emit("pulse://foreground-changed", true);
    let _ = window.emit(EVENT_LAUNCHER_OPENED, ());
}

/// Instante do último toque na tecla Windows engolido pelo runner e quantas
/// vezes o runner já retomou o foco do Iniciar depois dele.
static LAST_WIN_TAP: Mutex<Option<(Instant, u8)>> = Mutex::new(None);
/// Janela em que o Iniciar nativo ainda reage a um toque que o hook engoliu.
const START_STEAL_WINDOW: Duration = Duration::from_millis(1500);
/// Limite de retomadas por toque (evita disputa infinita com o shell).
const MAX_FOCUS_RETAKES: u8 = 2;

fn mark_win_tap() {
    if let Ok(mut slot) = LAST_WIN_TAP.lock() {
        *slot = Some((Instant::now(), 0));
    }
}

/// Se o toque recente ainda permite retomar o foco, consome uma retomada e
/// devolve há quanto tempo foi o toque.
#[cfg_attr(not(windows), allow(dead_code))]
fn claim_focus_retake() -> Option<Duration> {
    let mut slot = LAST_WIN_TAP.lock().ok()?;
    let (at, retakes) = slot.as_mut()?;
    let elapsed = at.elapsed();
    if elapsed > START_STEAL_WINDOW || *retakes >= MAX_FOCUS_RETAKES {
        return None;
    }
    *retakes += 1;
    Some(elapsed)
}

/// Pedido do sistema para abrir o Menu Iniciar (`SC_TASKLIST` na
/// `Shell_TrayWnd` do Pulse Shell). Com o runner ativo na tecla Windows, o
/// pedido é consumido: logo após um toque já tratado ele é descartado; sem
/// toque recente (janela elevada fora do alcance do hook, Ctrl+Esc) abre o
/// runner no lugar do Iniciar. Devolve `true` quando consumiu.
pub fn intercept_shell_start_request() -> bool {
    if !win_key::is_enabled() {
        // Runner desligado ou pausado por jogo protegido: Iniciar nativo.
        return false;
    }
    let recent_tap = LAST_WIN_TAP
        .lock()
        .ok()
        .and_then(|slot| slot.map(|(at, _)| at.elapsed()))
        .filter(|elapsed| *elapsed <= START_STEAL_WINDOW);
    #[cfg(windows)]
    crate::custom_dock::shell_log::info(
        "launcher_focus",
        match recent_tap {
            Some(elapsed) => format!(
                "SC_TASKLIST (abrir Iniciar) descartado {}ms após toque já tratado",
                elapsed.as_millis()
            ),
            None => "SC_TASKLIST (abrir Iniciar) sem toque visto pelo hook; runner abre no lugar"
                .to_string(),
        },
    );
    if recent_tap.is_none() {
        on_win_key_tap();
    }
    true
}

/// Diagnóstico `sem-foco`: mostra o runner sem ativá-lo (`SW_SHOWNOACTIVATE`).
fn show_launcher_without_activation(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        if let Ok(raw_hwnd) = window.hwnd() {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
            unsafe {
                let _ = ShowWindow(HWND(raw_hwnd.0), SW_SHOWNOACTIVATE);
            }
            return;
        }
    }
    let _ = window.show();
}

/// Traz a janela do runner para o primeiro plano com foco no teclado.
fn bring_launcher_to_front(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        if let Ok(raw_hwnd) = window.hwnd() {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, SetForegroundWindow};
            let hwnd = HWND(raw_hwnd.0);
            // Sem AllowSetForegroundWindow(ASFW_ANY): com o Pulse em primeiro
            // plano ele liberava o foco para QUALQUER processo, inclusive o
            // Iniciar (SearchHost) com ativação pendente.
            unsafe {
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }
    let _ = window.set_focus();
}

/// Fecha o runner pelo usuário (tecla Windows / Esc): devolve o foco à janela
/// que estava em primeiro plano antes de abrir. Sem isso o Windows escolhe a
/// próxima da Z-order, que muitas vezes é a Pesquisa/Iniciar (sempre carregada
/// e oculta) — o Iniciar "abria sozinho" sem nenhuma tecla.
pub fn dismiss_launcher_window(app: &AppHandle) {
    #[cfg(windows)]
    if !RunnerDiag::load().no_previous {
        focus::restore_previous();
    }
    hide_launcher_window(app);
}

/// Janela do runner perdeu o foco. O WebView2 às vezes emite `Focused(false)`
/// ao trocar o foco interno logo após abrir; se o foco continua no próprio
/// Pulse, o aviso é ignorado (fechar aí deixava o Windows ativar o Iniciar).
pub fn on_launcher_focus_lost(app: &AppHandle) {
    #[cfg(windows)]
    {
        if let Some(owner) = focus::foreground_owner_if_own() {
            crate::custom_dock::shell_log::info(
                "launcher_focus",
                format!("Perda de foco ignorada: foco segue no Pulse ({owner})"),
            );
            return;
        }
        // O Iniciar do Windows 11 (SearchHost) às vezes toma o foco ~0,25–0,6s
        // depois de um toque que o hook engoliu. Nunca se injeta tecla para
        // fechá-lo (um Esc simulado pode cair em outro app); o runner só tenta
        // retomar o foco.
        if focus::foreground_is_start_menu() {
            if let Some(elapsed) = claim_focus_retake() {
                crate::custom_dock::shell_log::info(
                    "launcher_focus",
                    format!(
                        "Iniciar tomou o foco {}ms após o toque; runner tenta retomar",
                        elapsed.as_millis()
                    ),
                );
                if let Some(window) = app.get_webview_window(LAUNCHER_WINDOW_LABEL) {
                    bring_launcher_to_front(&window);
                }
                return;
            }
        }
        crate::custom_dock::shell_log::info(
            "launcher_focus",
            format!(
                "Runner perdeu o foco para {}",
                focus::foreground_description()
            ),
        );
    }
    hide_launcher_window(app);
}

#[cfg(windows)]
mod focus {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
        IsWindowVisible, SetForegroundWindow,
    };

    /// Janela (de outro processo) em primeiro plano quando o runner abriu.
    static PREVIOUS: AtomicIsize = AtomicIsize::new(0);

    fn owner_pid(hwnd: HWND) -> u32 {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        pid
    }

    fn class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 128];
        let len = unsafe { GetClassNameW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..len.max(0) as usize])
    }

    pub fn remember_previous() {
        let foreground = unsafe { GetForegroundWindow() };
        let value = if foreground.is_invalid() || owner_pid(foreground) == std::process::id() {
            0
        } else {
            foreground.0 as isize
        };
        PREVIOUS.store(value, Ordering::Release);
    }

    /// Classe da janela em foco se ela pertence ao próprio Pulse.
    pub fn foreground_owner_if_own() -> Option<String> {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid() || owner_pid(foreground) != std::process::id() {
            return None;
        }
        Some(class_name(foreground))
    }

    /// A janela em foco é o Iniciar/Pesquisa nativos do Windows.
    pub fn foreground_is_start_menu() -> bool {
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const HOSTS: &[&str] = &[
            "startmenuexperiencehost.exe",
            "searchhost.exe",
            "searchapp.exe",
        ];
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid() {
            return false;
        }
        let pid = owner_pid(foreground);
        if pid == 0 {
            return false;
        }
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .is_ok();
            let _ = windows::Win32::Foundation::CloseHandle(process);
            if !ok {
                return false;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
            HOSTS.iter().any(|host| name.eq_ignore_ascii_case(host))
        }
    }

    pub fn foreground_description() -> String {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid() {
            return "nenhuma janela".to_string();
        }
        format!("pid {} ({})", owner_pid(foreground), class_name(foreground))
    }

    /// Devolve o foco à janela anterior, só se o runner ainda for o
    /// primeiro plano (se outro app já tomou o foco, não interfere).
    pub fn restore_previous() {
        let previous = HWND(PREVIOUS.swap(0, Ordering::AcqRel) as *mut _);
        if previous.is_invalid() || foreground_owner_if_own().is_none() {
            return;
        }
        unsafe {
            if IsWindow(Some(previous)).as_bool()
                && IsWindowVisible(previous).as_bool()
                && !IsIconic(previous).as_bool()
            {
                let _ = SetForegroundWindow(previous);
            }
        }
    }
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
    dismiss_launcher_window(&app);
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
        // Pulse elevado: o app abre pelo Explorer, sem herdar o administrador.
        if crate::shell_launch::try_open_unelevated(target, None) {
            return Ok(());
        }
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
