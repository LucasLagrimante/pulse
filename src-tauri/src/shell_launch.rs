//! Abre apps sem herdar o privilégio do Pulse.
//!
//! Um processo herda o token de quem o cria. Com o Pulse elevado (por exemplo,
//! via "Reiniciar como administrador"), tudo o que a Dock ou o Runner abriam
//! nascia como administrador. Um VS Code elevado faz o Windows (UIPI) deixar de
//! entregar as teclas aos apps comuns, e o atalho de outros programas (ex.: o
//! Ctrl+Win+Espaço do Handy) para de funcionar dentro dele.
//!
//! Com o Pulse elevado, o pedido é entregue ao Explorer (a área de trabalho,
//! `IShellDispatch2::ShellExecute`), que abre o app com o token comum do
//! usuário. Com o Pulse comum, nada muda.

/// Abre `target` (caminho, URL ou `shell:AppsFolder\...`) pelo Explorer quando o
/// Pulse está elevado. Devolve `Ok(true)` se o Explorer abriu, `Ok(false)` se o
/// Pulse não está elevado (o chamador abre normalmente) e `Err` se a entrega ao
/// Explorer falhou — nesse caso o chamador **não** deve abrir elevado por conta
/// própria sem registrar o motivo.
#[cfg(windows)]
pub fn open_unelevated(target: &str, params: Option<&str>) -> Result<bool, String> {
    if !crate::privileges::is_elevated_cached() {
        return Ok(false);
    }
    platform::shell_execute_via_explorer(target, params).map(|()| true)
}

/// Tenta abrir sem elevação. `true` = o Explorer abriu e o chamador não deve
/// abrir de novo; `false` = o chamador abre como sempre (Pulse comum, ou falha
/// da entrega ao Explorer, registrada na auditoria).
#[cfg(windows)]
pub fn try_open_unelevated(target: &str, params: Option<&str>) -> bool {
    let started = std::time::Instant::now();
    match open_unelevated(target, params) {
        Ok(opened) => opened,
        Err(error) => {
            crate::debug::record_failure(
                "shell_launch_unelevated",
                started.elapsed().as_millis(),
                crate::debug::Failure::new(
                    "Não foi possível abrir pelo Explorer sem privilégio; abrindo pelo Pulse",
                )
                .stage("explorer")
                .detail(error),
            );
            false
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows::core::{Interface, BSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, IServiceProvider,
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{
        IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellWindows, SID_STopLevelBrowser,
        ShellWindows, CSIDL_DESKTOP, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    pub fn shell_execute_via_explorer(target: &str, params: Option<&str>) -> Result<(), String> {
        // Thread própria: COM STA isolado, sem interferir no apartamento de quem chama.
        let target = target.to_owned();
        let params = params.map(str::to_owned);
        std::thread::spawn(move || unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let result = execute(&target, params.as_deref());
            if initialized {
                CoUninitialize();
            }
            result
        })
        .join()
        .map_err(|_| "A thread que abre pelo Explorer encerrou inesperadamente.".to_string())?
    }

    unsafe fn execute(target: &str, params: Option<&str>) -> Result<(), String> {
        let step = |stage: &str, error: windows::core::Error| format!("{stage}: {error}");
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)
            .map_err(|e| step("ShellWindows", e))?;
        let location = VARIANT::from(CSIDL_DESKTOP as i32);
        let empty = VARIANT::default();
        let mut hwnd = 0i32;
        let desktop: IDispatch = windows
            .FindWindowSW(&location, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
            .map_err(|e| step("FindWindowSW (Explorer em execução?)", e))?;
        let provider: IServiceProvider = desktop.cast().map_err(|e| step("IServiceProvider", e))?;
        let browser: IShellBrowser = provider
            .QueryService(&SID_STopLevelBrowser)
            .map_err(|e| step("IShellBrowser", e))?;
        let view = browser
            .QueryActiveShellView()
            .map_err(|e| step("QueryActiveShellView", e))?;
        let background: IDispatch = view
            .GetItemObject(SVGIO_BACKGROUND)
            .map_err(|e| step("GetItemObject", e))?;
        let folder_view: IShellFolderViewDual = background
            .cast()
            .map_err(|e| step("IShellFolderViewDual", e))?;
        let shell: IShellDispatch2 = folder_view
            .Application()
            .and_then(|app| app.cast())
            .map_err(|e| step("IShellDispatch2", e))?;

        let args = params.map(VARIANT::from).unwrap_or_default();
        let operation = VARIANT::from("open");
        let show = VARIANT::from(SW_SHOWNORMAL.0);
        shell
            .ShellExecute(&BSTR::from(target), &args, &empty, &operation, &show)
            .map_err(|e| step("ShellExecute", e))
    }
}
