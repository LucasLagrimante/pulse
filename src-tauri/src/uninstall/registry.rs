//! Registro do Windows por plataforma.
//!
//! No Windows a implementação real fica em `registry_windows.rs` (API nativa
//! `advapi32`). Fora dele não existe registro: o stub abaixo expõe a mesma API
//! e responde "chave inexistente" em toda leitura, para que os módulos que
//! consultam o registro (inventário, resíduos, tamanho de instalação) caiam no
//! caminho de "nada encontrado" sem `cfg` espalhado por eles. O inventário de
//! programas do Linux/macOS não passa por aqui: vem de `installed_unix.rs`.

#[cfg(windows)]
#[path = "registry_windows.rs"]
mod imp;

#[cfg(not(windows))]
mod imp {
    pub const ERROR_SUCCESS_CODE: u32 = 0;
    pub const ERROR_FILE_NOT_FOUND_CODE: u32 = 2;
    pub const ERROR_ACCESS_DENIED_CODE: u32 = 5;
    pub const ERROR_MORE_DATA_CODE: u32 = 234;
    pub const ERROR_NO_MORE_ITEMS_CODE: u32 = 259;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Hive {
        LocalMachine,
        CurrentUser,
        Users,
    }

    impl Hive {
        pub fn label(self) -> &'static str {
            match self {
                Hive::LocalMachine => "HKLM",
                Hive::CurrentUser => "HKCU",
                Hive::Users => "HKU",
            }
        }

        pub fn parse_label(value: &str) -> Option<Hive> {
            match value.trim().to_ascii_uppercase().as_str() {
                "HKLM" | "HKEY_LOCAL_MACHINE" => Some(Hive::LocalMachine),
                "HKCU" | "HKEY_CURRENT_USER" => Some(Hive::CurrentUser),
                "HKU" | "HKEY_USERS" => Some(Hive::Users),
                _ => None,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RegistryView {
        Native64,
        Native32,
    }

    impl RegistryView {
        pub fn label(self) -> &'static str {
            match self {
                RegistryView::Native64 => "64",
                RegistryView::Native32 => "32",
            }
        }

        pub fn parse_label(value: &str) -> Option<RegistryView> {
            match value.trim() {
                "64" => Some(RegistryView::Native64),
                "32" => Some(RegistryView::Native32),
                _ => None,
            }
        }
    }

    /// Nunca é construído fora do Windows: toda abertura falha antes.
    pub struct RegHandle {
        _private: (),
    }

    impl RegHandle {
        pub fn open(_hive: Hive, _subkey: &str, _view: RegistryView) -> Result<RegHandle, u32> {
            Err(ERROR_FILE_NOT_FOUND_CODE)
        }

        pub fn open_writable(
            _hive: Hive,
            _subkey: &str,
            _view: RegistryView,
        ) -> Result<RegHandle, u32> {
            Err(ERROR_FILE_NOT_FOUND_CODE)
        }

        pub fn exists(_hive: Hive, _subkey: &str, _view: RegistryView) -> bool {
            false
        }

        pub fn open_child(&self, _name: &str) -> Result<RegHandle, u32> {
            Err(ERROR_FILE_NOT_FOUND_CODE)
        }

        pub fn query_string(&self, _name: &str) -> Option<String> {
            None
        }

        pub fn query_u32(&self, _name: &str) -> Option<u32> {
            None
        }

        pub fn enum_keys(&self) -> Result<Vec<String>, u32> {
            Err(ERROR_FILE_NOT_FOUND_CODE)
        }

        pub fn delete_tree(self, _subkey: &str) -> Result<(), u32> {
            Err(ERROR_FILE_NOT_FOUND_CODE)
        }

        pub fn delete_key_tree(_hive: Hive, _subkey: &str, _view: RegistryView) -> Result<(), u32> {
            Err(ERROR_FILE_NOT_FOUND_CODE)
        }

        pub fn delete_empty_key(
            _hive: Hive,
            _subkey: &str,
            _view: RegistryView,
        ) -> Result<(), u32> {
            Err(ERROR_FILE_NOT_FOUND_CODE)
        }
    }
}

#[allow(unused_imports)]
pub use imp::*;
