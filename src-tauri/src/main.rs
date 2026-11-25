// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
fn acquire_instance() -> Result<pulse_windows_lib::single_instance::windows::InstanceGuard, ()> {
    use pulse_windows_lib::single_instance::windows::InstanceGuard;
    const NAME: &str = "Local\\Pulse_Windows_Single_Instance";
    // Reinício pedido em "Reiniciar como administrador" ou após auto-atualização:
    // a instância anterior ainda está fechando e precisa de alguns segundos para soltar o mutex.
    let relaunched = std::env::args().any(|arg| {
        arg == pulse_windows_lib::privileges::ELEVATED_RELAUNCH_ARG
            || arg == pulse_windows_lib::self_update::UPDATED_RELAUNCH_ARG
    });
    let result = if relaunched {
        InstanceGuard::acquire_after_relaunch(NAME, std::time::Duration::from_secs(15))
    } else {
        InstanceGuard::acquire(NAME)
    };
    result.map_err(|_| ())
}

/// Linux/macOS: `flock` + socket Unix. Outra instância viva recebe o pedido de
/// foco e esta sai; falha ao criar a trava (pasta sem permissão etc.) não
/// impede o Pulse de abrir. `--elevated-relaunch` não existe fora do Windows
/// (não há reinício elevado), então só o reinício pós-atualização espera.
#[cfg(unix)]
fn acquire_instance() -> Result<Option<pulse_windows_lib::single_instance::unix::InstanceGuard>, ()>
{
    use pulse_windows_lib::single_instance::unix::{AcquireError, InstanceGuard};
    const NAME: &str = "pulse-single-instance";
    let relaunched =
        std::env::args().any(|arg| arg == pulse_windows_lib::self_update::UPDATED_RELAUNCH_ARG);
    let result = if relaunched {
        InstanceGuard::acquire_after_relaunch(NAME, std::time::Duration::from_secs(15))
    } else {
        InstanceGuard::acquire(NAME)
    };
    match result {
        Ok(guard) => Ok(Some(guard)),
        Err(AcquireError::AlreadyRunning) => Err(()),
        Err(AcquireError::CreationFailed) => {
            eprintln!(
                "Pulse: não foi possível criar a trava de instância única; seguindo sem ela."
            );
            Ok(None)
        }
    }
}

fn main() {
    // Modo sentinela da Dock Beta: não adquire instância única nem abre GUI;
    // apenas aguarda o término do processo principal para restaurar a taskbar.
    if pulse_windows_lib::custom_dock::run_watchdog_if_requested() {
        return;
    }

    #[cfg(any(windows, unix))]
    let Ok(_instance_guard) = acquire_instance() else {
        return;
    };

    pulse_windows_lib::run()
}
