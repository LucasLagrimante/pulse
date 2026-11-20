//! Abrir arquivos e mostrá-los no gerenciador de arquivos fora do Windows.
//!
//! - Linux: `xdg-open` abre no aplicativo padrão; "mostrar na pasta" usa a
//!   interface D-Bus `org.freedesktop.FileManager1.ShowItems` (Dolphin, Nautilus,
//!   Nemo, Thunar...) e cai para `xdg-open` na pasta-mãe se ela não responder.
//! - macOS: `open` e `open -R` (TODO(macos): validar em hardware).
//!
//! Sempre argv fixo: o caminho vai como argumento, nunca dentro de string de shell.

use std::path::Path;
use std::process::{Command, Stdio};

/// Executa sem herdar o terminal e recolhe o processo filho numa thread, para
/// não deixar processo zumbi enquanto o Pulse estiver aberto.
fn spawn_detached(mut command: Command) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let _ = std::thread::Builder::new()
        .name("pulse-opener-wait".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

/// Abre no aplicativo padrão do sistema (pasta abre no gerenciador de arquivos).
pub(crate) fn open_with_default_app(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new("xdg-open");
    command.arg(path);
    spawn_detached(command).map_err(|error| format!("Não foi possível abrir o item: {error}"))
}

/// Mostra o item selecionado no gerenciador de arquivos.
#[cfg(target_os = "macos")]
pub(crate) fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    let mut command = Command::new("open");
    command.arg("-R").arg(path);
    spawn_detached(command).map_err(|error| format!("Não foi possível abrir o Finder: {error}"))
}

/// Mostra o item selecionado no gerenciador de arquivos. A chamada D-Bus pode
/// demorar enquanto o gerenciador inicia, então roda numa thread própria: o
/// comando Tauri síncrono não pode travar a thread principal.
#[cfg(not(target_os = "macos"))]
pub(crate) fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    let target = path.to_path_buf();
    let parent = path.parent().unwrap_or(path).to_path_buf();
    std::thread::Builder::new()
        .name("pulse-reveal".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            if show_items_via_dbus(&target) {
                return;
            }
            let mut fallback = Command::new("xdg-open");
            fallback.arg(&parent);
            let outcome = spawn_detached(fallback);
            crate::debug::record(
                "reveal_in_file_manager",
                started.elapsed().as_millis(),
                outcome.as_ref().map(|_| ()).map_err(|error| error.as_str()),
            );
        })
        .map(|_| ())
        .map_err(|error| format!("Não foi possível abrir o gerenciador de arquivos: {error}"))
}

#[cfg(not(target_os = "macos"))]
fn show_items_via_dbus(path: &Path) -> bool {
    Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--reply-timeout=5000",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(path)))
        .arg("string:")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// `file://` com percent-encoding de tudo que não for não reservado ou `/`.
/// A vírgula também é codificada: o `dbus-send` a usa como separador de array.
pub(crate) fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::file_uri;
    use std::path::Path;

    #[test]
    fn file_uri_encodes_spaces_commas_and_accents() {
        assert_eq!(
            file_uri(Path::new("/home/u/Área de trabalho/a,b.pdf")),
            "file:///home/u/%C3%81rea%20de%20trabalho/a%2Cb.pdf"
        );
        assert_eq!(file_uri(Path::new("/tmp/x_y-z.~1")), "file:///tmp/x_y-z.~1");
    }
}
