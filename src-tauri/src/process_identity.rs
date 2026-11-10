//! Identidade de processos que o PowerShell não consegue resolver.
//!
//! `Get-Process .Path` lê o módulo principal e exige acesso total ao processo:
//! falha em processos elevados, de outra sessão ou protegidos (PPL, como o
//! Defender). Aqui o caminho vem de `QueryFullProcessImageNameW`, que só exige
//! `PROCESS_QUERY_LIMITED_INFORMATION` (o mesmo que o Gerenciador de Tarefas
//! usa), e nome/editor vêm do recurso de versão do próprio arquivo.
//!
//! Pseudo-processos sem executável (Memory Compression, vmmemWSL, System…)
//! recebem um nome descritivo e uma categoria (`system_kind`) para o
//! frontend exibir um glifo semântico — nunca um ícone de outro aplicativo.

use crate::system_overview::TopProcess;

/// Identidade conhecida de um processo do Windows sem arquivo legível.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownProcess {
    pub display_name: &'static str,
    pub publisher: &'static str,
    /// Categoria para o glifo do frontend: `kernel`, `memory`,
    /// `virtualization` ou `security`.
    pub kind: &'static str,
    /// Executável relacionado cujo ícone representa o processo de fato
    /// (ex.: `wsl.exe` para a VM do WSL). `None` usa só o glifo.
    pub icon_source: Option<&'static str>,
}

/// Catálogo de processos do Windows que não expõem um executável próprio
/// ou cujo binário é protegido contra leitura.
pub fn known_process(name: &str) -> Option<KnownProcess> {
    let known = match name.trim().to_ascii_lowercase().trim_end_matches(".exe") {
        "system" => KnownProcess {
            display_name: "Sistema (kernel do Windows)",
            publisher: "Microsoft Windows",
            kind: "kernel",
            icon_source: None,
        },
        "registry" => KnownProcess {
            display_name: "Registro do Windows",
            publisher: "Microsoft Windows",
            kind: "kernel",
            icon_source: None,
        },
        "secure system" => KnownProcess {
            display_name: "Sistema seguro (VBS)",
            publisher: "Microsoft Windows",
            kind: "kernel",
            icon_source: None,
        },
        "idle" => KnownProcess {
            display_name: "Processo ocioso do sistema",
            publisher: "Microsoft Windows",
            kind: "kernel",
            icon_source: None,
        },
        "memory compression" => KnownProcess {
            display_name: "Compressão de memória",
            publisher: "Microsoft Windows",
            kind: "memory",
            icon_source: None,
        },
        "vmmemwsl" => KnownProcess {
            display_name: "WSL — máquina virtual Linux",
            publisher: "Microsoft Corporation",
            kind: "virtualization",
            icon_source: Some("%SystemRoot%\\System32\\wsl.exe"),
        },
        "vmmem" => KnownProcess {
            display_name: "Máquina virtual (Hyper-V)",
            publisher: "Microsoft Corporation",
            kind: "virtualization",
            icon_source: None,
        },
        "msmpeng" => KnownProcess {
            display_name: "Microsoft Defender Antivirus",
            publisher: "Microsoft Corporation",
            kind: "security",
            icon_source: None,
        },
        "nissrv" => KnownProcess {
            display_name: "Inspeção de rede do Microsoft Defender",
            publisher: "Microsoft Corporation",
            kind: "security",
            icon_source: None,
        },
        _ => return None,
    };
    Some(known)
}

/// O PowerShell usa o próprio `ProcessName` como nome quando não leu a versão.
fn lacks_display_name(process: &TopProcess) -> bool {
    match process.display_name.as_deref().map(str::trim) {
        None | Some("") => true,
        Some(display) => display.eq_ignore_ascii_case(process.name.trim()),
    }
}

fn candidate_pids(process: &TopProcess) -> Vec<u32> {
    let mut pids = vec![process.id];
    pids.extend(process.process_ids.iter().copied().filter(|pid| *pid != process.id));
    pids
}

/// Completa caminho, nome e editor dos processos que o PowerShell não leu.
pub fn enrich(processes: &mut [TopProcess]) {
    for process in processes.iter_mut() {
        if process.path.is_none() {
            process.path = candidate_pids(process)
                .into_iter()
                .find_map(win32::image_path);
        }

        if let Some(path) = process.path.clone() {
            if lacks_display_name(process) || process.publisher.is_none() {
                let (description, company) = win32::version_strings(&path);
                if lacks_display_name(process) {
                    if let Some(description) = description {
                        process.display_name = Some(description);
                    }
                }
                if process.publisher.is_none() {
                    process.publisher = company;
                }
            }
        }

        if let Some(known) = known_process(&process.name) {
            if lacks_display_name(process) {
                process.display_name = Some(known.display_name.to_owned());
            }
            if process.publisher.is_none() {
                process.publisher = Some(known.publisher.to_owned());
            }
            if process.path.is_none() {
                process.system_kind = Some(known.kind.to_owned());
            }
        }
    }
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Caminho do executável com acesso limitado; `None` para PID 0,
    /// processo encerrado, acesso negado ou pseudo-processo sem imagem.
    pub fn image_path(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
        };
        unsafe {
            let _ = CloseHandle(handle);
        }
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        let path = path.trim();
        // Pseudo-processos (ex.: Memory Compression) devolvem só o nome, sem pasta.
        if path.is_empty() || !path.contains('\\') {
            None
        } else {
            Some(path.to_owned())
        }
    }

    /// `FileDescription` e `CompanyName` do recurso de versão do arquivo.
    pub fn version_strings(path: &str) -> (Option<String>, Option<String>) {
        let file = wide(path);
        unsafe {
            let size = GetFileVersionInfoSizeW(PCWSTR(file.as_ptr()), None);
            if size == 0 {
                return (None, None);
            }
            let mut block = vec![0u8; size as usize];
            if GetFileVersionInfoW(
                PCWSTR(file.as_ptr()),
                None,
                size,
                block.as_mut_ptr() as *mut c_void,
            )
            .is_err()
            {
                return (None, None);
            }

            // Primeiro par idioma/codepage declarado; 0409/04B0 como reserva.
            let mut translations: Vec<(u16, u16)> = Vec::new();
            let mut pointer: *mut c_void = std::ptr::null_mut();
            let mut length = 0u32;
            let query = wide("\\VarFileInfo\\Translation");
            if VerQueryValueW(
                block.as_ptr() as *const c_void,
                PCWSTR(query.as_ptr()),
                &mut pointer,
                &mut length,
            )
            .as_bool()
                && !pointer.is_null()
                && length >= 4
            {
                let pairs = std::slice::from_raw_parts(pointer as *const u16, (length / 2) as usize);
                for &[language, codepage] in pairs.as_chunks::<2>().0 {
                    translations.push((language, codepage));
                }
            }
            translations.push((0x0409, 0x04B0));
            translations.push((0x0409, 0x04E4));

            let read = |field: &str| -> Option<String> {
                translations.iter().find_map(|(language, codepage)| {
                    let key = wide(&format!(
                        "\\StringFileInfo\\{language:04x}{codepage:04x}\\{field}"
                    ));
                    let mut value: *mut c_void = std::ptr::null_mut();
                    let mut chars = 0u32;
                    let found = VerQueryValueW(
                        block.as_ptr() as *const c_void,
                        PCWSTR(key.as_ptr()),
                        &mut value,
                        &mut chars,
                    )
                    .as_bool();
                    if !found || value.is_null() || chars == 0 {
                        return None;
                    }
                    let text = std::slice::from_raw_parts(value as *const u16, chars as usize);
                    let text = String::from_utf16_lossy(text);
                    let text = text.trim_end_matches('\0').trim();
                    (!text.is_empty()).then(|| text.to_owned())
                })
            };

            (read("FileDescription"), read("CompanyName"))
        }
    }
}

#[cfg(not(windows))]
mod win32 {
    pub fn image_path(_pid: u32) -> Option<String> {
        None
    }

    pub fn version_strings(_path: &str) -> (Option<String>, Option<String>) {
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(name: &str) -> TopProcess {
        TopProcess {
            name: name.to_owned(),
            id: 4,
            memory_mb: 10.0,
            process_ids: vec![4],
            process_count: 1,
            display_name: Some(name.to_owned()),
            publisher: None,
            cpu_percent: None,
            gpu_percent: None,
            icon: None,
            path: None,
            system_kind: None,
        }
    }

    #[test]
    fn reconhece_pseudo_processos_sem_diferenciar_caixa() {
        assert_eq!(known_process("Memory Compression").map(|k| k.kind), Some("memory"));
        assert_eq!(known_process("vmmemWSL").map(|k| k.kind), Some("virtualization"));
        assert_eq!(known_process("MsMpEng.exe").map(|k| k.kind), Some("security"));
        assert!(known_process("chrome").is_none());
    }

    #[test]
    fn pseudo_processo_sem_caminho_recebe_nome_e_categoria() {
        let mut list = vec![process("Memory Compression")];
        enrich(&mut list);
        assert_eq!(list[0].display_name.as_deref(), Some("Compressão de memória"));
        assert_eq!(list[0].publisher.as_deref(), Some("Microsoft Windows"));
        assert_eq!(list[0].system_kind.as_deref(), Some("memory"));
    }

    #[test]
    fn nao_sobrescreve_nome_lido_do_arquivo() {
        let mut item = process("MsMpEng");
        item.display_name = Some("Antimalware Service Executable".to_owned());
        item.path = Some("C:\\ProgramData\\Microsoft\\Windows Defender\\MsMpEng.exe".to_owned());
        let mut list = vec![item];
        enrich(&mut list);
        assert_eq!(list[0].display_name.as_deref(), Some("Antimalware Service Executable"));
        assert!(list[0].system_kind.is_none());
    }

    #[test]
    fn processo_desconhecido_sem_caminho_fica_sem_categoria() {
        let mut list = vec![process("algo-raro")];
        enrich(&mut list);
        assert_eq!(list[0].display_name.as_deref(), Some("algo-raro"));
        assert!(list[0].system_kind.is_none());
    }
}
