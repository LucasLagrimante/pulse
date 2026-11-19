//! Telemetria de hardware do módulo Desempenho no macOS.
//!
//! Temperaturas vêm dos `Components` do `sysinfo` (SMC/IOKit). Nome e VRAM da
//! GPU exigiriam `system_profiler SPDisplaysDataType` (lento, ~1 s) ou IOKit;
//! por enquanto ficam indisponíveis com motivo.
// TODO(macos): validar em hardware (Apple Silicon e Intel).

use super::{
    hottest_sensor, sanitize_id, valid_celsius, Capability, PerformanceDetails, ThermalSensor,
};

const MAX_SENSORS: usize = 16;

pub fn get_performance_details() -> PerformanceDetails {
    let components = sysinfo::Components::new_with_refreshed_list();
    let mut thermal_sensors: Vec<ThermalSensor> = components
        .iter()
        .filter_map(|component| {
            let celsius = component
                .temperature()
                .map(f64::from)
                .and_then(valid_celsius)?;
            let label = component.label().trim().to_owned();
            (!label.is_empty()).then(|| ThermalSensor {
                id: format!("smc-{}", sanitize_id(&label)),
                label,
                celsius,
                source: "SMC".into(),
            })
        })
        .collect();
    thermal_sensors.truncate(MAX_SENSORS);
    let temperature_celsius = if thermal_sensors.is_empty() {
        Capability {
            value: None,
            unavailable_reason: Some("Nenhum sensor térmico respondeu via SMC".into()),
        }
    } else {
        hottest_sensor(&thermal_sensors)
    };
    let reason = "Indisponível no macOS por enquanto (exige leitura via IOKit)";
    PerformanceDetails {
        gpu: Capability {
            value: None,
            unavailable_reason: Some(reason.into()),
        },
        vram_bytes: Capability {
            value: None,
            unavailable_reason: Some(reason.into()),
        },
        temperature_celsius,
        thermal_sensors,
    }
}
