// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
fn acquire_instance() -> Result<pulse_windows_lib::single_instance::windows::InstanceGuard, ()> {
    use pulse_windows_lib::single_instance::windows::InstanceGuard;
    const NAME: &str = "Local\\Pulse_Windows_Single_Instance";
    // Reinício pedido em "Reiniciar como administrador": a instância anterior
    // ainda está fechando e precisa de alguns segundos para soltar o mutex.
    let relaunched =
        std::env::args().any(|arg| arg == pulse_windows_lib::privileges::ELEVATED_RELAUNCH_ARG);
    let result = if relaunched {
        InstanceGuard::acquire_after_relaunch(NAME, std::time::Duration::from_secs(15))
    } else {
        InstanceGuard::acquire(NAME)
    };
    result.map_err(|_| ())
}

fn main() {
    #[cfg(windows)]
    let Ok(_instance_guard) = acquire_instance() else {
        return;
    };

    pulse_windows_lib::run()
}
