//! Coleta de bateria de periféricos no Linux via `/sys/class/power_supply/*`.
//!
//! O kernel Linux (hid-logitech-hidpp, hid-sony, hid-playstation, bluez/batt)
//! expõe periféricos sem fio com `scope == Device`, evitando confundir com a
//! bateria principal do notebook (`scope == System` ou `BAT*`).

use super::{
    classify_kind, measured_percent, normalize_devices, now_unix_ms, unavailable_percent,
    PeripheralBatteryDevice, PeripheralBatterySnapshot, PeripheralTransport,
};
use std::{fs, path::Path};

const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";

pub fn collect() -> PeripheralBatterySnapshot {
    let root = Path::new(POWER_SUPPLY_DIR);
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) => {
            return PeripheralBatterySnapshot {
                devices: Vec::new(),
                unavailable_reason: Some(format!(
                    "Não foi possível acessar {POWER_SUPPLY_DIR}: {err}"
                )),
                platform_note: None,
                collected_at_unix_ms: now_unix_ms(),
            };
        }
    };

    let mut devices = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let dir_name = entry.file_name().to_string_lossy().into_owned();

        let scope = read_trimmed(&path.join("scope")).unwrap_or_default();
        if !scope.eq_ignore_ascii_case("device") {
            continue;
        }

        let name = read_trimmed(&path.join("model_name"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| dir_name.clone());

        let online = read_trimmed(&path.join("online"))
            .map(|v| v != "0")
            .unwrap_or(true);

        let status = read_trimmed(&path.join("status")).unwrap_or_default();
        let charging = match status.to_lowercase().as_str() {
            "charging" => Some(true),
            "discharging" | "not charging" | "full" => Some(false),
            _ => None,
        };

        let battery_percent =
            match read_trimmed(&path.join("capacity")).and_then(|raw| raw.parse::<u8>().ok()) {
                Some(pct) => measured_percent(pct),
                None => unavailable_percent("O driver não reportou o percentual em capacity."),
            };

        devices.push(PeripheralBatteryDevice {
            id: format!("sysfs-{dir_name}"),
            kind: classify_kind(&name),
            name,
            hardware_name: None,
            custom_name: None,
            transport: PeripheralTransport::Sysfs,
            connected: online,
            charging,
            battery_percent,
            source: format!("Linux power_supply ({dir_name})"),
        });
    }

    PeripheralBatterySnapshot {
        devices: normalize_devices(devices),
        unavailable_reason: None,
        platform_note: Some(
            "Leitura direta de /sys/class/power_supply (scope=Device: Bluetooth, HID++, DualSense e afins)."
                .to_owned(),
        ),
        collected_at_unix_ms: now_unix_ms(),
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
