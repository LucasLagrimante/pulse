// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    let _instance_guard = match pulse_windows_lib::single_instance::windows::InstanceGuard::acquire(
        "Local\\Pulse_Windows_Single_Instance",
    ) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    pulse_windows_lib::run()
}
