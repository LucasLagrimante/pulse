//! Elevação real do processo e criação de ponto de restauração.
//!
//! ## Por que token e não `IsUserAnAdmin`
//!
//! `IsUserAnAdmin` (shell32) responde pelo SID do token **filtrado** do UAC:
//! para um administrador não elevado devolve `TRUE`. Se usássemos isso, o Pulse
//! criaria pontos de restauração com o token sem privilégio e falharia com
//! `ERROR_ACCESS_DENIED`. O teste certo é `CheckTokenMembership` no token
//! efetivo, que trata o SID de administradores como deny-only e devolve `FALSE`.
//!
//! ## Por que `srclient.dll` dinâmico
//!
//! `SRSetRestorePointW` não existe em toda edição do Windows (Home sem System
//! Restore). Carregar a DLL em runtime e resolver o símbolo por
//! `GetProcAddress` transforma "edição sem o serviço" em um erro declarado em
//! vez de falha de link/loading da aplicação inteira.

use std::time::{Duration, Instant};

use serde::Serialize;
use windows::core::PCSTR;
use windows::Win32::Foundation::{FreeLibrary, HMODULE, WIN32_ERROR};
use windows::core::BOOL;
use windows::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, WinBuiltinAdministratorsSid,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoInitializeSecurity, COINIT_APARTMENTTHREADED,
    EOLE_AUTHENTICATION_CAPABILITIES, RPC_C_AUTHN_LEVEL, RPC_C_IMP_LEVEL,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::Restore::{
    APPLICATION_UNINSTALL, RESTOREPOINTINFOW, RESTOREPOINTINFO_EVENT_TYPE, STATEMGRSTATUS,
};

/// `COM_DEFAULT_AUTHN_LEVEL` de `combaseapi.h`: quem autoriza a chamada é o
/// serviço System Restore, não uma credencial do processo.
const COM_DEFAULT_AUTHN_LEVEL: i32 = -1;

/// `srrestoreptapi.h`
const BEGIN_SYSTEM_CHANGE: u32 = 100;
const END_SYSTEM_CHANGE: u32 = 101;

const ERROR_SUCCESS_CODE: u32 = 0;
const ERROR_ACCESS_DENIED_CODE: u32 = 5;
const ERROR_SERVICE_DISABLED_CODE: u32 = 1057;

/// Tamanho do SID BUILTIN\Administrators. 68 é o valor oficial e-stable: o SID
/// tem tamanho fixo desde Windows 2000.
const ADMIN_SID_BUFFER: usize = 68;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallCapabilities {
    /// `true` só com token efetivo elevado (não "administrador do UAC").
    pub elevated: bool,
    pub restore_point_supported: bool,
    pub restore_point_unavailable_reason: Option<String>,
    pub verification_available: bool,
}

/// `true` quando o SID de administradores está habilitado **e presente** no
/// token efetivo. `false` inclui "não sei" — a UI nunca promete o que não foi
/// provado.
pub fn is_elevated() -> bool {
    unsafe {
        let mut buffer = [0u8; ADMIN_SID_BUFFER];
        let mut size = buffer.len() as u32;
        let created = CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            None,
            Some(windows::Win32::Security::PSID(
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
            )),
            &mut size,
        );
        if created.is_err() {
            return false;
        }
        let mut member = BOOL(0);
        CheckTokenMembership(
            None,
            windows::Win32::Security::PSID(buffer.as_mut_ptr() as *mut std::ffi::c_void),
            &mut member,
        )
        .is_ok()
            && member.as_bool()
    }
}

/// `wevtutil` consultando o log `Application` do provedor `System Restore`.
///
/// Usamos `/f:RenderedXml` de propósito: essa saída é UTF-8 e não passa pela
/// página de código do console — o mesmo cuidado que `startup_inventory.rs`
/// aplica aos eventos de boot.
fn restore_point_events(text: &str, marker: &str, started_at: Instant) -> bool {
    let threshold = chrono::Utc::now()
        - chrono::Duration::from_std(started_at.elapsed()).unwrap_or(chrono::Duration::zero());
    text.split("</Event>").any(|block| {
        block.contains(marker) && block_mentions_time_after(block, threshold)
    })
}

/// Só conta o evento se ele for **posterior** ao instante em que pedimos o
/// ponto: sem isso, uma descrição igual já existente confirmaria a criação.
fn block_mentions_time_after(block: &str, threshold: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(start) = block.find("SystemTime=\"") else {
        return false;
    };
    let rest = &block[start + "SystemTime=\"".len()..];
    let Some(end) = rest.find('"') else {
        return false;
    };
    let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(&rest[..end]) else {
        return false;
    };
    timestamp.with_timezone(&chrono::Utc) >= threshold
}

type SetRestorePointFn =
    unsafe extern "system" fn(*const RESTOREPOINTINFOW, *mut STATEMGRSTATUS) -> BOOL;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePointOutcome {
    pub created: bool,
    /// `true` apenas quando o evento de criação foi encontrado no log com a
    /// descrição que o próprio Windows gravou. Sem verificação, a UI mostra
    /// "criado, sem confirmação no log" em vez de "criado".
    pub verified: bool,
    pub sequence_number: i64,
    pub description: String,
    pub unavailable_reason: Option<String>,
}

/// Cria o ponto de restauração: `BEGIN_SYSTEM_CHANGE` e, se der certo,
/// `END_SYSTEM_CHANGE` com o mesmo número de sequência.
///
/// O Windows pode devolver sucesso sem criar nada (limite padrão de um ponto a
/// cada 24 h). Por isso a confirmação é feita no log de eventos, procurando a
/// descrição que acabamos de gravar.
pub fn create_restore_point(description: &str) -> RestorePointOutcome {
    let description = description.trim().chars().take(200).collect::<String>();
    let marker = description.clone();
    let started_at = Instant::now();

    if !is_elevated() {
        return RestorePointOutcome {
            created: false,
            verified: false,
            sequence_number: 0,
            description,
            unavailable_reason: Some(
                "O ponto de restauração exige privilégio administrativo. Reinicie o Pulse como administrador para usar esta opção."
                    .to_string(),
            ),
        };
    }

    initialize_com();

    let Some(mut call) = SrRestorePoint::load() else {
        return RestorePointOutcome {
            created: false,
            verified: false,
            sequence_number: 0,
            description,
            unavailable_reason: Some(
                "O System Restore não está disponível nesta edição do Windows.".to_string(),
            ),
        };
    };

    let (sequence_number, created) = call.run(&description);
    let verified = created && wait_for_restore_point_event(&marker, started_at);

    RestorePointOutcome {
        created,
        verified,
        sequence_number,
        description,
        unavailable_reason: if created {
            None
        } else {
            call.unavailable_reason.clone()
        },
    }
}

/// `SRSetRestorePointW` exige COM inicializado no processo. O Tauri já pode ter
/// inicializado COM como MTA, e `CoInitializeSecurity` só pode ser chamada uma
/// vez: por isso o `Once`, para não estourar `RPC_E_CHANGED_MODE` na segunda
/// desinstalação da sessão.
fn initialize_com() {
    static COM_READY: std::sync::Once = std::sync::Once::new();
    COM_READY.call_once(|| unsafe {
        let _ = CoInitializeSecurity(
            None,
            COM_DEFAULT_AUTHN_LEVEL,
            None,
            None,
            RPC_C_AUTHN_LEVEL(0),
            RPC_C_IMP_LEVEL(0),
            None,
            EOLE_AUTHENTICATION_CAPABILITIES(0),
            None,
        );
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    });
}

fn describe_restore_error(code: u32) -> String {
    match code {
        0 => "O Windows limitou a criação de pontos de restauração (um por dia, por padrão) e nenhum ponto novo foi gravado.".to_string(),
        ERROR_ACCESS_DENIED_CODE => "O Windows recusou a criação do ponto de restauração por falta de privilégio administrativo.".to_string(),
        ERROR_SERVICE_DISABLED_CODE => "O serviço System Restore está desativado nesta instalação.".to_string(),
        other => format!("O System Restore falhou (código {}).", other),
    }
}

struct SrRestorePoint {
    entry: SetRestorePointFn,
    module: HMODULE,
    unavailable_reason: Option<String>,
}

impl Drop for SrRestorePoint {
    fn drop(&mut self) {
        let _ = unsafe { FreeLibrary(self.module) };
    }
}

impl SrRestorePoint {
    fn load() -> Option<SrRestorePoint> {
        let name: Vec<u16> = "srclient.dll\0".encode_utf16().collect();
        let Ok(module) = (unsafe {
            LoadLibraryExW(
                windows::core::PCWSTR(name.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }) else {
            return None;
        };
        let symbol = std::ffi::CString::new("SRSetRestorePointW").ok()?;
        let address = unsafe { GetProcAddress(module, PCSTR(symbol.as_ptr().cast::<u8>())) };
        let Some(entry) = address else {
            let _ = unsafe { FreeLibrary(module) };
            return None;
        };
        Some(SrRestorePoint {
            entry: unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, SetRestorePointFn>(
                    entry,
                )
            },
            module,
            unavailable_reason: None,
        })
    }

    fn invoke(&self, description: &str, event_type: u32, sequence: i64) -> Result<i64, u32> {
        let mut description_buffer = [0u16; 256];
        for (slot, unit) in description_buffer
            .iter_mut()
            .zip(description.encode_utf16().take(255))
        {
            *slot = unit;
        }
        let spec = RESTOREPOINTINFOW {
            dwEventType: RESTOREPOINTINFO_EVENT_TYPE(event_type),
            dwRestorePtType: APPLICATION_UNINSTALL,
            llSequenceNumber: sequence,
            szDescription: description_buffer,
        };
        let mut status = STATEMGRSTATUS {
            nStatus: WIN32_ERROR(ERROR_SUCCESS_CODE),
            llSequenceNumber: 0,
        };
        let ok = unsafe { (self.entry)(&spec, &mut status) };
        if status.nStatus.0 != ERROR_SUCCESS_CODE {
            return Err(status.nStatus.0);
        }
        if ok.as_bool() {
            Ok(status.llSequenceNumber)
        } else {
            // `TRUE` com `nStatus` de sucesso e sequência 0 não é ponto criado:
            // o Windows pula a criação (limite de frequência) e devolve a
            // sequência do ponto já existente. Sem número não há o que confirmar.
            if status.llSequenceNumber == 0 {
                Err(ERROR_SUCCESS_CODE)
            } else {
                Ok(status.llSequenceNumber)
            }
        }
    }

    /// Devolve `(número de sequência, criado)`. Falha fica em
    /// `unavailable_reason` para a UI falar o motivo real do Windows.
    fn run(&mut self, description: &str) -> (i64, bool) {
        let sequence = match self.invoke(description, BEGIN_SYSTEM_CHANGE, 0) {
            Ok(sequence) => sequence,
            Err(code) => {
                self.unavailable_reason = Some(describe_restore_error(code));
                return (0, false);
            }
        };
        match self.invoke(description, END_SYSTEM_CHANGE, sequence) {
            Ok(_) => (sequence, true),
            Err(code) => {
                self.unavailable_reason = Some(describe_restore_error(code));
                (sequence, false)
            }
        }
    }
}

/// O evento de criação pode ser gravado com alguns segundos de atraso pelo
/// serviço, então a confirmação é uma espera curta — bounded, para nunca
/// prender a UI.
fn wait_for_restore_point_event(marker: &str, started_at: Instant) -> bool {
    let deadline = started_at + Duration::from_secs(45);
    loop {
        let output = crate::process::hidden_command("wevtutil.exe")
            .args([
                "qe",
                "Application",
                "/q:*[System[Provider[@Name='System Restore']]]",
                "/c:10",
                "/rd:true",
                "/f:RenderedXml",
            ])
            .output();
        if let Ok(result) = output {
            if result.status.success() {
                let xml = String::from_utf8_lossy(&result.stdout);
                if restore_point_events(&xml, marker, started_at) {
                    return true;
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
}

pub fn capabilities() -> UninstallCapabilities {
    let elevated = is_elevated();
    let restore_point_supported = SrRestorePoint::load().is_some();
    UninstallCapabilities {
        elevated,
        restore_point_supported,
        restore_point_unavailable_reason: if !elevated {
            Some(
                "Requer privilégio administrativo: reinicie o Pulse como administrador para gravar um ponto de restauração."
                    .to_string(),
            )
        } else if !restore_point_supported {
            Some("Esta edição do Windows não expõe o System Restore.".to_string())
        } else {
            None
        },
        verification_available: restore_point_supported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deteccao_de_elevacao_nao_panca() {
        // Não asserção sobre o valor: em CI sem elevação pode ser `false`, e em
        // máquina de dev elevado `true`. O que não pode acontecer é o processo
        // travar ou devolver erro.
        let _ = is_elevated();
    }

    #[test]
    fn capabilities_sempre_traz_justificativa_quando_sem_privilgio() {
        let caps = capabilities();
        if !caps.elevated {
            assert!(
                caps.restore_point_unavailable_reason.is_some(),
                "sem elevação a opção precisa dizer por quê"
            );
        }
    }

    #[test]
    fn restore_point_event_detection_exige_evento_posterior_ao_inicio() {
        let started = Instant::now();
        let xml = "<Event><System><TimeCreated SystemTime=\"2001-01-01T00:00:00.0000000Z\"/>\
                   <Provider Name=\"System Restore\"/></System><EventData>\
                   <Data>Pulse antes de desinstalar Teste</Data></EventData></Event>";
        assert!(!restore_point_events(xml, "Pulse antes de desinstalar Teste", started));
    }

    #[test]
    fn restore_point_event_detection_encontra_evento_com_descricao() {
        let started = Instant::now();
        let xml = "<Event><System><TimeCreated SystemTime=\"2099-01-01T00:00:00.0000000Z\"/>\
                   <Provider Name=\"System Restore\"/></System><EventData>\
                   <Data>Pulse antes de desinstalar Teste</Data></EventData></Event>";
        assert!(restore_point_events(xml, "Pulse antes de desinstalar Teste", started));
    }

    #[test]
    fn descricao_de_erro_traduz_codigos_comuns() {
        assert!(describe_restore_error(ERROR_ACCESS_DENIED_CODE).contains("administrativo"));
        assert!(describe_restore_error(ERROR_SERVICE_DISABLED_CODE).contains("desativado"));
        assert!(describe_restore_error(42).contains("42"));
    }

    #[test]
    fn descricao_vazia_ainda_revela_motivo() {
        let outcome = create_restore_point("teste");
        if !outcome.created {
            assert!(outcome.unavailable_reason.is_some());
        }
    }
}