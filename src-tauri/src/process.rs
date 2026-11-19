//! Processo nativo sem janelas de console no Windows.
//!
//! Toda chamada a um executável externo do Pulse deve passar por este helper.
//! Isso evita que consultas em segundo plano (especialmente durante a abertura)
//! criem janelas de terminal visíveis ao usuário. Fora do Windows não existe
//! janela de console a esconder: o helper devolve um `Command` simples.

use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }

    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// `hidden_command` com `LC_ALL=C`/`LANG=C`: use quando a saída da CLI for
/// interpretada pelo Pulse, para não depender do idioma do sistema.
#[cfg(unix)]
pub fn c_locale_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = hidden_command(program);
    command.env("LC_ALL", "C").env("LANG", "C");
    command
}
