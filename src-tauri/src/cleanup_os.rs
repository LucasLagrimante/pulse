//! Primitivas de sistema usadas pela limpeza de disco, uma por plataforma:
//! lixeira, tempo ocioso do usuário e fonte de energia.
//!
//! - Windows: Shell (`SHQueryRecycleBinW`/`SHEmptyRecycleBinW`),
//!   `GetLastInputInfo` e `GetSystemPowerStatus`.
//! - Linux: lixeira freedesktop (`~/.local/share/Trash` e `.Trash-<uid>` dos
//!   volumes montados), `IdleSinceHint` do
//!   systemd-logind e `/sys/class/power_supply`.
//! - macOS: `~/.Trash`, `HIDIdleTime` do `ioreg` e `pmset -g batt`.
//!
//! "Não sei" nunca vira valor inventado: ociosidade desconhecida é `None` (a
//! limpeza automática não roda) e energia desconhecida conta como tomada,
//! igual ao comportamento do Windows para desktops sem bateria.

use std::time::Duration;

#[cfg(windows)]
mod imp {
    use std::time::Duration;
    use windows_sys::Win32::{
        System::{
            Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
            SystemInformation::GetTickCount,
        },
        UI::{
            Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
            Shell::{
                SHEmptyRecycleBinW, SHQueryRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
                SHERB_NOSOUND, SHQUERYRBINFO,
            },
        },
    };

    pub fn query_recycle_bin() -> Result<(u64, usize), String> {
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: caminho nulo consulta todas as unidades do usuário atual; `info` é
        // um buffer válido e tem `cbSize` inicializado conforme o contrato da API.
        let status = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
        if status < 0 {
            return Err(format!(
                "A Lixeira do Windows não pôde ser consultada (HRESULT 0x{status:08X})"
            ));
        }
        Ok((
            u64::try_from(info.i64Size).unwrap_or_default(),
            usize::try_from(info.i64NumItems).unwrap_or_default(),
        ))
    }

    pub fn empty_recycle_bin() -> bool {
        // SAFETY: HWND e caminho nulos são aceitos pela API e significam todas as
        // Lixeiras do usuário. A confirmação já ocorreu na interface do Pulse.
        let status = unsafe {
            SHEmptyRecycleBinW(
                std::ptr::null_mut(),
                std::ptr::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
        };
        status >= 0
    }

    pub fn user_idle_time() -> Option<Duration> {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` é um buffer válido com `cbSize` inicializado.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return None;
        }
        // SAFETY: sem parâmetros; ambos os contadores usam a mesma base de 32 bits,
        // então `wrapping_sub` trata o estouro a cada ~49,7 dias.
        let now = unsafe { GetTickCount() };
        Some(Duration::from_millis(u64::from(
            now.wrapping_sub(info.dwTime),
        )))
    }

    pub fn on_external_power() -> bool {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: `status` é um buffer válido do tamanho esperado pela API.
        if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
            return true;
        }
        // 0 = bateria, 1 = tomada, 255 = desconhecido (desktops sem bateria).
        status.ACLineStatus != 0
    }
}

#[cfg(not(windows))]
mod imp {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// Uma lixeira: pasta com o conteúdo restaurável, pastas de metadados que
    /// são esvaziadas junto e arquivos de cache que ficam inválidos depois.
    #[derive(Debug, Clone)]
    pub(crate) struct Trash {
        pub(crate) files: PathBuf,
        pub(crate) metadata: Vec<PathBuf>,
        pub(crate) caches: Vec<PathBuf>,
        /// Lixeira principal: erro de leitura vira falha da categoria. As
        /// lixeiras de outros volumes só somam quando legíveis.
        pub(crate) primary: bool,
    }

    /// Lixeira freedesktop com raiz em `base`: conteúdo em `files/`, metadados
    /// em `info/` (e `expunged/` do KDE) e `directorysizes`, cache de tamanhos
    /// que a especificação permite recriar.
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn freedesktop_trash(base: &Path, primary: bool) -> Trash {
        Trash {
            files: base.join("files"),
            metadata: vec![base.join("info"), base.join("expunged")],
            caches: vec![base.join("directorysizes")],
            primary,
        }
    }

    /// Lixeiras do usuário. Linux: `$XDG_DATA_HOME/Trash` e, em cada volume
    /// montado, `.Trash-<uid>` ou `.Trash/<uid>` (onde o Dolphin, o Nautilus e o
    /// `gio` põem o que foi apagado fora do volume da pasta pessoal).
    #[cfg(not(target_os = "macos"))]
    fn user_trashes() -> Vec<Trash> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let home_trash = dirs::data_dir()
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join("Trash");
        let mut seen = vec![fs::canonicalize(&home_trash).unwrap_or_else(|_| home_trash.clone())];
        let mut trashes = vec![freedesktop_trash(&home_trash, true)];
        #[cfg(target_os = "linux")]
        {
            // SAFETY: sem parâmetros nem efeitos colaterais.
            let uid = unsafe { libc::geteuid() };
            for volume in crate::disk_analyzer::mounts::mounted_user_volumes() {
                let top = Path::new(&volume.mount_point);
                // A especificação manda ignorar `.Trash` compartilhada que seja link.
                let shared_is_link = top.join(".Trash").is_symlink();
                let candidates = [
                    Some(top.join(format!(".Trash-{uid}"))),
                    (!shared_is_link).then(|| top.join(".Trash").join(uid.to_string())),
                ];
                for base in candidates.into_iter().flatten() {
                    let is_real_dir = fs::symlink_metadata(&base).is_ok_and(|meta| meta.is_dir());
                    if !is_real_dir {
                        continue;
                    }
                    let canonical = fs::canonicalize(&base).unwrap_or_else(|_| base.clone());
                    if seen.contains(&canonical) {
                        continue;
                    }
                    seen.push(canonical);
                    trashes.push(freedesktop_trash(&base, false));
                }
            }
        }
        trashes
    }

    /// TODO(macos): validar em hardware. Ler `~/.Trash` exige acesso total ao
    /// disco (TCC); sem ele o erro aparece como motivo da indisponibilidade.
    #[cfg(target_os = "macos")]
    fn user_trashes() -> Vec<Trash> {
        dirs::home_dir()
            .map(|home| Trash {
                files: home.join(".Trash"),
                metadata: Vec::new(),
                caches: Vec::new(),
                primary: true,
            })
            .into_iter()
            .collect()
    }

    fn measure(path: &Path, visited: &mut usize) -> u64 {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return 0;
        };
        *visited += 1;
        if (*visited).is_multiple_of(15) {
            std::thread::yield_now();
        }
        if !meta.is_dir() {
            return meta.len();
        }
        fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| measure(&entry.path(), visited))
            .fold(0u64, u64::saturating_add)
    }

    /// `(bytes, itens de primeiro nível)` somando as lixeiras informadas.
    pub(crate) fn query_trashes(trashes: &[Trash]) -> Result<(u64, usize), String> {
        let mut bytes = 0u64;
        let mut count = 0usize;
        let mut visited = 0usize;
        for trash in trashes {
            let entries = match fs::read_dir(&trash.files) {
                Ok(entries) => entries,
                // Lixeira nunca usada: a pasta ainda não existe.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) if trash.primary => {
                    return Err(format!(
                        "A Lixeira ({}) não pôde ser lida: {error}",
                        trash.files.display()
                    ))
                }
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                if cfg!(target_os = "macos") && entry.file_name() == ".DS_Store" {
                    continue;
                }
                count += 1;
                bytes = bytes.saturating_add(measure(&entry.path(), &mut visited));
            }
        }
        Ok((bytes, count))
    }

    /// Esvazia as lixeiras informadas. `false` se algum item ficou para trás.
    pub(crate) fn empty_trashes(trashes: &[Trash]) -> bool {
        let mut ok = true;
        for trash in trashes {
            for dir in std::iter::once(&trash.files).chain(&trash.metadata) {
                let Ok(entries) = fs::read_dir(dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    // `symlink_metadata`: um link na lixeira é removido como link,
                    // nunca seguido para apagar o destino.
                    let removed = match fs::symlink_metadata(&path) {
                        Ok(meta) if meta.is_dir() => fs::remove_dir_all(&path),
                        Ok(_) => fs::remove_file(&path),
                        Err(error) => Err(error),
                    };
                    ok &= removed.is_ok();
                }
            }
            for cache in &trash.caches {
                // Cache de tamanhos por pasta: sem as pastas, as linhas ficam
                // inválidas. O gerenciador de arquivos o recria quando precisar.
                if fs::symlink_metadata(cache).is_ok_and(|meta| meta.is_file()) {
                    let _ = fs::remove_file(cache);
                }
            }
        }
        ok
    }

    pub fn query_recycle_bin() -> Result<(u64, usize), String> {
        let trashes = user_trashes();
        if trashes.is_empty() {
            return Err("A pasta pessoal não foi encontrada para localizar a Lixeira.".into());
        }
        query_trashes(&trashes)
    }

    pub fn empty_recycle_bin() -> bool {
        empty_trashes(&user_trashes())
    }

    #[cfg(target_os = "linux")]
    pub fn user_idle_time() -> Option<Duration> {
        // logind recebe o estado ocioso do ambiente gráfico (KDE, GNOME,
        // Cinnamon...). `IdleSinceHint` é o instante em µs CLOCK_REALTIME.
        let output = std::process::Command::new("loginctl")
            .args([
                "show-session",
                "auto",
                "-p",
                "IdleHint",
                "-p",
                "IdleSinceHint",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        parse_logind_idle(&text, std::time::SystemTime::now())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn parse_logind_idle(text: &str, now: std::time::SystemTime) -> Option<Duration> {
        let mut idle = None;
        let mut since = None;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("IdleHint", value)) => idle = Some(value.trim() == "yes"),
                Some(("IdleSinceHint", value)) => since = value.trim().parse::<u64>().ok(),
                _ => {}
            }
        }
        match (idle?, since?) {
            (false, _) => Some(Duration::ZERO),
            (true, 0) => None,
            (true, micros) => {
                let since = std::time::UNIX_EPOCH + Duration::from_micros(micros);
                now.duration_since(since).ok()
            }
        }
    }

    #[cfg(target_os = "macos")]
    pub fn user_idle_time() -> Option<Duration> {
        let output = std::process::Command::new("ioreg")
            .args(["-c", "IOHIDSystem", "-d", "4"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().find(|line| line.contains("\"HIDIdleTime\""))?;
        let nanos = line.rsplit('=').next()?.trim().parse::<u64>().ok()?;
        Some(Duration::from_nanos(nanos))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn user_idle_time() -> Option<Duration> {
        None
    }

    #[cfg(target_os = "linux")]
    pub fn on_external_power() -> bool {
        let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
            return true;
        };
        let mut saw_mains = false;
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
            if kind.trim() != "Mains" {
                continue;
            }
            saw_mains = true;
            if fs::read_to_string(path.join("online")).is_ok_and(|v| v.trim() == "1") {
                return true;
            }
        }
        // Sem fonte "Mains" (desktop) conta como tomada.
        !saw_mains
    }

    #[cfg(target_os = "macos")]
    pub fn on_external_power() -> bool {
        std::process::Command::new("pmset")
            .args(["-g", "batt"])
            .output()
            .map(|out| !String::from_utf8_lossy(&out.stdout).contains("'Battery Power'"))
            .unwrap_or(true)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn on_external_power() -> bool {
        true
    }
}

/// `(bytes, itens)` da lixeira do usuário.
pub fn query_recycle_bin() -> Result<(u64, usize), String> {
    imp::query_recycle_bin()
}

/// Esvazia a lixeira do usuário. `false` quando algum item não pôde ser removido.
pub fn empty_recycle_bin() -> bool {
    imp::empty_recycle_bin()
}

/// Tempo desde a última entrada de teclado/mouse da sessão interativa.
pub fn user_idle_time() -> Option<Duration> {
    imp::user_idle_time()
}

/// `false` só quando o sistema confirma que o notebook está na bateria.
pub fn on_external_power() -> bool {
    imp::on_external_power()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::imp::parse_logind_idle;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn logind_ativo_e_ocioso_zero() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(
            parse_logind_idle("IdleHint=no\nIdleSinceHint=0\n", now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn logind_ocioso_mede_desde_o_hint() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let text = "IdleHint=yes\nIdleSinceHint=400000000\n";
        assert_eq!(parse_logind_idle(text, now), Some(Duration::from_secs(600)));
    }

    #[test]
    fn logind_sem_hint_e_desconhecido() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(
            parse_logind_idle("IdleHint=yes\nIdleSinceHint=0\n", now),
            None
        );
        assert_eq!(parse_logind_idle("", now), None);
    }

    /// Lixeira freedesktop falsa num diretório temporário: mede, esvazia
    /// conteúdo e metadados, apaga o cache e nunca segue links.
    #[test]
    fn lixeira_injetavel_mede_e_esvazia_sem_seguir_links() {
        use super::imp::{empty_trashes, freedesktop_trash, query_trashes};
        use std::fs;
        let root = std::env::temp_dir().join(format!("pulse-trash-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let base = root.join("Trash");
        let outside = root.join("fora.txt");
        fs::create_dir_all(base.join("files/pasta")).unwrap();
        fs::create_dir_all(base.join("info")).unwrap();
        fs::write(base.join("files/a.txt"), b"12345").unwrap();
        fs::write(base.join("files/pasta/b.bin"), b"123").unwrap();
        fs::write(base.join("info/a.txt.trashinfo"), b"[Trash Info]").unwrap();
        fs::write(base.join("directorysizes"), b"3 0 pasta").unwrap();
        fs::write(&outside, b"nao apagar").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("files/link")).unwrap();

        let trash = [freedesktop_trash(&base, true)];
        let (bytes, items) = query_trashes(&trash).unwrap();
        assert_eq!(items, 3);
        assert!(bytes >= 8, "{bytes}");

        assert!(empty_trashes(&trash));
        assert_eq!(query_trashes(&trash).unwrap(), (0, 0));
        assert_eq!(fs::read_dir(base.join("info")).unwrap().count(), 0);
        assert!(!base.join("directorysizes").exists());
        assert_eq!(fs::read(&outside).unwrap(), b"nao apagar");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lixeira_inexistente_conta_como_vazia() {
        let base = std::env::temp_dir().join("pulse-trash-que-nao-existe");
        let trash = [super::imp::freedesktop_trash(&base, true)];
        assert_eq!(super::imp::query_trashes(&trash).unwrap(), (0, 0));
    }

    /// Coleta real, só leitura: a lixeira do usuário desta máquina é consultada
    /// sem nunca ser esvaziada.
    #[test]
    fn lixeira_real_e_consultada_sem_erro() {
        assert!(super::query_recycle_bin().is_ok());
    }
}
