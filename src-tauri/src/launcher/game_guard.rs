//! Pausa o hook da tecla Windows enquanto um jogo protegido por anti-cheat está aberto.
//!
//! Anti-cheats (ex.: o do Marvel Rivals) tomam o foco do runner e abrem a
//! Pesquisa do Windows, e alguns tratam um `WH_KEYBOARD_LL` ativo como
//! interceptação de teclado. Em vez de competir com eles, o Pulse desinstala
//! o hook por completo: a tecla Windows volta a ser 100% nativa até o jogo fechar.
//!
//! A detecção é por nome de processo (snapshot Toolhelp a cada `POLL`), só
//! enquanto o usuário quer o runner na tecla Windows.

/// Executáveis que pausam o runner (comparação sem diferenciar maiúsculas).
/// Inclui anti-cheats com processo próprio e jogos cujo anti-cheat é só driver.
pub const GUARDED_PROCESSES: &[&str] = &[
    // NetEase (anti-cheat só em driver)
    "marvel-win64-shipping.exe",
    "narakabladepoint.exe",
    // Riot Vanguard (driver vgk; o vgc nem sempre está de pé)
    "valorant-win64-shipping.exe",
    "league of legends.exe",
    // Activision Ricochet (só driver)
    "cod.exe",
    "modernwarfare.exe",
    "blackopscoldwar.exe",
    // HoYoverse (driver mhyprot)
    "genshinimpact.exe",
    "yuanshen.exe",
    "starrail.exe",
    "zenlesszonezero.exe",
    // Tencent ACE
    "deltaforceclient-win64-shipping.exe",
    // Anti-cheat embutido no próprio jogo
    "robloxplayerbeta.exe",
    "overwatch.exe",
    "cs2.exe",
    // Jogos com EAC/BattlEye/EA Javelin (o processo do anti-cheat pode
    // demorar a subir ou rodar como serviço protegido)
    "fortniteclient-win64-shipping.exe",
    "r5apex.exe",
    "r5apex_dx12.exe",
    "tslgame.exe",
    "rainbowsix.exe",
    "rainbowsix_be.exe",
    "escapefromtarkov.exe",
    "rustclient.exe",
    "deadbydaylight-win64-shipping.exe",
    "destiny2.exe",
    "bf2042.exe",
    "discovery.exe",
    "lostark.exe",
    "eldenring.exe",
    "haloinfinite.exe",
    "gta5.exe",
    "gta5_enhanced.exe",
    // EA Javelin
    "eaanticheat.gameservicelauncher.exe",
    "eaanticheat.gameservice.exe",
    // Easy Anti-Cheat
    "easyanticheat.exe",
    "easyanticheat_eos.exe",
    "start_protected_game.exe",
    // BattlEye
    "beservice.exe",
    "beservice_x64.exe",
    // Riot Vanguard
    "vgc.exe",
    // FACEIT
    "faceit.exe",
    "faceitservice.exe",
    // Tencent ACE
    "sguard64.exe",
    "sguardsvc64.exe",
    // nProtect GameGuard
    "gamemon.des",
    "gamemon64.des",
];

/// Primeiro processo da lista protegida encontrado entre os nomes informados.
pub fn find_guarded<'a, I>(names: I) -> Option<&'static str>
where
    I: IntoIterator<Item = &'a str>,
{
    names.into_iter().find_map(|name| {
        GUARDED_PROCESSES
            .iter()
            .copied()
            .find(|guarded| name.eq_ignore_ascii_case(guarded))
    })
}

#[cfg(windows)]
mod platform {
    use super::find_guarded;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    const POLL: Duration = Duration::from_secs(3);

    static STARTED: AtomicBool = AtomicBool::new(false);
    static ACTIVE: OnceLock<Mutex<Option<&'static str>>> = OnceLock::new();

    fn active_slot() -> &'static Mutex<Option<&'static str>> {
        ACTIVE.get_or_init(|| Mutex::new(None))
    }

    /// Processo protegido que está pausando o runner agora, se houver.
    pub fn active() -> Option<&'static str> {
        active_slot().lock().ok().and_then(|guard| *guard)
    }

    fn scan() -> Result<Option<&'static str>, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|e| format!("CreateToolhelp32Snapshot falhou: {e}"))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut names = Vec::new();
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while ok {
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            names.push(String::from_utf16_lossy(&entry.szExeFile[..end]));
            ok = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }
        unsafe {
            let _ = CloseHandle(snapshot);
        }
        Ok(find_guarded(names.iter().map(String::as_str)))
    }

    /// Inicia (uma única vez) a thread de vigilância. `on_change` é chamado
    /// quando o estado muda (entrou/saiu de jogo protegido).
    pub fn start(wanted: fn() -> bool, on_change: fn()) {
        if STARTED.swap(true, Ordering::AcqRel) {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("pulse-game-guard".into())
            .spawn(move || {
                let mut last_error_logged = false;
                loop {
                    let detected = if wanted() {
                        match scan() {
                            Ok(found) => {
                                last_error_logged = false;
                                found
                            }
                            Err(error) => {
                                if !last_error_logged {
                                    last_error_logged = true;
                                    crate::debug::record_failure(
                                        "launcher_game_guard",
                                        0,
                                        crate::debug::Failure::new(
                                            "Não foi possível verificar jogos protegidos",
                                        )
                                        .stage("toolhelp_snapshot")
                                        .detail(error),
                                    );
                                }
                                active()
                            }
                        }
                    } else {
                        None
                    };
                    let changed = {
                        let mut guard = active_slot().lock().unwrap_or_else(|e| e.into_inner());
                        let changed = *guard != detected;
                        *guard = detected;
                        changed
                    };
                    if changed {
                        on_change();
                    }
                    std::thread::sleep(POLL);
                }
            });
        if let Err(error) = spawned {
            STARTED.store(false, Ordering::Release);
            crate::debug::record_failure(
                "launcher_game_guard",
                0,
                crate::debug::Failure::new(
                    "Não foi possível iniciar a detecção de jogos protegidos",
                )
                .stage("spawn")
                .detail(error.to_string()),
            );
        }
    }
}

#[cfg(windows)]
pub use platform::{active, start};

#[cfg(not(windows))]
pub fn active() -> Option<&'static str> {
    None
}

#[cfg(not(windows))]
pub fn start(_wanted: fn() -> bool, _on_change: fn()) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_game_and_anti_cheat_ignoring_case() {
        assert_eq!(
            find_guarded(["explorer.exe", "Marvel-Win64-Shipping.exe"]),
            Some("marvel-win64-shipping.exe")
        );
        assert_eq!(
            find_guarded(["EasyAntiCheat_EOS.exe"]),
            Some("easyanticheat_eos.exe")
        );
    }

    #[test]
    fn guarded_list_is_lowercase_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for name in GUARDED_PROCESSES {
            assert_eq!(
                *name,
                name.to_ascii_lowercase(),
                "{name} fora de minúsculas"
            );
            assert!(seen.insert(*name), "{name} duplicado");
        }
    }

    #[test]
    fn detects_games_with_driver_only_anti_cheat() {
        assert_eq!(
            find_guarded(["VALORANT-Win64-Shipping.exe"]),
            Some("valorant-win64-shipping.exe")
        );
        assert_eq!(
            find_guarded(["League of Legends.exe"]),
            Some("league of legends.exe")
        );
        assert_eq!(
            find_guarded(["GenshinImpact.exe"]),
            Some("genshinimpact.exe")
        );
    }

    #[test]
    fn ignores_unrelated_processes_and_partial_names() {
        assert_eq!(find_guarded(["Marvel.exe", "code.exe", "beservice"]), None);
        assert_eq!(find_guarded(std::iter::empty::<&str>()), None);
    }
}
