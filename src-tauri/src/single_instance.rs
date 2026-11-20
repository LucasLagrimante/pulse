#[cfg(windows)]
pub mod windows {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    type Handle = *mut std::ffi::c_void;
    type Hwnd = *mut std::ffi::c_void;
    type Bool = i32;
    type Dword = u32;
    type Lpcwstr = *const u16;

    const ERROR_ALREADY_EXISTS: Dword = 183;
    const SW_RESTORE: i32 = 9;

    extern "system" {
        fn CreateMutexW(
            lp_mutex_attributes: *mut std::ffi::c_void,
            b_initial_owner: Bool,
            lp_name: Lpcwstr,
        ) -> Handle;
        fn GetLastError() -> Dword;
        fn CloseHandle(h_object: Handle) -> Bool;
        fn FindWindowW(lp_class_name: Lpcwstr, lp_window_name: Lpcwstr) -> Hwnd;
        fn ShowWindow(h_wnd: Hwnd, n_cmd_show: i32) -> Bool;
        fn SetForegroundWindow(h_wnd: Hwnd) -> Bool;
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AcquireError {
        CreationFailed,
        AlreadyRunning,
    }

    pub struct InstanceGuard {
        handle: Handle,
    }

    impl InstanceGuard {
        pub fn acquire(name: &str) -> Result<Self, AcquireError> {
            let wide_name: Vec<u16> = OsStr::new(name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 1, wide_name.as_ptr()) };
            if handle.is_null() {
                return Err(AcquireError::CreationFailed);
            }
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    let title: Vec<u16> = OsStr::new("Pulse")
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect();
                    let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                    if !hwnd.is_null() {
                        ShowWindow(hwnd, SW_RESTORE);
                        SetForegroundWindow(hwnd);
                    }
                    CloseHandle(handle);
                }
                return Err(AcquireError::AlreadyRunning);
            }
            Ok(Self { handle })
        }

        /// Usado pela cópia elevada do Pulse: a instância anterior ainda está
        /// encerrando, então espera o mutex ser liberado em vez de focá-la e sair.
        /// Esgotado o prazo, cai no comportamento normal de `acquire`.
        pub fn acquire_after_relaunch(
            name: &str,
            timeout: std::time::Duration,
        ) -> Result<Self, AcquireError> {
            let wide_name: Vec<u16> = OsStr::new(name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let started = std::time::Instant::now();
            while started.elapsed() < timeout {
                let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 1, wide_name.as_ptr()) };
                if handle.is_null() {
                    return Err(AcquireError::CreationFailed);
                }
                if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
                    return Ok(Self { handle });
                }
                unsafe {
                    CloseHandle(handle);
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Self::acquire(name)
        }
    }

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    CloseHandle(self.handle);
                }
            }
        }
    }
}

/// Instância única no Linux/macOS.
///
/// - Trava: `flock` exclusivo em `<runtime>/<nome>.lock` (`$XDG_RUNTIME_DIR` no
///   Linux; sem ele, a pasta de dados do Pulse). O kernel solta a trava quando o
///   processo morre, então não há trava órfã depois de um crash.
/// - Foco: a instância dona escuta um socket Unix (`<nome>.sock`); a segunda
///   instância envia `focus` e sai, e a dona mostra a janela principal pelo
///   handler registrado em [`set_focus_handler`] (o tray registra no setup).
#[cfg(unix)]
pub mod unix {
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    type FocusHandler = Box<dyn Fn() + Send + Sync>;

    static FOCUS_HANDLER: OnceLock<FocusHandler> = OnceLock::new();
    /// Pedido de foco que chegou antes de a janela existir.
    static PENDING_FOCUS: AtomicBool = AtomicBool::new(false);
    const FOCUS_MESSAGE: &[u8] = b"focus\n";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AcquireError {
        CreationFailed,
        AlreadyRunning,
    }

    pub struct InstanceGuard {
        socket_path: Option<PathBuf>,
        // Mantém o descritor (e com ele o `flock`) vivo até o fim do processo.
        _lock: File,
    }

    fn sanitize(name: &str) -> String {
        let clean: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if clean.is_empty() {
            "pulse".to_owned()
        } else {
            clean
        }
    }

    fn base_dir() -> PathBuf {
        dirs::runtime_dir().unwrap_or_else(crate::storage::paths::pulse_local_dir)
    }

    fn paths(name: &str) -> (PathBuf, PathBuf) {
        let base = base_dir();
        let name = sanitize(name);
        (
            base.join(format!("{name}.lock")),
            base.join(format!("{name}.sock")),
        )
    }

    fn open_lock(path: &PathBuf) -> Result<File, AcquireError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| AcquireError::CreationFailed)?;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|_| AcquireError::CreationFailed)
    }

    /// `Ok(true)` = trava obtida; `Ok(false)` = outra instância é dona.
    fn try_lock(file: &File) -> Result<bool, AcquireError> {
        // SAFETY: descritor válido enquanto `file` vive; flock não toca memória.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(true);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(false),
            _ => Err(AcquireError::CreationFailed),
        }
    }

    fn request_focus() {
        match FOCUS_HANDLER.get() {
            Some(handler) => handler(),
            None => PENDING_FOCUS.store(true, Ordering::SeqCst),
        }
    }

    /// Registra a ação de "trazer o Pulse para a frente". Se uma segunda
    /// instância pediu foco antes do registro, a ação roda na hora.
    pub fn set_focus_handler(handler: impl Fn() + Send + Sync + 'static) {
        if FOCUS_HANDLER.set(Box::new(handler)).is_ok()
            && PENDING_FOCUS.swap(false, Ordering::SeqCst)
        {
            request_focus();
        }
    }

    fn notify_existing(socket_path: &PathBuf) {
        if let Ok(mut stream) = UnixStream::connect(socket_path) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let _ = stream.write_all(FOCUS_MESSAGE);
        }
    }

    fn start_listener(socket_path: &PathBuf) -> Option<PathBuf> {
        // Quem tem a trava é o único dono legítimo: socket existente é resto de crash.
        let _ = fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path).ok()?;
        let spawned = std::thread::Builder::new()
            .name("pulse-single-instance".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let mut buffer = [0u8; 16];
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if buffer[..read].starts_with(b"focus") {
                        request_focus();
                    }
                }
            });
        if spawned.is_err() {
            let _ = fs::remove_file(socket_path);
            return None;
        }
        Some(socket_path.clone())
    }

    impl InstanceGuard {
        fn own(lock: File, socket_path: &PathBuf) -> Self {
            let mut lock = lock;
            let _ = lock.set_len(0);
            let _ = writeln!(lock, "{}", std::process::id());
            Self {
                socket_path: start_listener(socket_path),
                _lock: lock,
            }
        }

        pub fn acquire(name: &str) -> Result<Self, AcquireError> {
            let (lock_path, socket_path) = paths(name);
            let lock = open_lock(&lock_path)?;
            if try_lock(&lock)? {
                Ok(Self::own(lock, &socket_path))
            } else {
                notify_existing(&socket_path);
                Err(AcquireError::AlreadyRunning)
            }
        }

        /// Reinício após auto-atualização: a instância anterior ainda está
        /// encerrando, então espera a trava ser liberada em vez de focá-la e sair.
        /// Esgotado o prazo, cai no comportamento normal de `acquire`.
        pub fn acquire_after_relaunch(name: &str, timeout: Duration) -> Result<Self, AcquireError> {
            let (lock_path, socket_path) = paths(name);
            let lock = open_lock(&lock_path)?;
            let started = Instant::now();
            while started.elapsed() < timeout {
                if try_lock(&lock)? {
                    return Ok(Self::own(lock, &socket_path));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            drop(lock);
            Self::acquire(name)
        }
    }

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            if let Some(path) = self.socket_path.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn single_instance_guard_can_be_acquired() {
        use super::windows::InstanceGuard;
        let guard = InstanceGuard::acquire("Local\\Pulse_Test_Instance");
        assert!(guard.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn unix_guard_is_exclusive_and_forwards_focus_requests() {
        use super::unix::{set_focus_handler, AcquireError, InstanceGuard};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let name = format!("pulse-test-instance-{}", std::process::id());
        let first = InstanceGuard::acquire(&name).expect("primeira instância deve obter a trava");

        let focused = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&focused);
        set_focus_handler(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        // A segunda tentativa (outro descritor, mesmo arquivo) falha e pede foco.
        let second = InstanceGuard::acquire(&name);
        assert_eq!(second.err(), Some(AcquireError::AlreadyRunning));
        let deadline = Instant::now() + Duration::from_secs(3);
        while focused.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            focused.load(Ordering::SeqCst) >= 1,
            "pedido de foco não chegou"
        );

        // Reinício: espera a instância anterior soltar a trava.
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(first);
        });
        let relaunched = InstanceGuard::acquire_after_relaunch(&name, Duration::from_secs(5));
        releaser.join().unwrap();
        assert!(relaunched.is_ok());
    }
}
