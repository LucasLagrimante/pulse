//! Ações nativas de energia do menu Pulse da Barra Superior.

use std::os::windows::process::CommandExt;
use windows::{
    core::w,
    Win32::{
        Foundation::HWND,
        System::{Power::SetSuspendState, Shutdown::LockWorkStation},
        UI::WindowsAndMessaging::{
            MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_ICONWARNING, MB_YESNO,
        },
    },
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn lock_workstation() {
    unsafe {
        let _ = LockWorkStation();
    }
}

pub fn sleep_system() {
    unsafe {
        let _ = SetSuspendState(false, false, false);
    }
}

pub fn restart_with_confirm(parent: HWND) {
    let result = unsafe {
        MessageBoxW(
            Some(parent),
            w!("Deseja reiniciar o computador agora? Aplicativos com alterações não salvas podem solicitar confirmação."),
            w!("Reiniciar Windows — Pulse Shell"),
            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
        )
    };
    if result == IDYES {
        spawn_shutdown(&["/r", "/t", "0"]);
    }
}

pub fn shutdown_with_confirm(parent: HWND) {
    let result = unsafe {
        MessageBoxW(
            Some(parent),
            w!("Deseja desligar o computador agora? Aplicativos com alterações não salvas podem solicitar confirmação."),
            w!("Desligar Windows — Pulse Shell"),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    if result == IDYES {
        spawn_shutdown(&["/s", "/t", "0"]);
    }
}

fn spawn_shutdown(args: &[&str]) {
    let _ = std::process::Command::new("shutdown.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
