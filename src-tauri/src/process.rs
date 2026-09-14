//! Processo nativo sem janelas de console no Windows.
//!
//! Toda chamada a um executável externo do Pulse deve passar por este helper.
//! Isso evita que consultas em segundo plano (especialmente durante a abertura)
//! criem janelas de terminal visíveis ao usuário.

use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}
