//! Histórico local de consumo das cotas de IA.
//!
//! Toda atualização de cotas que termina com scan `ready` grava um ponto por
//! janela medida neste arquivo append-only (JSONL, 100% local):
//! `%LOCALAPPDATA%\Pulse\quota-history.jsonl`.
//!
//! O app, o painel rápido (tray flyout) e o tray passam pelo mesmo
//! `get_subscription_usage`, então um único ponto de gravação cobre os três.
//! A interface consulta via `get_quota_history` e desenha um gráfico por
//! janela com o dia de maior consumo dos últimos 30 dias.

use crate::subscription_usage::ProviderUsage;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// Dias retidos em disco (margem acima dos 30 exibidos).
pub const RETENTION_DAYS: i64 = 35;
/// Dias retornados por padrão na consulta.
pub const DEFAULT_QUERY_DAYS: u32 = 30;
/// Teto de dias aceito na consulta.
pub const MAX_QUERY_DAYS: u32 = 35;
/// Intervalo mínimo entre dois pontos da mesma série.
pub const MIN_INTERVAL_MS: i64 = 10 * 60 * 1000;
/// Variação de uso que justifica um ponto extra dentro do intervalo mínimo.
pub const SIGNIFICANT_DELTA: f64 = 5.0;
/// Teto de pontos retornados por consulta.
pub const MAX_POINTS: usize = 6000;
/// Série sintética para saldo/créditos de provedores sem janelas (OpenRouter).
pub const CREDITS_WINDOW_ID: &str = "__credits__";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaHistoryPoint {
    pub provider_id: String,
    pub window_id: String,
    pub label: String,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub credits_remaining: Option<f64>,
    pub sampled_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaHistoryResponse {
    pub points: Vec<QuotaHistoryPoint>,
    pub retained_days: u32,
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn sanitize_percent(value: Option<f64>) -> Option<f64> {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
}

fn sanitize_credits(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

pub fn history_file() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
        .join("quota-history.jsonl")
}

/// Extrai um ponto por janela medida. Ignora provedores desativados,
/// indisponíveis ou sem leitura — nunca grava zero fictício.
pub(crate) fn points_from_providers(
    providers: &[ProviderUsage],
    sampled_at_fallback_ms: i64,
) -> Vec<QuotaHistoryPoint> {
    let mut points = Vec::new();
    for provider in providers {
        if provider.disabled || provider.status != "available" {
            continue;
        }
        let sampled_at = provider
            .sampled_at_unix_ms
            .filter(|sampled| *sampled > 0)
            .unwrap_or(sampled_at_fallback_ms);
        for (index, window) in provider.windows.iter().enumerate() {
            let used = sanitize_percent(window.used_percent);
            let remaining = sanitize_percent(window.remaining_percent);
            if used.is_none() && remaining.is_none() {
                continue;
            }
            let window_id = if window.id.trim().is_empty() {
                format!("window-{index}")
            } else {
                window.id.clone()
            };
            points.push(QuotaHistoryPoint {
                provider_id: provider.id.clone(),
                window_id,
                label: if window.label.trim().is_empty() {
                    "Cota".into()
                } else {
                    window.label.clone()
                },
                used_percent: used,
                remaining_percent: remaining,
                credits_remaining: None,
                sampled_at_unix_ms: sampled_at,
            });
        }
        if let Some(credits) = sanitize_credits(provider.credits_remaining) {
            points.push(QuotaHistoryPoint {
                provider_id: provider.id.clone(),
                window_id: CREDITS_WINDOW_ID.into(),
                label: if provider.id == "openrouter" {
                    "Saldo (USD)".into()
                } else {
                    "Créditos".into()
                },
                used_percent: None,
                remaining_percent: None,
                credits_remaining: Some(credits),
                sampled_at_unix_ms: sampled_at,
            });
        }
    }
    points
}

/// Decide se um ponto novo merece gravação (piso de intervalo + salto relevante).
fn should_record(
    last_sampled_ms: Option<i64>,
    last_used: Option<f64>,
    point: &QuotaHistoryPoint,
) -> bool {
    let Some(last_sampled_ms) = last_sampled_ms else {
        return true;
    };
    let elapsed = point.sampled_at_unix_ms - last_sampled_ms;
    if elapsed >= MIN_INTERVAL_MS {
        return true;
    }
    if elapsed < 60_000 {
        return false;
    }
    match (last_used, point.used_percent) {
        (Some(previous), Some(current)) => (current - previous).abs() >= SIGNIFICANT_DELTA,
        _ => false,
    }
}

fn read_all_points() -> Vec<QuotaHistoryPoint> {
    let Ok(content) = fs::read_to_string(history_file()) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Grava o snapshot final do scan. Chamado uma única vez por ciclo `ready`;
/// leituras intermediárias (`running`) não geram pontos.
pub(crate) fn record_report(providers: &[ProviderUsage]) {
    let now = now_unix_ms();
    let points = points_from_providers(providers, now);
    if points.is_empty() {
        return;
    }
    let file = history_file();
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let cutoff = now - RETENTION_DAYS * 86_400_000;
    let existing = read_all_points();
    let mut last: HashMap<(String, String), (i64, Option<f64>)> = HashMap::new();
    for point in &existing {
        let key = (point.provider_id.clone(), point.window_id.clone());
        match last.get(&key) {
            Some((sampled, _)) if *sampled >= point.sampled_at_unix_ms => {}
            _ => {
                last.insert(key, (point.sampled_at_unix_ms, point.used_percent));
            }
        }
    }
    let fresh: Vec<&QuotaHistoryPoint> = points
        .iter()
        .filter(|point| {
            let key = (point.provider_id.clone(), point.window_id.clone());
            match last.get(&key) {
                Some((sampled, used)) => should_record(Some(*sampled), *used, point),
                None => true,
            }
        })
        .collect();
    if fresh.is_empty() {
        return;
    }
    if let Ok(mut handle) = OpenOptions::new().create(true).append(true).open(&file) {
        for point in &fresh {
            if let Ok(line) = serde_json::to_string(point) {
                let _ = writeln!(handle, "{line}");
            }
        }
    }
    // Poda por idade em seguida: o arquivo fica limitado a ~35 dias.
    let retained: Vec<&QuotaHistoryPoint> = existing
        .iter()
        .filter(|point| point.sampled_at_unix_ms >= cutoff)
        .collect();
    let needs_prune = retained.len() != existing.len();
    if needs_prune {
        let mut pruned: Vec<u8> = Vec::new();
        for point in retained {
            if let Ok(line) = serde_json::to_string(point) {
                pruned.extend_from_slice(line.as_bytes());
                pruned.push(b'\n');
            }
        }
        for point in fresh {
            if let Ok(line) = serde_json::to_string(point) {
                pruned.extend_from_slice(line.as_bytes());
                pruned.push(b'\n');
            }
        }
        let tmp = file.with_extension("jsonl.tmp");
        if fs::write(&tmp, &pruned).is_ok() {
            let _ = fs::rename(&tmp, &file);
        }
    }
}

fn get_quota_history_sync(
    provider_id: Option<String>,
    window_id: Option<String>,
    days: Option<u32>,
) -> QuotaHistoryResponse {
    let days = days
        .unwrap_or(DEFAULT_QUERY_DAYS)
        .clamp(1, MAX_QUERY_DAYS);
    let cutoff = now_unix_ms() - i64::from(days) * 86_400_000;
    let mut points: Vec<QuotaHistoryPoint> = read_all_points()
        .into_iter()
        .filter(|point| point.sampled_at_unix_ms >= cutoff)
        .filter(|point| {
            provider_id
                .as_deref()
                .map_or(true, |id| point.provider_id == id)
        })
        .filter(|point| {
            window_id
                .as_deref()
                .map_or(true, |id| point.window_id == id)
        })
        .collect();
    points.sort_by_key(|point| point.sampled_at_unix_ms);
    if points.len() > MAX_POINTS {
        let excess = points.len() - MAX_POINTS;
        points.drain(..excess);
    }
    QuotaHistoryResponse {
        points,
        retained_days: RETENTION_DAYS as u32,
    }
}

#[tauri::command]
pub async fn get_quota_history(
    provider_id: Option<String>,
    window_id: Option<String>,
    days: Option<u32>,
) -> QuotaHistoryResponse {
    tauri::async_runtime::spawn_blocking(move || {
        get_quota_history_sync(provider_id, window_id, days)
    })
    .await
    .unwrap_or_else(|_| QuotaHistoryResponse {
        points: Vec::new(),
        retained_days: RETENTION_DAYS as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        get_quota_history_sync, points_from_providers, should_record, QuotaHistoryPoint,
        CREDITS_WINDOW_ID, MIN_INTERVAL_MS,
    };
    use crate::subscription_usage::{ProviderUsage, QuotaWindow};

    fn window(id: &str, used: Option<f64>) -> QuotaWindow {
        QuotaWindow {
            id: id.into(),
            label: id.into(),
            used_percent: used,
            remaining_percent: used.map(|value| 100.0 - value),
            window_minutes: Some(300),
            resets_at_unix_seconds: None,
            reset_description: None,
            unavailable_reason: None,
        }
    }

    fn provider(id: &str, disabled: bool, status: &str, windows: Vec<QuotaWindow>) -> ProviderUsage {
        ProviderUsage {
            id: id.into(),
            provider: id.into(),
            subtitle: "teste".into(),
            plan: None,
            environment: None,
            status: status.into(),
            windows,
            credits_remaining: None,
            credits_total: None,
            credits_used: None,
            reset_credits_available: None,
            sampled_at_unix_ms: Some(1_700_000_000_000),
            source: "teste".into(),
            unavailable_reason: None,
            disabled,
        }
    }

    fn point(used: Option<f64>, sampled: i64) -> QuotaHistoryPoint {
        QuotaHistoryPoint {
            provider_id: "claude".into(),
            window_id: "session".into(),
            label: "Sessão".into(),
            used_percent: used,
            remaining_percent: used.map(|value| 100.0 - value),
            credits_remaining: None,
            sampled_at_unix_ms: sampled,
        }
    }

    #[test]
    fn skips_disabled_unavailable_and_unmeasured() {
        let points = points_from_providers(
            &[
                provider("claude", false, "available", vec![window("session", Some(20.0))]),
                provider("codex", true, "available", vec![window("session", Some(10.0))]),
                provider("agy", false, "unavailable", vec![window("session", Some(10.0))]),
                provider("empty", false, "available", vec![window("session", None)]),
            ],
            1_700_000_000_000,
        );
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].provider_id, "claude");
        assert_eq!(points[0].used_percent, Some(20.0));
    }

    #[test]
    fn records_credits_series_for_key_providers() {
        let mut openrouter = provider("openrouter", false, "available", vec![]);
        openrouter.credits_remaining = Some(12.5);
        let points = points_from_providers(&[openrouter], 1_700_000_000_000);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].window_id, CREDITS_WINDOW_ID);
        assert_eq!(points[0].credits_remaining, Some(12.5));
    }

    #[test]
    fn never_records_fake_zero_for_missing_reading() {
        let points = points_from_providers(
            &[provider("claude", false, "available", vec![window("session", None)])],
            1_700_000_000_000,
        );
        assert!(points.is_empty());
    }

    #[test]
    fn enforces_minimum_interval_but_keeps_large_jumps() {
        let base = 1_700_000_000_000;
        assert!(should_record(None, None, &point(Some(10.0), base)));
        // 2 min depois, sem variação: ignora.
        assert!(!should_record(
            Some(base),
            Some(10.0),
            &point(Some(10.2), base + 120_000),
        ));
        // 2 min depois, salto de 8 pontos: grava.
        assert!(should_record(
            Some(base),
            Some(10.0),
            &point(Some(18.0), base + 120_000),
        ));
        // Após o piso de 10 min: sempre grava.
        assert!(should_record(
            Some(base),
            Some(10.0),
            &point(Some(10.1), base + MIN_INTERVAL_MS),
        ));
    }

    #[test]
    fn query_filters_by_series() {
        let response = get_quota_history_sync(
            Some("no-such-provider".into()),
            None,
            Some(30),
        );
        assert!(response.points.iter().all(|point| point.provider_id == "no-such-provider"));
        assert_eq!(response.retained_days, super::RETENTION_DAYS as u32);
    }
}
