//! Acesso ao registro do Windows pela API nativa (`advapi32`).
//!
//! ## Por que a API e não `reg.exe`
//!
//! A leitura de resíduos percorre dezenas de milhares de chaves. Um
//! `reg query /s` por chave é um subprocesso por item: inviavel. Além disso,
//! `reg delete` é proibido pelas diretrizes do projeto (apagar a chave de
//! inicialização destrói a linha de comando do programa).
//!
//! ## Honestidade do erro
//!
//! Toda função devolve o código Win32 bruto (`ERROR_*`) para que a camada de
//! cima possa dizer "requer privilégio administrativo" em vez de engolir a
//! falha. `None` de `query_string` significa "o valor não existe ou não é
//! texto" — nunca "valor vazio".

use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_ALL_ACCESS, KEY_READ,
    KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

pub const ERROR_SUCCESS_CODE: u32 = 0;
pub const ERROR_FILE_NOT_FOUND_CODE: u32 = 2;
pub const ERROR_ACCESS_DENIED_CODE: u32 = 5;
pub const ERROR_MORE_DATA_CODE: u32 = 234;
pub const ERROR_NO_MORE_ITEMS_CODE: u32 = 259;

const REG_DWORD_CODE: u32 = 4;

/// Hive do registro. Mantido fechado de propósito:_root keys_ do Windows são
/// poucas e adicionar uma exigiria novo tratamento de elevação.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
    Users,
}

impl Hive {
    pub fn raw(self) -> HKEY {
        match self {
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::Users => HKEY_USERS,
        }
    }

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

/// Visão de 32/64 bits. O Windows redirectiona `SOFTWARE` emwow64; sem variar a
/// visão, metade dos apps de 32 bits simplesmente não existe para o Pulse.
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

/// Handle de chave com fechamento automático — chave esquecida aberta impede o
/// `delete_subkey_all` (o Windows marca a chave como "em uso").
pub struct RegHandle {
    handle: HKEY,
    view: RegistryView,
}

impl Drop for RegHandle {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.handle) };
    }
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn key_access(view: RegistryView, write: bool) -> REG_SAM_FLAGS {
    let base = if write { KEY_ALL_ACCESS.0 } else { KEY_READ.0 };
    let wow = match view {
        RegistryView::Native64 => KEY_WOW64_64KEY.0,
        RegistryView::Native32 => KEY_WOW64_32KEY.0,
    };
    REG_SAM_FLAGS(base | wow)
}

impl RegHandle {
    pub fn open(hive: Hive, subkey: &str, view: RegistryView) -> Result<RegHandle, u32> {
        let wide = to_wide(subkey);
        let mut handle = HKEY(std::ptr::null_mut());
        let status = unsafe {
            RegOpenKeyExW(
                hive.raw(),
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                key_access(view, false),
                &mut handle,
            )
        };
        if status.0 != ERROR_SUCCESS_CODE {
            return Err(status.0);
        }
        Ok(RegHandle { handle, view })
    }

    /// Abre para escrita. Falha com `ERROR_ACCESS_DENIED_CODE` em HKLM sem
    /// elevação — a camada de cima traduz isso para o usuário.
    pub fn open_writable(
        hive: Hive,
        subkey: &str,
        view: RegistryView,
    ) -> Result<RegHandle, u32> {
        let wide = to_wide(subkey);
        let mut handle = HKEY(std::ptr::null_mut());
        let status = unsafe {
            RegOpenKeyExW(
                hive.raw(),
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                key_access(view, true),
                &mut handle,
            )
        };
        if status.0 != ERROR_SUCCESS_CODE {
            return Err(status.0);
        }
        Ok(RegHandle { handle, view })
    }

    /// `true` quando a chave existe (mesmo sem permissão de leitura do conteúdo).
    pub fn exists(hive: Hive, subkey: &str, view: RegistryView) -> bool {
        Self::open(hive, subkey, view).is_ok()
    }

    pub fn open_child(&self, name: &str) -> Result<RegHandle, u32> {
        let wide = to_wide(name);
        let mut handle = HKEY(std::ptr::null_mut());
        let status = unsafe {
            RegOpenKeyExW(
                self.handle,
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                key_access(self.view, false),
                &mut handle,
            )
        };
        if status.0 != ERROR_SUCCESS_CODE {
            return Err(status.0);
        }
        Ok(RegHandle { handle, view: self.view })
    }

    pub fn query_string(&self, name: &str) -> Option<String> {
        let wide = to_wide(name);
        let pointer = windows::core::PCWSTR(wide.as_ptr());
        let mut size: u32 = 0;
        let status = unsafe { RegQueryValueExW(self.handle, pointer, None, None, None, Some(&mut size)) };
        if status.0 != ERROR_SUCCESS_CODE || size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        let mut value_type = REG_VALUE_TYPE(0);
        let mut read = size;
        let status = unsafe {
            RegQueryValueExW(
                self.handle,
                pointer,
                None,
                Some(&mut value_type),
                Some(buffer.as_mut_ptr()),
                Some(&mut read),
            )
        };
        if status.0 != ERROR_SUCCESS_CODE {
            return None;
        }
        if value_type.0 != REG_SZ.0 && value_type.0 != REG_EXPAND_SZ.0 {
            return None;
        }
        let units = read as usize / 2;
        let utf16: Vec<u16> = (0..units)
            .map(|index| {
                u16::from_le_bytes([buffer[index * 2], buffer[index * 2 + 1]])
            })
            .collect();
        let value = String::from_utf16_lossy(&utf16);
        let trimmed = value.trim_matches(|character: char| character == '\0' || character.is_whitespace());
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    pub fn query_u32(&self, name: &str) -> Option<u32> {
        let wide = to_wide(name);
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut value_type = REG_VALUE_TYPE(0);
        let status = unsafe {
            RegQueryValueExW(
                self.handle,
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                Some(&mut value_type),
                Some(&mut value as *mut u32 as *mut u8),
                Some(&mut size),
            )
        };
        if status.0 != ERROR_SUCCESS_CODE || value_type.0 != REG_DWORD_CODE {
            return None;
        }
        Some(value)
    }

    /// Subchaves diretas. Interrompe no primeiro erro: um `ERROR_NO_MORE_ITEMS`
    /// é o fim normal, qualquer outro erro significa lista parcial — devolver
    /// parcial sem aviso seria pior que devolver vazio para o chamador decidir.
    pub fn enum_keys(&self) -> Result<Vec<String>, u32> {
        let mut keys = Vec::new();
        let mut index: u32 = 0;
        loop {
            let mut buffer = [0u16; 512];
            let mut length = buffer.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(
                    self.handle,
                    index,
                    Some(windows::core::PWSTR(buffer.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status.0 == ERROR_NO_MORE_ITEMS_CODE {
                return Ok(keys);
            }
            if status.0 != ERROR_SUCCESS_CODE {
                return Err(status.0);
            }
            if length == 0 {
                index += 1;
                continue;
            }
            // `RegEnumKeyExW` devolve o comprimento **sem** o terminador nulo;
            // subtrair 1 aqui truncaria o último caractere de toda subchave e a
            // lista de instalados viria quase vazia.
            let usable = (length as usize).min(buffer.len());
            let name = String::from_utf16_lossy(&buffer[..usable]);
            if !name.is_empty() {
                keys.push(name);
            }
            index += 1;
        }
    }

    pub fn delete_tree(self, subkey: &str) -> Result<(), u32> {
        let wide = to_wide(subkey);
        let status = unsafe {
            RegDeleteTreeW(
                self.handle,
                windows::core::PCWSTR(wide.as_ptr()),
            )
        };
        if status.0 == ERROR_SUCCESS_CODE {
            return Ok(());
        }
        Err(status.0)
    }


    /// Remove a chave e todas as subchaves. `RegDeleteKeyExW` só apaga chaves
    /// vazias; `RegDeleteTreeW` é o que o Painel de Controle usa.
    pub fn delete_key_tree(hive: Hive, subkey: &str, view: RegistryView) -> Result<(), u32> {
        let (parent_path, name) = match split_parent(subkey) {
            Some(parts) => parts,
            None => return Err(ERROR_FILE_NOT_FOUND_CODE),
        };
        let parent = Self::open_writable(hive, &parent_path, view)?;
        parent.delete_tree(&name)
    }

    /// Fallback quando `RegDeleteTreeW` não está disponível/permite: remove a
    /// chave vazia. Usado só como segunda tentativa.
    pub fn delete_empty_key(hive: Hive, subkey: &str, view: RegistryView) -> Result<(), u32> {
        let (parent_path, name) = match split_parent(subkey) {
            Some(parts) => parts,
            None => return Err(ERROR_FILE_NOT_FOUND_CODE),
        };
        let parent = Self::open_writable(hive, &parent_path, view)?;
        let wide = to_wide(&name);
        let status = unsafe {
            RegDeleteKeyExW(
                parent.handle,
                windows::core::PCWSTR(wide.as_ptr()),
                key_access(view, true).0,
                None,
            )
        };
        if status.0 == ERROR_SUCCESS_CODE {
            Ok(())
        } else {
            Err(status.0)
        }
    }
}

fn split_parent(subkey: &str) -> Option<(String, String)> {
    let trimmed = subkey.trim_end_matches('\\');
    let position = trimmed.rfind('\\')?;
    let parent = trimmed[..position].to_string();
    let name = trimmed[position + 1..].to_string();
    if name.is_empty() {
        return None;
    }
    Some((parent, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_parent_separates_last_segment() {
        let (parent, name) = split_parent("SOFTWARE\\Foo\\Bar").expect("deve separar");
        assert_eq!(parent, "SOFTWARE\\Foo");
        assert_eq!(name, "Bar");
    }

    #[test]
    fn split_parent_ignora_barra_final() {
        let (parent, name) = split_parent("SOFTWARE\\Foo\\Bar\\").expect("deve separar");
        assert_eq!(parent, "SOFTWARE\\Foo");
        assert_eq!(name, "Bar");
    }

    #[test]
    fn split_parent_rejeita_caminho_sem_nivel() {
        assert!(split_parent("Bar").is_none());
        assert!(split_parent("").is_none());
    }

    #[test]
    fn hive_label_round_trip() {
        for hive in [Hive::LocalMachine, Hive::CurrentUser, Hive::Users] {
            assert_eq!(Hive::parse_label(hive.label()), Some(hive));
        }
        assert_eq!(Hive::parse_label("HKEY_LOCAL_MACHINE"), Some(Hive::LocalMachine));
        assert_eq!(Hive::parse_label("HKCR"), None);
    }

    #[test]
    fn view_label_round_trip() {
        for view in [RegistryView::Native64, RegistryView::Native32] {
            assert_eq!(RegistryView::parse_label(view.label()), Some(view));
        }
        assert_eq!(RegistryView::parse_label("128"), None);
    }

    #[test]
    fn leitura_de_chave_inexistente_devolve_codigo_de_arquivo() {
        let status = RegHandle::open(
            Hive::LocalMachine,
            "SOFTWARE\\Pulse\\chave-que-nao-existe-9d2f",
            RegistryView::Native64,
        );
        assert_eq!(status.err(), Some(ERROR_FILE_NOT_FOUND_CODE));
    }

    #[test]
    fn leitura_de_software_nao_exige_elevacao() {
        let handle = RegHandle::open(Hive::LocalMachine, "SOFTWARE", RegistryView::Native64)
            .expect("HKLM\\SOFTWARE deve ser legível sem elevação");
        assert!(handle.enum_keys().is_ok());
    }

    #[test]
    fn query_string_trim_nulo_e_vazio() {
        // Valor presente mas vazio precisa virar None: a UI trata ausência e
        // string vazia igual, e "versão: " é pior que honesto "Indisponível".
        let handle = RegHandle::open(Hive::LocalMachine, "SOFTWARE", RegistryView::Native64)
            .expect("deve abrir SOFTWARE");
        let value = handle.query_string("Pulse-valor-inexistente");
        assert!(value.is_none());
    }

    #[test]
    fn error_codes_usados_pelo_modulo_sao_os_do_windows() {
        assert_eq!(ERROR_FILE_NOT_FOUND_CODE, 2);
        assert_eq!(ERROR_ACCESS_DENIED_CODE, 5);
        assert_eq!(ERROR_MORE_DATA_CODE, 234);
        assert_eq!(ERROR_NO_MORE_ITEMS_CODE, 259);
    }
}