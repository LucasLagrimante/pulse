//! Coleta de bateria de periféricos no Windows (Fases 1 e 2).
//!
//! Combina quatro fontes nativas sem depender de softwares de fabricante:
//! 1. SetupAPI (`DEVPKEY_Bluetooth_Battery`) — fones/periféricos Bluetooth Classic e BLE
//!    já agregados pelo Windows.
//! 2. WinRT Bluetooth LE GATT (`0x180F` -> `0x2A19`) — mouses e teclados BLE modernos
//!    que não populam o nó PnP.
//! 3. Protocolo HID / USB 2.4GHz direto (`hid_windows`) — dongles Logitech LIGHTSPEED/
//!    Unifying/Bolt (HID++ 2.0), HyperX Wireless (incl. `03F0:0A8E`), Razer, SteelSeries,
//!    Corsair e DualSense/DualShock 4.
//! 4. XInput (`XInputGetBatteryInformation`) — controles Xbox sem fio ou USB.

use super::{
    classify_kind, hid_windows, measured_percent, normalize_devices, now_unix_ms,
    unavailable_percent, PeripheralBatteryDevice, PeripheralBatterySnapshot, PeripheralKind,
    PeripheralTransport,
};
use std::collections::HashSet;
use windows::{
    core::GUID,
    Devices::{
        Bluetooth::{
            BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice,
            GenericAttributeProfile::GattCommunicationStatus,
        },
        Enumeration::DeviceInformation,
    },
    Storage::Streams::DataReader,
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
                SetupDiGetDevicePropertyW, DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO,
                SP_DEVINFO_DATA,
            },
            Properties::{
                DEVPKEY_Device_ContainerId, DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName,
                DEVPROPTYPE, DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID,
                DEVPROP_TYPE_STRING,
            },
        },
        Foundation::{DEVPROPKEY, ERROR_SUCCESS},
        UI::Input::XboxController::{
            XInputGetBatteryInformation, XInputGetState, BATTERY_DEVTYPE_GAMEPAD,
            BATTERY_LEVEL_EMPTY, BATTERY_LEVEL_FULL, BATTERY_LEVEL_LOW, BATTERY_LEVEL_MEDIUM,
            BATTERY_TYPE_ALKALINE, BATTERY_TYPE_DISCONNECTED, BATTERY_TYPE_NIMH,
            BATTERY_TYPE_WIRED, XINPUT_BATTERY_INFORMATION, XINPUT_STATE,
        },
    },
};

/// `{104EA319-6EE2-4701-BD47-8DDBF425BBE5}` PID 2 — percentual de bateria exposto
/// pelo stack Bluetooth do Windows (mesma fonte das Configurações do sistema).
const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_values(
        0x104ea319,
        0x6ee2,
        0x4701,
        [0xbd, 0x47, 0x8d, 0xdb, 0xf4, 0x25, 0xbb, 0xe5],
    ),
    pid: 2,
};

/// `{83DA6326-97A6-4088-9453-A1923F573B29}` PID 15 — `DEVPKEY_Device_IsConnected`.
const DEVPKEY_DEVICE_IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_values(
        0x83da6326,
        0x97a6,
        0x4088,
        [0x94, 0x53, 0xa1, 0x92, 0x3f, 0x57, 0x3b, 0x29],
    ),
    pid: 15,
};

/// UUID padrão do serviço GATT Battery (`0x180F`).
const GATT_BATTERY_SERVICE_UUID: GUID = GUID::from_values(
    0x0000180f,
    0x0000,
    0x1000,
    [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
);

/// UUID padrão da característica GATT Battery Level (`0x2A19`).
const GATT_BATTERY_LEVEL_UUID: GUID = GUID::from_values(
    0x00002a19,
    0x0000,
    0x1000,
    [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
);

#[allow(dead_code)]
pub fn collect() -> PeripheralBatterySnapshot {
    collect_with_diagnostics().0
}

pub fn collect_with_diagnostics() -> (PeripheralBatterySnapshot, Vec<String>) {
    let mut devices = Vec::new();
    let mut notes = Vec::new();

    // Coleta HID 2.4GHz primeiro para manter telemetria nativa detalhada na deduplicação.
    let mut hid_result = hid_windows::collect();
    devices.append(&mut hid_result.devices);

    match collect_pnp_bluetooth() {
        Ok(mut pnp_devices) => devices.append(&mut pnp_devices),
        Err(err) => notes.push(format!("PnP Bluetooth: {err}")),
    }

    match collect_ble_gatt() {
        Ok(mut ble_devices) => devices.append(&mut ble_devices),
        Err(err) => notes.push(format!("BLE GATT: {err}")),
    }

    devices.extend(collect_xinput());

    let devices = normalize_devices(devices);
    let platform_note = if notes.is_empty() {
        Some(
            "Fontes ativas: USB 2.4GHz / HID (Logitech HID++, HyperX, Razer, SteelSeries, Corsair, DualSense), Bluetooth Classic (PnP), Bluetooth LE (GATT) e XInput."
                .to_owned(),
        )
    } else {
        Some(format!(
            "Algumas fontes responderam com ressalvas: {}",
            notes.join(" · ")
        ))
    };

    (
        PeripheralBatterySnapshot {
            devices,
            unavailable_reason: None,
            platform_note,
            collected_at_unix_ms: now_unix_ms(),
        },
        hid_result.diagnostics,
    )
}

fn collect_pnp_bluetooth() -> Result<Vec<PeripheralBatteryDevice>, String> {
    let dev_info =
        unsafe { SetupDiGetClassDevsW(None, None, None, DIGCF_ALLCLASSES | DIGCF_PRESENT) }
            .map_err(|e| format!("SetupDiGetClassDevsW falhou: {e}"))?;

    if dev_info.is_invalid() {
        return Err("Handle PnP inválido.".to_owned());
    }

    struct DevInfoGuard(HDEVINFO);
    impl Drop for DevInfoGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
    let _guard = DevInfoGuard(dev_info);

    let mut found = Vec::new();
    let mut seen_containers = HashSet::new();
    let mut index: u32 = 0;

    loop {
        let mut dev_data = SP_DEVINFO_DATA {
            cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        if unsafe { SetupDiEnumDeviceInfo(dev_info, index, &mut dev_data) }.is_err() {
            break;
        }
        index += 1;

        let Some(battery) = read_devprop_u8(dev_info, &dev_data, &DEVPKEY_BLUETOOTH_BATTERY) else {
            continue;
        };

        let container_id = read_devprop_guid(dev_info, &dev_data, &DEVPKEY_Device_ContainerId);
        if let Some(cid) = container_id.as_ref() {
            if !seen_containers.insert(cid.clone()) {
                continue;
            }
        }

        let name = read_devprop_string(dev_info, &dev_data, &DEVPKEY_Device_FriendlyName)
            .or_else(|| read_devprop_string(dev_info, &dev_data, &DEVPKEY_Device_DeviceDesc))
            .unwrap_or_else(|| format!("Dispositivo Bluetooth #{index}"));

        let connected =
            read_devprop_bool(dev_info, &dev_data, &DEVPKEY_DEVICE_IS_CONNECTED).unwrap_or(true);

        let id = container_id
            .map(|cid| format!("bt-pnp-{cid}"))
            .unwrap_or_else(|| format!("bt-pnp-{index}-{}", name.to_lowercase()));

        found.push(PeripheralBatteryDevice {
            id,
            kind: classify_kind(&name),
            name,
            hardware_name: None,
            custom_name: None,
            transport: PeripheralTransport::BluetoothClassic,
            connected,
            charging: None,
            battery_percent: measured_percent(battery),
            source: "Windows PnP (Bluetooth)".to_owned(),
        });
    }

    Ok(found)
}

fn read_devprop_u8(dev_info: HDEVINFO, dev_data: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<u8> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut buffer = [0u8; 4];
    let mut required = 0u32;
    let res = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            Some(&mut buffer),
            Some(&mut required),
            0,
        )
    };
    if res.is_ok() && prop_type == DEVPROP_TYPE_BYTE && required >= 1 {
        Some(buffer[0])
    } else {
        None
    }
}

fn read_devprop_bool(
    dev_info: HDEVINFO,
    dev_data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<bool> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut buffer = [0u8; 4];
    let mut required = 0u32;
    let res = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            Some(&mut buffer),
            Some(&mut required),
            0,
        )
    };
    if res.is_ok() && prop_type == DEVPROP_TYPE_BOOLEAN && required >= 1 {
        Some(buffer[0] != 0)
    } else {
        None
    }
}

fn read_devprop_guid(
    dev_info: HDEVINFO,
    dev_data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<String> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut buffer = [0u8; 16];
    let mut required = 0u32;
    let res = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            Some(&mut buffer),
            Some(&mut required),
            0,
        )
    };
    if res.is_ok() && prop_type == DEVPROP_TYPE_GUID && required == 16 {
        let mut hex = String::with_capacity(32);
        for b in buffer {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{b:02x}");
        }
        Some(hex)
    } else {
        None
    }
}

fn read_devprop_string(
    dev_info: HDEVINFO,
    dev_data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<String> {
    let mut prop_type = DEVPROPTYPE::default();
    let mut required = 0u32;
    let _ = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            None,
            Some(&mut required),
            0,
        )
    };
    if !(2..=4096).contains(&required) || prop_type != DEVPROP_TYPE_STRING {
        return None;
    }
    let mut buffer = vec![0u8; required as usize];
    let res = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            Some(&mut buffer),
            Some(&mut required),
            0,
        )
    };
    if res.is_err() {
        return None;
    }
    let wide: Vec<u16> = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_le_bytes(*chunk))
        .take_while(|&ch| ch != 0)
        .collect();
    let text = String::from_utf16_lossy(&wide).trim().to_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Aguarda uma `IAsyncOperation` WinRT de forma bloqueante com timeout curto,
/// evitando travar a coleta caso um dispositivo BLE não responda.
macro_rules! wait_winrt {
    ($op:expr) => {{
        let op = $op;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        loop {
            match op.Status() {
                Ok(status) if status.0 == 0 => {
                    // AsyncStatus::Started == 0
                    if std::time::Instant::now() >= deadline {
                        let _ = op.Cancel();
                        break Err(windows::core::Error::new(
                            windows::Win32::Foundation::E_FAIL,
                            "Tempo esgotado aguardando resposta Bluetooth LE",
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(_) => break op.GetResults(),
                Err(err) => break Err(err),
            }
        }
    }};
}

fn collect_ble_gatt() -> Result<Vec<PeripheralBatteryDevice>, String> {
    let selector = BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(
        BluetoothConnectionStatus::Connected,
    )
    .map_err(|e| format!("Selector BLE indisponível: {e}"))?;

    let devices_op = DeviceInformation::FindAllAsyncAqsFilter(&selector)
        .map_err(|e| format!("Enumeração BLE falhou: {e}"))?;
    let devices_info =
        wait_winrt!(devices_op).map_err(|e| format!("Leitura da lista BLE falhou: {e}"))?;

    let mut found = Vec::new();
    let count = devices_info.Size().unwrap_or(0);

    for i in 0..count {
        let Ok(info) = devices_info.GetAt(i) else {
            continue;
        };
        let Ok(device_id) = info.Id() else {
            continue;
        };
        let fallback_name = info
            .Name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_else(|_| "Periférico BLE".to_owned());

        let Ok(ble_op) = BluetoothLEDevice::FromIdAsync(&device_id) else {
            continue;
        };
        let Ok(ble_device) = wait_winrt!(ble_op) else {
            continue;
        };

        let name = ble_device
            .Name()
            .map(|s| s.to_string_lossy())
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(fallback_name);

        let connected = ble_device
            .ConnectionStatus()
            .map(|st| st == BluetoothConnectionStatus::Connected)
            .unwrap_or(false);
        if !connected {
            continue;
        }

        let battery_percent = match read_gatt_battery_percent(&ble_device) {
            Ok(Some(pct)) => measured_percent(pct),
            Ok(None) => {
                unavailable_percent("Dispositivo BLE não expõe o serviço GATT Battery (0x180F).")
            }
            Err(reason) => unavailable_percent(reason),
        };

        let addr = ble_device.BluetoothAddress().unwrap_or(0);
        found.push(PeripheralBatteryDevice {
            id: format!("ble-{addr:012x}"),
            kind: classify_kind(&name),
            name,
            hardware_name: None,
            custom_name: None,
            transport: PeripheralTransport::BluetoothLe,
            connected: true,
            charging: None,
            battery_percent,
            source: "Bluetooth LE (GATT 0x180F)".to_owned(),
        });
    }

    Ok(found)
}

fn read_gatt_battery_percent(ble_device: &BluetoothLEDevice) -> Result<Option<u8>, String> {
    let services_op = ble_device
        .GetGattServicesForUuidWithCacheModeAsync(
            GATT_BATTERY_SERVICE_UUID,
            BluetoothCacheMode::Cached,
        )
        .map_err(|e| format!("Falha ao consultar serviço GATT: {e}"))?;
    let services_res =
        wait_winrt!(services_op).map_err(|e| format!("Falha ao aguardar serviço GATT: {e}"))?;

    if services_res.Status().ok() != Some(GattCommunicationStatus::Success) {
        return Ok(None);
    }
    let Ok(services) = services_res.Services() else {
        return Ok(None);
    };
    if services.Size().unwrap_or(0) == 0 {
        return Ok(None);
    }
    let service = services
        .GetAt(0)
        .map_err(|e| format!("Falha ao abrir serviço GATT: {e}"))?;

    let chars_op = service
        .GetCharacteristicsForUuidWithCacheModeAsync(
            GATT_BATTERY_LEVEL_UUID,
            BluetoothCacheMode::Cached,
        )
        .map_err(|e| format!("Falha ao consultar característica 0x2A19: {e}"))?;
    let chars_res = wait_winrt!(chars_op)
        .map_err(|e| format!("Falha ao aguardar característica 0x2A19: {e}"))?;

    if chars_res.Status().ok() != Some(GattCommunicationStatus::Success) {
        return Ok(None);
    }
    let Ok(chars) = chars_res.Characteristics() else {
        return Ok(None);
    };
    if chars.Size().unwrap_or(0) == 0 {
        return Ok(None);
    }
    let characteristic = chars
        .GetAt(0)
        .map_err(|e| format!("Falha ao abrir característica 0x2A19: {e}"))?;

    // Tenta cache primeiro; se falhar, faz leitura direta no rádio.
    let read_res = characteristic
        .ReadValueWithCacheModeAsync(BluetoothCacheMode::Cached)
        .and_then(|op| wait_winrt!(op))
        .or_else(|_| {
            characteristic
                .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)
                .and_then(|op| wait_winrt!(op))
        })
        .map_err(|e| format!("Falha ao ler nível de bateria BLE: {e}"))?;

    if read_res.Status().ok() != Some(GattCommunicationStatus::Success) {
        return Err("O dispositivo BLE recusou a leitura do nível de bateria.".to_owned());
    }
    let value = read_res
        .Value()
        .map_err(|e| format!("Buffer de bateria BLE ausente: {e}"))?;
    let reader =
        DataReader::FromBuffer(&value).map_err(|e| format!("Falha ao ler buffer BLE: {e}"))?;
    let len = reader.UnconsumedBufferLength().unwrap_or(0);
    if len == 0 {
        return Ok(None);
    }
    let byte = reader
        .ReadByte()
        .map_err(|e| format!("Falha ao extrair byte de bateria BLE: {e}"))?;
    Ok(Some(byte))
}

fn collect_xinput() -> Vec<PeripheralBatteryDevice> {
    let mut devices = Vec::new();
    for slot in 0u32..4 {
        let mut state = XINPUT_STATE::default();
        let state_err = unsafe { XInputGetState(slot, &mut state) };
        if state_err != ERROR_SUCCESS.0 {
            continue;
        }

        let mut info = XINPUT_BATTERY_INFORMATION::default();
        let batt_err =
            unsafe { XInputGetBatteryInformation(slot, BATTERY_DEVTYPE_GAMEPAD, &mut info) };
        if batt_err != ERROR_SUCCESS.0 {
            continue;
        }
        if info.BatteryType == BATTERY_TYPE_DISCONNECTED {
            continue;
        }

        let (battery_percent, charging, source) = match info.BatteryType {
            BATTERY_TYPE_WIRED => (
                unavailable_percent("Conectado via cabo USB (sem telemetria de bateria)."),
                None,
                "XInput (USB)".to_owned(),
            ),
            BATTERY_TYPE_ALKALINE | BATTERY_TYPE_NIMH => {
                // XInput expõe apenas 4 degraus discretos — informamos explicitamente na fonte.
                let pct = match info.BatteryLevel {
                    BATTERY_LEVEL_EMPTY => Some(10),
                    BATTERY_LEVEL_LOW => Some(25),
                    BATTERY_LEVEL_MEDIUM => Some(60),
                    BATTERY_LEVEL_FULL => Some(95),
                    _ => None,
                };
                let cap = match pct {
                    Some(v) => measured_percent(v),
                    None => unavailable_percent("Nível de bateria XInput desconhecido."),
                };
                (cap, Some(false), "XInput (nível discreto)".to_owned())
            }
            _ => (
                unavailable_percent("Tipo de bateria XInput não informado pelo driver."),
                None,
                "XInput".to_owned(),
            ),
        };

        devices.push(PeripheralBatteryDevice {
            id: format!("xinput-slot-{slot}"),
            name: format!("Controle Xbox #{}", slot + 1),
            hardware_name: None,
            custom_name: None,
            kind: PeripheralKind::Controller,
            transport: PeripheralTransport::Xinput,
            connected: true,
            charging,
            battery_percent,
            source,
        });
    }
    devices
}
