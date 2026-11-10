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

#[cfg(not(windows))]
pub mod windows {
    pub struct InstanceGuard;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AcquireError {
        Unsupported,
    }
    impl InstanceGuard {
        pub fn acquire(_name: &str) -> Result<Self, AcquireError> {
            Ok(Self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::windows::InstanceGuard;

    #[test]
    fn single_instance_guard_can_be_acquired() {
        let guard = InstanceGuard::acquire("Local\\Pulse_Test_Instance");
        assert!(guard.is_ok());
    }
}
