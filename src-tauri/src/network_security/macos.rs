//! Stub honesto de Rede e Segurança para macOS.
//!
//! // TODO(macos): validar em hardware

use super::{now_unix_ms, NetworkSnapshot, PostureSignal, SecurityPosture, SignalSeverity};

const MACOS_UNAVAILABLE_REASON: &str = "Coleta de rede e postura ainda não validada no macOS";

pub fn collect_network() -> NetworkSnapshot {
    // TODO(macos): validar em hardware
    NetworkSnapshot::unavailable(MACOS_UNAVAILABLE_REASON)
}

pub fn collect_posture() -> SecurityPosture {
    // TODO(macos): validar em hardware
    SecurityPosture {
        signals: vec![PostureSignal {
            id: "posture_collection",
            title: "Sinais de postura",
            severity: SignalSeverity::Unavailable,
            summary: MACOS_UNAVAILABLE_REASON.to_string(),
            rationale: "Sem medição não há como afirmar que o sistema está em ordem.",
            evidence: Vec::new(),
        }],
        attention_count: 0,
        unavailable_count: 1,
        collected_at_unix_ms: now_unix_ms(),
        elevated: unsafe { libc::geteuid() == 0 },
    }
}
