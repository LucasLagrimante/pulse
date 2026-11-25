//! Coleta de bateria de periféricos no macOS via `ioreg`.

use super::{
    classify_kind, measured_percent, normalize_devices, now_unix_ms, PeripheralBatteryDevice,
    PeripheralBatterySnapshot, PeripheralTransport,
};
use std::process::Command;

pub fn collect() -> PeripheralBatterySnapshot {
    let output = Command::new("ioreg")
        .args(["-r", "-d", "1", "-k", "BatteryPercent"])
        .output();

    let output = match output {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            return PeripheralBatterySnapshot {
                devices: Vec::new(),
                unavailable_reason: Some(format!(
                    "ioreg encerrou com código {:?}.",
                    out.status.code()
                )),
                platform_note: None,
                collected_at_unix_ms: now_unix_ms(),
            };
        }
        Err(err) => {
            return PeripheralBatterySnapshot {
                devices: Vec::new(),
                unavailable_reason: Some(format!("Falha ao executar ioreg: {err}")),
                platform_note: None,
                collected_at_unix_ms: now_unix_ms(),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_pct: Option<u8> = None;
    let mut index = 0usize;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("+-o ") {
            flush_entry(
                &mut devices,
                &mut current_name,
                &mut current_pct,
                &mut index,
            );
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("\"Product\" = ") {
            let clean = rest.trim_matches('"').trim();
            if !clean.is_empty() {
                current_name = Some(clean.to_owned());
            }
        } else if let Some(rest) = trimmed.strip_prefix("\"BatteryPercent\" = ") {
            if let Ok(pct) = rest.trim().parse::<u8>() {
                current_pct = Some(pct);
            }
        }
    }
    flush_entry(
        &mut devices,
        &mut current_name,
        &mut current_pct,
        &mut index,
    );

    PeripheralBatterySnapshot {
        devices: normalize_devices(devices),
        unavailable_reason: None,
        platform_note: Some("Leitura via IOKit (ioreg BatteryPercent).".to_owned()),
        collected_at_unix_ms: now_unix_ms(),
    }
}

fn flush_entry(
    devices: &mut Vec<PeripheralBatteryDevice>,
    name: &mut Option<String>,
    pct: &mut Option<u8>,
    index: &mut usize,
) {
    if let (Some(product), Some(level)) = (name.take(), pct.take()) {
        *index += 1;
        devices.push(PeripheralBatteryDevice {
            id: format!("ioreg-{index}-{}", product.to_lowercase()),
            kind: classify_kind(&product),
            name: product,
            hardware_name: None,
            custom_name: None,
            transport: PeripheralTransport::Ioreg,
            connected: true,
            charging: None,
            battery_percent: measured_percent(level),
            source: "macOS IOKit (ioreg)".to_owned(),
        });
    } else {
        *name = None;
        *pct = None;
    }
}
