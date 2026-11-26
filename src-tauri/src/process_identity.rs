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
//!
//! No Linux o caminho vem de `/proc/<pid>/exe`, o nome do aplicativo da
//! entrada `.desktop` correspondente e o "editor" da origem do pacote
//! (Flatpak, Snap ou o pacote `dpkg` dono do executável).

use crate::system_overview::TopProcess;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, OnceLock},
};

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
    pids.extend(
        process
            .process_ids
            .iter()
            .copied()
            .filter(|pid| *pid != process.id),
    );
    pids
}

/// Completa caminho, nome e editor dos processos que o PowerShell não leu.
pub fn enrich(processes: &mut [TopProcess]) {
    for process in processes.iter_mut() {
        if process.path.is_none() {
            process.path = candidate_pids(process)
                .into_iter()
                .find_map(native::image_path);
        }

        if let Some(path) = process.path.clone() {
            let needs_name = lacks_display_name(process);
            let needs_publisher = process.publisher.is_none();
            if needs_name || needs_publisher {
                let (description, company) = native::version_strings(&path);
                if needs_name {
                    // O PowerShell usa `FileDescription`; quando ele devolve o
                    // nome técnico do exe, `ProductName` tem o nome oficial.
                    process.display_name = official_name_for_path(&path).or(description);
                }
                if needs_publisher {
                    process.publisher = company;
                }
            }
        }

        #[cfg(any(windows, test))]
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

    #[cfg(target_os = "linux")]
    linux::enrich(processes);
}

/// Nome oficial de um executável lido do recurso de versão do PE.
///
/// Jogos expõem binários técnicos (`Marvel-Win64-Shipping.exe`) enquanto
/// `ProductName`/`FileDescription` trazem o nome real do produto. `ProductName`
/// tem prioridade porque em jogos Unreal a descrição costuma ser o nome do exe.
/// O cache evita reabrir o arquivo a cada reconciliação da Dock na thread de UI.
pub fn official_name_for_path(path: &str) -> Option<String> {
    static NAME_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = NAME_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().ok().and_then(|guard| guard.get(path).cloned()) {
        return cached;
    }

    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let resolved = native::version_field(path, "ProductName")
        .and_then(|value| clean_version_name(&value, stem))
        .or_else(|| {
            native::version_field(path, "FileDescription")
                .and_then(|value| clean_version_name(&value, stem))
        });

    if let Ok(mut guard) = cache.lock() {
        if guard.len() > 256 {
            guard.clear();
        }
        guard.insert(path.to_owned(), resolved.clone());
    }
    resolved
}

/// Remove caracteres de controle e devolve o rótulo apenas quando ele acrescenta
/// informação em relação ao nome do arquivo (nomes técnicos são descartados).
pub fn clean_version_name(raw: &str, exe_stem: &str) -> Option<String> {
    let sanitized: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = sanitized.trim().trim_matches('\0').trim();
    if name.is_empty() || name.eq_ignore_ascii_case(exe_stem.trim()) {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let generic = matches!(lower.as_str(), "shipping" | "unreal engine" | "unity")
        || lower.contains("-shipping")
        || lower.starts_with("unity player");
    (!generic).then(|| name.to_owned())
}

/// Escolhe o rótulo exibido: apelido conhecido > nome oficial > nome do arquivo.
pub fn choose_display_name(stem: &str, official: Option<&str>, friendly: Option<&str>) -> String {
    let stem = stem.trim();
    [friendly, official]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|candidate| !candidate.is_empty() && !candidate.eq_ignore_ascii_case(stem))
        .map(str::to_owned)
        .unwrap_or_else(|| stem.to_owned())
}

#[cfg(windows)]
use win32 as native;

#[cfg(target_os = "linux")]
mod linux;

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

    /// Bloco binário do recurso de versão + pares idioma/codepage declarados.
    type VersionBlock = (Vec<u8>, Vec<(u16, u16)>);

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

    /// Bloco de recurso de versão e os pares idioma/codepage declarados.
    fn version_block(path: &str) -> Option<VersionBlock> {
        let file = wide(path);
        unsafe {
            let size = GetFileVersionInfoSizeW(PCWSTR(file.as_ptr()), None);
            if size == 0 {
                return None;
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
                return None;
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
                let pairs =
                    std::slice::from_raw_parts(pointer as *const u16, (length / 2) as usize);
                for &[language, codepage] in pairs.as_chunks::<2>().0 {
                    translations.push((language, codepage));
                }
            }
            translations.push((0x0409, 0x04B0));
            translations.push((0x0409, 0x04E4));
            Some((block, translations))
        }
    }

    fn read_version_field(
        block: &[u8],
        translations: &[(u16, u16)],
        field: &str,
    ) -> Option<String> {
        unsafe {
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
        }
    }

    /// Campo arbitrário do recurso de versão (`ProductName`, `FileDescription`…).
    pub fn version_field(path: &str, field: &str) -> Option<String> {
        let (block, translations) = version_block(path)?;
        read_version_field(&block, &translations, field)
    }

    /// `FileDescription` e `CompanyName` do recurso de versão do arquivo.
    pub fn version_strings(path: &str) -> (Option<String>, Option<String>) {
        let Some((block, translations)) = version_block(path) else {
            return (None, None);
        };
        (
            read_version_field(&block, &translations, "FileDescription"),
            read_version_field(&block, &translations, "CompanyName"),
        )
    }
}

#[cfg(not(windows))]
mod native {
    /// Extrai o executável de `argv[0]` em `/proc/<pid>/cmdline` quando ele é
    /// um caminho absoluto válido. Rejeita nomes relativos e strings alteradas
    /// (ex.: `sshd: user@pts/0`, `-bash`).
    #[cfg(target_os = "linux")]
    pub(super) fn parse_cmdline_exe(bytes: &[u8]) -> Option<String> {
        let first = bytes.split(|&byte| byte == 0).next()?;
        let raw = std::str::from_utf8(first).ok()?.trim();
        if !raw.starts_with('/') {
            return None;
        }
        // Alguns daemons colocam a linha inteira no argv[0] separada por espaço.
        let candidate = if std::path::Path::new(raw).is_file() {
            raw
        } else {
            let head = raw.split_whitespace().next()?;
            if head.starts_with('/') && std::path::Path::new(head).is_file() {
                head
            } else {
                return None;
            }
        };
        Some(candidate.to_owned())
    }

    /// Caminho do executável por `/proc/<pid>/exe` (Linux). Para processos de
    /// outro usuário onde `exe` exige ptrace (EACCES), recorre ao `argv[0]` de
    /// `/proc/<pid>/cmdline` somente se apontar para um arquivo existente.
    #[cfg(target_os = "linux")]
    pub fn image_path(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        if let Ok(target) = std::fs::read_link(format!("/proc/{pid}/exe")) {
            let text = target.to_string_lossy();
            let cleaned = text.trim_end_matches(" (deleted)").trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_owned());
            }
        }
        let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        parse_cmdline_exe(&bytes)
    }

    /// No macOS o `sysinfo` já entrega o caminho na coleta.
    #[cfg(not(target_os = "linux"))]
    pub fn image_path(_pid: u32) -> Option<String> {
        None
    }

    /// Executáveis ELF/Mach-O não têm recurso de versão como o PE.
    pub fn version_strings(_path: &str) -> (Option<String>, Option<String>) {
        (None, None)
    }

    /// Sem recurso de versão PE, o rótulo vem do `.desktop` ou do próprio nome.
    pub fn version_field(_path: &str, _field: &str) -> Option<String> {
        None
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
            user: None,
        }
    }

    #[test]
    fn reconhece_pseudo_processos_sem_diferenciar_caixa() {
        assert_eq!(
            known_process("Memory Compression").map(|k| k.kind),
            Some("memory")
        );
        assert_eq!(
            known_process("vmmemWSL").map(|k| k.kind),
            Some("virtualization")
        );
        assert_eq!(
            known_process("MsMpEng.exe").map(|k| k.kind),
            Some("security")
        );
        assert!(known_process("chrome").is_none());
    }

    #[test]
    fn pseudo_processo_sem_caminho_recebe_nome_e_categoria() {
        let mut list = vec![process("Memory Compression")];
        enrich(&mut list);
        assert_eq!(
            list[0].display_name.as_deref(),
            Some("Compressão de memória")
        );
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
        assert_eq!(
            list[0].display_name.as_deref(),
            Some("Antimalware Service Executable")
        );
        assert!(list[0].system_kind.is_none());
    }

    #[test]
    fn processo_desconhecido_sem_caminho_fica_sem_categoria() {
        let mut list = vec![process("algo-raro")];
        enrich(&mut list);
        assert_eq!(list[0].display_name.as_deref(), Some("algo-raro"));
        assert!(list[0].system_kind.is_none());
    }

    #[test]
    fn nome_oficial_descarta_nome_tecnico_e_caracteres_de_controle() {
        assert_eq!(
            clean_version_name("Marvel's Spider-Man\u{7}", "Marvel-Win64-Shipping").as_deref(),
            Some("Marvel's Spider-Man")
        );
        assert_eq!(
            clean_version_name("Marvel-Win64-Shipping", "Marvel-Win64-Shipping"),
            None
        );
        assert_eq!(
            clean_version_name("Unreal Engine", "MeuJogo-Win64-Shipping"),
            None
        );
        assert_eq!(clean_version_name("   ", "jogo"), None);
    }

    #[test]
    fn rotulo_prefere_apelido_conhecido_e_depois_nome_oficial() {
        assert_eq!(
            choose_display_name("code", Some("Visual Studio Code"), Some("VS Code")),
            "VS Code"
        );
        // Apelido que só muda a caixa não acrescenta informação: usa o nome oficial.
        assert_eq!(
            choose_display_name("chrome", Some("Google Chrome"), Some("Chrome")),
            "Google Chrome"
        );
        assert_eq!(
            choose_display_name("Marvel-Win64-Shipping", Some("Marvel's Spider-Man"), None),
            "Marvel's Spider-Man"
        );
        assert_eq!(choose_display_name("app-raro", None, None), "app-raro");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn extrai_executavel_valido_do_cmdline() {
        assert_eq!(
            native::parse_cmdline_exe(b"/bin/sh\0-c\0true\0").as_deref(),
            Some("/bin/sh")
        );
        assert_eq!(native::parse_cmdline_exe(b"-bash\0"), None);
        assert_eq!(native::parse_cmdline_exe(b"sshd: user@pts/0\0"), None);
        assert_eq!(native::parse_cmdline_exe(b"/nao/existe/binario\0"), None);
    }
}
