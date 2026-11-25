//! Leitura direta via Win32 HID (`setupapi.dll` + `hid.dll` + `kernel32.dll`)
//! para dongles 2.4GHz proprietários e periféricos USB/HID no Windows.
//!
//! Implementa E/S assíncrona (`OVERLAPPED` + `CancelIoEx`) com timeouts curtos
//! (100–250ms) para nunca bloquear o shell nem vazar IRPs pendentes na pilha.

use super::{
    classify_kind,
    hid_protocol::{
        anonymize_hid_path_label, build_hidpp10_pairing_name_request, build_hidpp_request,
        build_razer_feature_request, build_stinger_core_batt_feature,
        build_stinger_core_conn_feature, classify_hidpp_reply, corsair_protocol_for_pid,
        hyperx_accepts_collection, interpret_razer_reading, is_logitech_hidpp_collection,
        is_sony_controller_pid, logitech_collection_priority, logitech_hidpp_receiver_key,
        logitech_pid_fallback_name, lookup_hyperx_model, normalize_vendor_prefix,
        parse_corsair_bragi_reply, parse_corsair_slipstream_feature, parse_corsair_void_response,
        parse_hidpp10_pairing_name, parse_hidpp_battery_status, parse_hidpp_battery_voltage,
        parse_hidpp_device_type, parse_hidpp_name_chunk, parse_hidpp_name_length,
        parse_hidpp_ping_ok, parse_hidpp_root_feature, parse_hidpp_unified_battery,
        parse_hyperx_cloud2_hp_battery, parse_hyperx_cloud2_hp_charging,
        parse_hyperx_cloud2_kingston_battery, parse_hyperx_cloud2_kingston_charging,
        parse_hyperx_cloud2_kingston_conn, parse_hyperx_cloud3_battery,
        parse_hyperx_cloud3_charging, parse_hyperx_cloud3s_battery, parse_hyperx_cloud3s_charging,
        parse_hyperx_cloud3s_conn, parse_hyperx_cloud_alpha2_battery,
        parse_hyperx_cloud_alpha_battery, parse_hyperx_cloud_alpha_charging,
        parse_hyperx_cloud_alpha_conn, parse_hyperx_cloud_flight_battery,
        parse_razer_battery_response, parse_razer_charging_response, parse_sony_battery_report,
        parse_steelseries_response, parse_stinger_core_batt_response,
        parse_stinger_core_conn_response, resolve_corsair_name, resolve_razer_name,
        resolve_steelseries_name, steelseries_protocol_for_pid, CorsairProtocol, HidBatteryReading,
        HidppReply, HyperXProtocol, SteelSeriesProtocol, CORSAIR_VID, HIDPP_ERR_CONNECTION_FAILED,
        HIDPP_ERR_RESOURCE_ERROR, HIDPP_ERR_UNKNOWN_DEVICE, HIDPP_FEAT_BATTERY_STATUS,
        HIDPP_FEAT_BATTERY_VOLTAGE, HIDPP_FEAT_DEVICE_NAME, HIDPP_FEAT_ROOT,
        HIDPP_FEAT_UNIFIED_BATTERY, HIDPP_SW_ID, HP_VID, KINGSTON_VID, LOGITECH_VID, RAZER_VID,
        SONY_VID, STEELSERIES_VID,
    },
    PeripheralBatteryDevice, PeripheralKind, PeripheralTransport,
};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use windows::{
    core::{HRESULT, PCWSTR},
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
                SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
                SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
            },
            HumanInterfaceDevice::{
                HidD_FlushQueue, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature,
                HidD_GetHidGuid, HidD_GetInputReport, HidD_GetPreparsedData, HidD_GetProductString,
                HidD_SetFeature, HidD_SetOutputReport, HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS,
                HIDP_STATUS_SUCCESS, PHIDP_PREPARSED_DATA,
            },
        },
        Foundation::{
            CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0,
        },
        Storage::FileSystem::{
            CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_OVERLAPPED,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            Threading::{CreateEventW, ResetEvent, WaitForSingleObject},
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        },
    },
};

const HID_IO_TIMEOUT_MS: u32 = 150;
const HIDPP_PROBE_TIMEOUT_MS: u32 = 140;
const HIDPP_INTER_FRAME_TIMEOUT_MS: u32 = 60;

struct DevInfoGuard(HDEVINFO);
impl Drop for DevInfoGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct PreparsedGuard(PHIDP_PREPARSED_DATA);
impl Drop for PreparsedGuard {
    fn drop(&mut self) {
        if self.0 .0 != 0 {
            unsafe {
                let _ = HidD_FreePreparsedData(self.0);
            }
        }
    }
}

#[derive(Clone)]
struct HidCollection {
    path_wide_z: Vec<u16>,
    path_lower: String,
    vid: u16,
    pid: u16,
    caps: HIDP_CAPS,
    product_string: Option<String>,
}

pub struct HidCollectionResult {
    pub devices: Vec<PeripheralBatteryDevice>,
    pub diagnostics: Vec<String>,
}

pub fn collect() -> HidCollectionResult {
    let mut diagnostics = Vec::new();
    let collections = match enumerate_hid_collections() {
        Ok(list) => list,
        Err(err) => {
            crate::debug::record_failure(
                "peripheral_battery_hid",
                0,
                crate::debug::Failure::new("Falha ao enumerar interfaces HID no Windows.")
                    .stage("enumerate")
                    .detail(err),
            );
            diagnostics.push("Erro ao enumerar interfaces HID via SetupAPI.".to_owned());
            return HidCollectionResult {
                devices: Vec::new(),
                diagnostics,
            };
        }
    };

    for col in &collections {
        if is_tracked_vendor(col.vid) {
            diagnostics.push(format!(
                "VID_{:04X}&PID_{:04X} [{}] UsagePage=0x{:04X} Usage=0x{:04X} In={} Out={} Feat={}",
                col.vid,
                col.pid,
                anonymize_hid_path_label(&col.path_lower),
                col.caps.UsagePage,
                col.caps.Usage,
                col.caps.InputReportByteLength,
                col.caps.OutputReportByteLength,
                col.caps.FeatureReportByteLength,
            ));
        }
    }

    let mut devices = Vec::new();
    let mut seen_ids = HashSet::new();

    collect_logitech_hidpp(&collections, &mut devices, &mut seen_ids, &mut diagnostics);
    collect_hyperx_devices(&collections, &mut devices, &mut seen_ids, &mut diagnostics);
    collect_razer_devices(&collections, &mut devices, &mut seen_ids);
    collect_steelseries_devices(&collections, &mut devices, &mut seen_ids);
    collect_corsair_devices(&collections, &mut devices, &mut seen_ids);
    collect_sony_controllers(&collections, &mut devices, &mut seen_ids);

    HidCollectionResult {
        devices,
        diagnostics,
    }
}

fn is_tracked_vendor(vid: u16) -> bool {
    matches!(
        vid,
        LOGITECH_VID | HP_VID | KINGSTON_VID | RAZER_VID | STEELSERIES_VID | CORSAIR_VID | SONY_VID
    )
}

fn enumerate_hid_collections() -> Result<Vec<HidCollection>, String> {
    let hid_guid = unsafe { HidD_GetHidGuid() };
    let dev_info = unsafe {
        SetupDiGetClassDevsW(
            Some(&hid_guid),
            None,
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }
    .map_err(|e| format!("SetupDiGetClassDevsW: {e}"))?;

    if dev_info.is_invalid() {
        return Err("Handle HID SetupAPI inválido.".to_owned());
    }
    let _dev_info_guard = DevInfoGuard(dev_info);

    let mut list = Vec::new();
    let mut index: u32 = 0;

    loop {
        let mut iface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if unsafe { SetupDiEnumDeviceInterfaces(dev_info, None, &hid_guid, index, &mut iface_data) }
            .is_err()
        {
            break;
        }
        index += 1;

        let mut required = 0u32;
        let _ = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface_data,
                None,
                0,
                Some(&mut required),
                None,
            )
        };
        let min_detail_size = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        if (required as usize) <= min_detail_size || required > 8192 {
            continue;
        }

        let mut aligned = vec![0u32; (required as usize).div_ceil(4)];
        let detail_ptr = aligned
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        unsafe {
            (*detail_ptr).cbSize = min_detail_size as u32;
        }
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface_data,
                Some(detail_ptr),
                required,
                None,
                None,
            )
        }
        .is_err()
        {
            continue;
        }

        // Em SP_DEVICE_INTERFACE_DETAIL_DATA_W, `cbSize` ocupa os primeiros 4 bytes e
        // `DevicePath` (UTF-16) inicia no offset 4 tanto em x86 quanto em x64.
        let path_offset = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
        let raw_bytes =
            unsafe { std::slice::from_raw_parts(aligned.as_ptr().cast::<u8>(), required as usize) };
        if raw_bytes.len() <= path_offset {
            continue;
        }
        let mut path_wide_z: Vec<u16> = raw_bytes[path_offset..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&w| w != 0)
            .collect();
        if path_wide_z.is_empty() {
            continue;
        }
        let path_lower = String::from_utf16_lossy(&path_wide_z).to_ascii_lowercase();
        path_wide_z.push(0);

        // Abre com acesso 0 apenas para consultar VID/PID/Caps sem exigir permissão de I/O exclusivo.
        let Ok(meta_handle) = open_hid_handle(&path_wide_z, 0, false) else {
            continue;
        };

        let mut attrs = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            ..Default::default()
        };
        if !unsafe { HidD_GetAttributes(meta_handle.0, &mut attrs) } {
            continue;
        }
        if !is_tracked_vendor(attrs.VendorID) {
            continue;
        }

        let mut preparsed = PHIDP_PREPARSED_DATA::default();
        if !unsafe { HidD_GetPreparsedData(meta_handle.0, &mut preparsed) } || preparsed.0 == 0 {
            continue;
        }
        let _preparsed_guard = PreparsedGuard(preparsed);

        let mut caps = HIDP_CAPS::default();
        if unsafe { HidP_GetCaps(preparsed, &mut caps) } != HIDP_STATUS_SUCCESS {
            continue;
        }

        let product_string = read_product_string(meta_handle.0);
        list.push(HidCollection {
            path_wide_z,
            path_lower,
            vid: attrs.VendorID,
            pid: attrs.ProductID,
            caps,
            product_string,
        });
    }

    Ok(list)
}

fn open_hid_handle(
    path_wide_z: &[u16],
    desired_access: u32,
    overlapped: bool,
) -> Result<HandleGuard, String> {
    let flags = if overlapped {
        FILE_FLAG_OVERLAPPED
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(0)
    };
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path_wide_z.as_ptr()),
            desired_access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
    }
    .map_err(|e| format!("CreateFileW: {e}"))?;
    Ok(HandleGuard(handle))
}

fn open_io_handle(col: &HidCollection) -> Option<HandleGuard> {
    open_hid_handle(&col.path_wide_z, GENERIC_READ.0 | GENERIC_WRITE.0, true)
        .or_else(|_| open_hid_handle(&col.path_wide_z, GENERIC_READ.0, true))
        .ok()
}

fn open_feature_handle(col: &HidCollection) -> Option<HandleGuard> {
    open_hid_handle(&col.path_wide_z, GENERIC_READ.0 | GENERIC_WRITE.0, false)
        .or_else(|_| open_hid_handle(&col.path_wide_z, 0, false))
        .ok()
}

fn read_product_string(handle: HANDLE) -> Option<String> {
    let mut wide = [0u16; 128];
    let ok = unsafe {
        HidD_GetProductString(
            handle,
            wide.as_mut_ptr().cast(),
            std::mem::size_of_val(&wide) as u32,
        )
    };
    if !ok {
        return None;
    }
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    let text = String::from_utf16_lossy(&wide[..len]).trim().to_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn set_feature(handle: HANDLE, report: &[u8]) -> bool {
    unsafe { HidD_SetFeature(handle, report.as_ptr().cast(), report.len() as u32) }
}

fn get_feature(handle: HANDLE, report_id: u8, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    buf[0] = report_id;
    let ok = unsafe { HidD_GetFeature(handle, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if ok {
        Some(buf)
    } else {
        None
    }
}

fn get_input_report(handle: HANDLE, report_id: u8, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    buf[0] = report_id;
    let ok = unsafe { HidD_GetInputReport(handle, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if ok {
        Some(buf)
    } else {
        None
    }
}

fn create_manual_event() -> Result<HandleGuard, String> {
    let event = unsafe { CreateEventW(None, true, false, None) }
        .map_err(|e| format!("CreateEventW: {e}"))?;
    Ok(HandleGuard(event))
}

fn write_overlapped(
    handle: HANDLE,
    event: HANDLE,
    data: &[u8],
    timeout_ms: u32,
) -> Result<(), String> {
    unsafe { ResetEvent(event) }.map_err(|e| format!("ResetEvent: {e}"))?;
    let mut ov = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let mut written = 0u32;
    match unsafe { WriteFile(handle, Some(data), Some(&mut written), Some(&mut ov)) } {
        Ok(()) => Ok(()),
        Err(err) if err.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
            let wait = unsafe { WaitForSingleObject(event, timeout_ms) };
            if wait == WAIT_OBJECT_0 {
                unsafe { GetOverlappedResult(handle, &ov, &mut written, false) }
                    .map_err(|e| format!("GetOverlappedResult(write): {e}"))
            } else {
                // Cancela a E/S e drena GetOverlappedResult antes de liberar a pilha.
                unsafe {
                    let _ = CancelIoEx(handle, Some(&ov));
                    let mut drained = 0u32;
                    let _ = GetOverlappedResult(handle, &ov, &mut drained, true);
                }
                Err("Timeout na escrita HID.".to_owned())
            }
        }
        Err(err) => Err(format!("WriteFile: {err}")),
    }
}

fn read_overlapped(
    handle: HANDLE,
    event: HANDLE,
    buf: &mut [u8],
    timeout_ms: u32,
) -> Result<usize, String> {
    unsafe { ResetEvent(event) }.map_err(|e| format!("ResetEvent: {e}"))?;
    let mut ov = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let mut read = 0u32;
    match unsafe { ReadFile(handle, Some(buf), Some(&mut read), Some(&mut ov)) } {
        Ok(()) => Ok(read as usize),
        Err(err) if err.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
            let wait = unsafe { WaitForSingleObject(event, timeout_ms) };
            if wait == WAIT_OBJECT_0 {
                unsafe { GetOverlappedResult(handle, &ov, &mut read, false) }
                    .map_err(|e| format!("GetOverlappedResult(read): {e}"))?;
                Ok(read as usize)
            } else {
                unsafe {
                    let _ = CancelIoEx(handle, Some(&ov));
                    let mut drained = 0u32;
                    let _ = GetOverlappedResult(handle, &ov, &mut drained, true);
                }
                Ok(0)
            }
        }
        Err(err) => Err(format!("ReadFile: {err}")),
    }
}

fn write_padded_output(
    handle: HANDLE,
    event: HANDLE,
    payload: &[u8],
    output_len: u16,
    timeout_ms: u32,
) -> Result<(), String> {
    let target_len = (output_len as usize).max(payload.len());
    let mut padded = vec![0u8; target_len];
    padded[..payload.len()].copy_from_slice(payload);
    match write_overlapped(handle, event, &padded, timeout_ms) {
        Ok(()) => Ok(()),
        Err(write_err) => {
            if unsafe { HidD_SetOutputReport(handle, padded.as_ptr().cast(), padded.len() as u32) }
            {
                Ok(())
            } else {
                Err(write_err)
            }
        }
    }
}

fn write_and_read_report(
    handle: HANDLE,
    event: HANDLE,
    caps: &HIDP_CAPS,
    request: &[u8],
    post_write_delay_ms: u64,
    matcher: impl Fn(&[u8]) -> bool,
) -> Option<Vec<u8>> {
    unsafe {
        let _ = HidD_FlushQueue(handle);
    }
    write_padded_output(
        handle,
        event,
        request,
        caps.OutputReportByteLength,
        HID_IO_TIMEOUT_MS,
    )
    .ok()?;
    if post_write_delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(post_write_delay_ms));
    }

    let read_len = (caps.InputReportByteLength as usize).max(64);
    let mut buf = vec![0u8; read_len];
    let deadline = Instant::now() + Duration::from_millis(240);
    for attempt in 0..8 {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let rem_ms = deadline
            .saturating_duration_since(now)
            .as_millis()
            .clamp(35, HID_IO_TIMEOUT_MS as u128) as u32;
        let wait_ms = if attempt == 0 { rem_ms } else { rem_ms.min(60) };
        buf.fill(0);
        let Ok(n) = read_overlapped(handle, event, &mut buf, wait_ms) else {
            break;
        };
        if n == 0 {
            break;
        }
        if matcher(&buf[..n]) {
            buf.truncate(n);
            return Some(buf);
        }
    }
    None
}

fn push_reading(
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
    id: String,
    name: String,
    fallback_kind: PeripheralKind,
    reading: HidBatteryReading,
    base_source: &str,
) {
    if !seen_ids.insert(id.clone()) {
        return;
    }
    let classified = classify_kind(&name);
    let kind = if classified == PeripheralKind::Other {
        fallback_kind
    } else {
        classified
    };
    let source = match reading.discrete_note {
        Some(note) => format!("{base_source} ({note})"),
        None => base_source.to_owned(),
    };
    let (connected, charging, battery_percent) = reading.into_capability();
    devices.push(PeripheralBatteryDevice {
        id,
        name,
        hardware_name: None,
        custom_name: None,
        kind,
        transport: PeripheralTransport::UsbHid,
        connected,
        charging,
        battery_percent,
        source,
    });
}

// ---------------------------------------------------------------------------
// Logitech HID++ 2.0 (VID 0x046D) — LIGHTSPEED (0xC53F), Unifying, Bolt e direto
// ---------------------------------------------------------------------------

fn collect_logitech_hidpp(
    collections: &[HidCollection],
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
    diagnostics: &mut Vec<String>,
) {
    let mut grouped: HashMap<String, Vec<&HidCollection>> = HashMap::new();
    for col in collections {
        if col.vid != LOGITECH_VID {
            continue;
        }
        if !is_logitech_hidpp_collection(
            col.caps.UsagePage,
            col.caps.Usage,
            col.caps.InputReportByteLength,
            col.caps.OutputReportByteLength,
        ) {
            continue;
        }
        let key = logitech_hidpp_receiver_key(&col.path_lower, col.vid, col.pid);
        grouped.entry(key).or_default().push(col);
    }

    for (receiver_key, mut group) in grouped {
        group.sort_by_key(|c| {
            logitech_collection_priority(
                c.caps.UsagePage,
                c.caps.Usage,
                c.caps.InputReportByteLength,
                c.caps.OutputReportByteLength,
            )
        });

        let mut handled_slots = HashSet::new();
        for col in group {
            let (saw_hidpp_reply, found_slots) = probe_logitech_collection(
                col,
                &receiver_key,
                &handled_slots,
                devices,
                seen_ids,
                diagnostics,
            );
            handled_slots.extend(found_slots);
            // Assim que a coleção principal do receptor (ex.: MI_02&Col02 20B) respondeu HID++,
            // não precisa retestar Col01/Col03 irmãs no mesmo dongle.
            if saw_hidpp_reply {
                break;
            }
        }
    }
}

#[derive(Debug)]
enum HidppCallError {
    Protocol(u8),
    Timeout,
    Io,
}

fn hidpp_call(
    handle: HANDLE,
    event: HANDLE,
    caps: &HIDP_CAPS,
    dev_idx: u8,
    feat_idx: u8,
    func_id: u8,
    params: &[u8],
) -> Result<Vec<u8>, HidppCallError> {
    let Some(req) = build_hidpp_request(
        caps.OutputReportByteLength as usize,
        dev_idx,
        feat_idx,
        func_id,
        HIDPP_SW_ID,
        params,
    ) else {
        return Err(HidppCallError::Io);
    };
    unsafe {
        let _ = HidD_FlushQueue(handle);
    }
    write_overlapped(handle, event, &req, HID_IO_TIMEOUT_MS).map_err(|_| HidppCallError::Io)?;

    let read_len = (caps.InputReportByteLength as usize).max(20);
    let mut buf = vec![0u8; read_len];
    let deadline = Instant::now() + Duration::from_millis(HIDPP_PROBE_TIMEOUT_MS as u64);

    for attempt in 0..8 {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining_ms = deadline.saturating_duration_since(now).as_millis() as u32;
        let wait_ms = if attempt == 0 {
            remaining_ms.clamp(25, HIDPP_PROBE_TIMEOUT_MS)
        } else {
            remaining_ms.clamp(20, HIDPP_INTER_FRAME_TIMEOUT_MS)
        };
        buf.fill(0);
        let n =
            read_overlapped(handle, event, &mut buf, wait_ms).map_err(|_| HidppCallError::Io)?;
        if n == 0 {
            return Err(HidppCallError::Timeout);
        }
        match classify_hidpp_reply(&buf[..n], dev_idx, feat_idx, func_id, HIDPP_SW_ID) {
            HidppReply::Data(slice) => return Ok(slice.to_vec()),
            HidppReply::Error(code) => return Err(HidppCallError::Protocol(code)),
            HidppReply::Ignore => continue,
        }
    }
    Err(HidppCallError::Timeout)
}

fn hidpp_resolve_feature(
    handle: HANDLE,
    event: HANDLE,
    caps: &HIDP_CAPS,
    dev_idx: u8,
    feature_id: u16,
) -> Result<Option<u8>, HidppCallError> {
    let bytes = feature_id.to_be_bytes();
    let resp = hidpp_call(
        handle,
        event,
        caps,
        dev_idx,
        HIDPP_FEAT_ROOT as u8,
        0,
        &bytes,
    )?;
    Ok(parse_hidpp_root_feature(&resp))
}

fn hidpp10_read_pairing_name(
    handle: HANDLE,
    event: HANDLE,
    caps: &HIDP_CAPS,
    slot: u8,
) -> Option<String> {
    let req = build_hidpp10_pairing_name_request(caps.OutputReportByteLength as usize, slot)?;
    unsafe {
        let _ = HidD_FlushQueue(handle);
    }
    write_overlapped(handle, event, &req, HID_IO_TIMEOUT_MS).ok()?;
    let read_len = (caps.InputReportByteLength as usize).max(20);
    let mut buf = vec![0u8; read_len];
    for _ in 0..4 {
        buf.fill(0);
        let n = read_overlapped(handle, event, &mut buf, 60).ok()?;
        if n == 0 {
            break;
        }
        if let Some(name) = parse_hidpp10_pairing_name(&buf[..n], slot) {
            return Some(name);
        }
    }
    None
}

fn hidpp_read_device_name(
    handle: HANDLE,
    event: HANDLE,
    caps: &HIDP_CAPS,
    dev_idx: u8,
    fallback_raw: Option<&str>,
    pid: u16,
) -> (String, Option<PeripheralKind>) {
    let mut detected_kind = None;
    if let Ok(Some(feat_name)) =
        hidpp_resolve_feature(handle, event, caps, dev_idx, HIDPP_FEAT_DEVICE_NAME)
    {
        if let Ok(type_resp) = hidpp_call(handle, event, caps, dev_idx, feat_name, 2, &[]) {
            detected_kind = parse_hidpp_device_type(&type_resp);
        }
        if let Ok(len_resp) = hidpp_call(handle, event, caps, dev_idx, feat_name, 0, &[]) {
            if let Some(total_len) = parse_hidpp_name_length(&len_resp) {
                let mut bytes = Vec::with_capacity(total_len);
                let mut offset = 0usize;
                while offset < total_len && offset < 64 {
                    let Ok(chunk_resp) =
                        hidpp_call(handle, event, caps, dev_idx, feat_name, 1, &[offset as u8])
                    else {
                        break;
                    };
                    let chunk = parse_hidpp_name_chunk(&chunk_resp, total_len - bytes.len());
                    if chunk.is_empty() {
                        break;
                    }
                    bytes.extend_from_slice(chunk);
                    offset = bytes.len();
                }
                let text = String::from_utf8_lossy(&bytes).trim().to_owned();
                if !text.is_empty() {
                    return (normalize_vendor_prefix("Logitech", &text), detected_kind);
                }
            }
        }
    }

    if (1..=6).contains(&dev_idx) {
        if let Some(paired) = hidpp10_read_pairing_name(handle, event, caps, dev_idx) {
            return (normalize_vendor_prefix("Logitech", &paired), detected_kind);
        }
    }

    let raw_fallback = fallback_raw
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && !s.eq_ignore_ascii_case("usb receiver")
                && !s.eq_ignore_ascii_case("lightspeed receiver")
        })
        .or_else(|| logitech_pid_fallback_name(pid))
        .unwrap_or("Logitech Wireless");
    (
        normalize_vendor_prefix("Logitech", raw_fallback),
        detected_kind,
    )
}

fn hidpp_query_battery(
    handle: HANDLE,
    event: HANDLE,
    caps: &HIDP_CAPS,
    dev_idx: u8,
) -> Option<(HidBatteryReading, &'static str)> {
    // 1. Unified Battery (0x1004)
    if let Ok(Some(feat_1004)) =
        hidpp_resolve_feature(handle, event, caps, dev_idx, HIDPP_FEAT_UNIFIED_BATTERY)
    {
        let soc_supported = hidpp_call(handle, event, caps, dev_idx, feat_1004, 0, &[])
            .ok()
            .and_then(|r| r.get(5).copied())
            .is_some_and(|flags| (flags & 0x02) != 0);
        if let Ok(status_resp) = hidpp_call(handle, event, caps, dev_idx, feat_1004, 1, &[]) {
            return Some((
                parse_hidpp_unified_battery(&status_resp, soc_supported),
                "Logitech HID++ 2.0 (0x1004)",
            ));
        }
    }

    // 2. Fallback Battery Level Status (0x1000)
    if let Ok(Some(feat_1000)) =
        hidpp_resolve_feature(handle, event, caps, dev_idx, HIDPP_FEAT_BATTERY_STATUS)
    {
        if let Ok(status_resp) = hidpp_call(handle, event, caps, dev_idx, feat_1000, 0, &[]) {
            return Some((
                parse_hidpp_battery_status(&status_resp),
                "Logitech HID++ 2.0 (0x1000)",
            ));
        }
    }

    // 3. Fallback Battery Voltage (0x1001)
    if let Ok(Some(feat_1001)) =
        hidpp_resolve_feature(handle, event, caps, dev_idx, HIDPP_FEAT_BATTERY_VOLTAGE)
    {
        if let Ok(volt_resp) = hidpp_call(handle, event, caps, dev_idx, feat_1001, 0, &[]) {
            return Some((
                parse_hidpp_battery_voltage(&volt_resp),
                "Logitech HID++ 2.0 (0x1001)",
            ));
        }
    }

    None
}

fn probe_logitech_collection(
    col: &HidCollection,
    receiver_key: &str,
    already_handled: &HashSet<u8>,
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
    diagnostics: &mut Vec<String>,
) -> (bool, HashSet<u8>) {
    let Ok(handle) = open_hid_handle(&col.path_wide_z, GENERIC_READ.0 | GENERIC_WRITE.0, true)
    else {
        return (false, HashSet::new());
    };
    let Ok(event) = create_manual_event() else {
        return (false, HashSet::new());
    };
    let mut saw_hidpp_reply = false;
    let mut handled_now = HashSet::new();
    let mut silent_timeouts = 0u8;

    for (nonce_idx, &dev_idx) in [0xFFu8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]
        .iter()
        .enumerate()
    {
        if already_handled.contains(&dev_idx) {
            continue;
        }
        let nonce = 0x41u8.wrapping_add(nonce_idx as u8);
        let ping_res = hidpp_call(
            handle.0,
            event.0,
            &col.caps,
            dev_idx,
            HIDPP_FEAT_ROOT as u8,
            1,
            &[0x00, 0x00, nonce],
        );

        match ping_res {
            Ok(resp) if parse_hidpp_ping_ok(&resp, nonce) => {
                saw_hidpp_reply = true;
                silent_timeouts = 0;
                let Some((reading, source)) =
                    hidpp_query_battery(handle.0, event.0, &col.caps, dev_idx)
                else {
                    // Em receptores (0xFF) o dongle em si não possui bateria — segue para 0x01..=0x06.
                    continue;
                };
                let (name, proto_kind) = hidpp_read_device_name(
                    handle.0,
                    event.0,
                    &col.caps,
                    dev_idx,
                    col.product_string.as_deref(),
                    col.pid,
                );
                let dev_id = format!("hidpp-{receiver_key}-idx{dev_idx:02x}");
                diagnostics.push(format!(
                    "Logitech HID++ VID_{:04X}&PID_{:04X} idx=0x{:02X} -> '{}' ({:?})",
                    col.vid, col.pid, dev_idx, name, reading.percent
                ));
                push_reading(
                    devices,
                    seen_ids,
                    dev_id,
                    name,
                    proto_kind.unwrap_or(PeripheralKind::Mouse),
                    reading,
                    source,
                );
                handled_now.insert(dev_idx);
                if dev_idx == 0xFF {
                    break;
                }
            }
            Ok(_) => {
                saw_hidpp_reply = true;
                silent_timeouts = 0;
            }
            Err(HidppCallError::Protocol(code)) => {
                saw_hidpp_reply = true;
                silent_timeouts = 0;
                if code == HIDPP_ERR_RESOURCE_ERROR && (1..=6).contains(&dev_idx) {
                    // Slot pareado existe no receptor, mas o periférico está dormindo/desligado.
                    let raw_name = hidpp10_read_pairing_name(handle.0, event.0, &col.caps, dev_idx)
                        .or_else(|| logitech_pid_fallback_name(col.pid).map(str::to_owned))
                        .unwrap_or_else(|| format!("Dispositivo Logitech (slot {dev_idx})"));
                    let name = normalize_vendor_prefix("Logitech", &raw_name);
                    let dev_id = format!("hidpp-{receiver_key}-idx{dev_idx:02x}");
                    push_reading(
                        devices,
                        seen_ids,
                        dev_id,
                        name,
                        PeripheralKind::Other,
                        HidBatteryReading::disconnected(
                            "Periférico Logitech pareado desligado ou em suspensão (HID++ 0x09).",
                        ),
                        "Logitech HID++ 2.0",
                    );
                    handled_now.insert(dev_idx);
                } else if matches!(code, HIDPP_ERR_UNKNOWN_DEVICE | HIDPP_ERR_CONNECTION_FAILED) {
                    continue;
                }
            }
            Err(HidppCallError::Timeout) => {
                silent_timeouts += 1;
                // Se nem 0xFF nem o slot 0x01 responderam na mesma coleção, essa coleção não fala HID++.
                if dev_idx == 0x01 && silent_timeouts >= 2 {
                    break;
                }
            }
            Err(HidppCallError::Io) => break,
        }
    }

    (saw_hidpp_reply, handled_now)
}

// ---------------------------------------------------------------------------
// HyperX Wireless (VID 0x03F0 / 0x0951) — incl. Cloud Stinger Core DTS (03F0:0A8E)
// ---------------------------------------------------------------------------

fn collect_hyperx_devices(
    collections: &[HidCollection],
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
    diagnostics: &mut Vec<String>,
) {
    for col in collections {
        let Some(spec) = lookup_hyperx_model(col.vid, col.pid) else {
            continue;
        };
        if !hyperx_accepts_collection(
            spec,
            col.caps.UsagePage,
            col.caps.Usage,
            col.caps.InputReportByteLength,
            col.caps.OutputReportByteLength,
            col.caps.FeatureReportByteLength,
        ) {
            continue;
        }
        let dev_id = format!("hyperx-{:04x}-{:04x}", col.vid, col.pid);
        if seen_ids.contains(&dev_id) {
            continue;
        }

        let name = col
            .product_string
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("hyperx"))
            .map(|s| normalize_vendor_prefix("HyperX", s))
            .unwrap_or_else(|| spec.name.to_owned());

        let reading_opt = match spec.protocol {
            HyperXProtocol::StingerCoreHp => query_stinger_core(col, true, diagnostics),
            HyperXProtocol::StingerCoreKingston => query_stinger_core(col, false, diagnostics),
            HyperXProtocol::Cloud2Hp => query_hyperx_cloud2_hp(col),
            HyperXProtocol::Cloud2Kingston | HyperXProtocol::CloudFlightS => {
                query_hyperx_cloud2_kingston(col, spec.protocol == HyperXProtocol::CloudFlightS)
            }
            HyperXProtocol::CloudAlpha => query_hyperx_cloud_alpha(col),
            HyperXProtocol::CloudFlight => query_hyperx_cloud_flight(col),
            HyperXProtocol::Cloud3 => query_hyperx_cloud3(col),
            HyperXProtocol::Cloud3S | HyperXProtocol::Mix2 | HyperXProtocol::Stinger3 => {
                query_hyperx_cloud3s(col, spec.protocol)
            }
            HyperXProtocol::CloudAlpha2 => query_hyperx_cloud_alpha2(col),
        };

        if let Some(reading) = reading_opt {
            push_reading(
                devices,
                seen_ids,
                dev_id,
                name,
                PeripheralKind::Headset,
                reading,
                &format!("HyperX HID ({:04X}:{:04X})", col.vid, col.pid),
            );
        }
    }
}

fn query_stinger_core(
    col: &HidCollection,
    strict_hp_header: bool,
    diagnostics: &mut Vec<String>,
) -> Option<HidBatteryReading> {
    let handle = open_feature_handle(col)?;
    let feat_len = col.caps.FeatureReportByteLength as usize;

    // Passo 1: verifica se o headset está conectado ao dongle USB.
    let conn_cmd = build_stinger_core_conn_feature(feat_len);
    if !set_feature(handle.0, &conn_cmd) {
        diagnostics.push(format!(
            "HyperX {:04X}:{:04X} SetFeature(conn) falhou",
            col.vid, col.pid
        ));
        return None;
    }
    std::thread::sleep(Duration::from_millis(20));

    let conn_resp = get_feature(handle.0, 0xFF, feat_len)?;
    match parse_stinger_core_conn_response(&conn_resp) {
        Some(true) => {}
        Some(false) => {
            return Some(HidBatteryReading::disconnected(
                "Headset desligado ou fora do alcance do dongle USB.",
            ));
        }
        None => return None,
    }

    // Passo 2: envia comando de leitura de tensão/carga e aguarda resposta pronta (até ~200ms).
    let batt_cmd = build_stinger_core_batt_feature(feat_len);
    if !set_feature(handle.0, &batt_cmd) {
        diagnostics.push(format!(
            "HyperX {:04X}:{:04X} SetFeature(batt) falhou",
            col.vid, col.pid
        ));
        return Some(HidBatteryReading::unavailable(
            None,
            "Não foi possível solicitar leitura de bateria ao dongle HyperX.",
        ));
    }

    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(20));
        let Some(resp) = get_feature(handle.0, 0xFF, feat_len) else {
            continue;
        };
        if let Some(reading) = parse_stinger_core_batt_response(&resp, strict_hp_header) {
            return Some(reading);
        }
        if !strict_hp_header {
            break;
        }
    }

    Some(HidBatteryReading::unavailable(
        None,
        "O dongle HyperX não concluiu a telemetria de bateria a tempo.",
    ))
}

fn query_hyperx_cloud2_hp(col: &HidCollection) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;
    let batt_resp = write_and_read_report(
        handle.0,
        event.0,
        &col.caps,
        &[0x06, 0xFF, 0xBB, 0x02],
        40,
        |r| parse_hyperx_cloud2_hp_battery(r).is_some(),
    )?;
    let pct = parse_hyperx_cloud2_hp_battery(&batt_resp)?;
    let charging = write_and_read_report(
        handle.0,
        event.0,
        &col.caps,
        &[0x06, 0xFF, 0xBB, 0x03],
        30,
        |r| parse_hyperx_cloud2_hp_charging(r).is_some(),
    )
    .and_then(|r| parse_hyperx_cloud2_hp_charging(&r));
    if pct == 0 && charging != Some(true) {
        return Some(HidBatteryReading::disconnected(
            "Headset HyperX desconectado ou sem resposta válida do dongle.",
        ));
    }
    if pct == 0 {
        return Some(HidBatteryReading::unavailable(
            charging,
            "Headset em carregamento; percentual exato omitido pelo dongle.",
        ));
    }
    Some(HidBatteryReading::measured(pct, charging))
}

fn build_kingston_cloud2_cmd(cmd: u8) -> [u8; 16] {
    [
        0x06, 0x00, 0x02, 0x00, 0x9A, 0x00, 0x00, 0x68, 0x4A, 0x8E, 0x0A, 0x00, 0x00, 0x00, 0xBB,
        cmd,
    ]
}

fn query_hyperx_cloud2_kingston(
    col: &HidCollection,
    check_conn: bool,
) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;
    let _ = get_input_report(
        handle.0,
        0x06,
        (col.caps.InputReportByteLength as usize).max(64),
    );

    if check_conn {
        let conn_req = build_kingston_cloud2_cmd(0x01);
        if let Some(resp) =
            write_and_read_report(handle.0, event.0, &col.caps, &conn_req, 30, |r| {
                parse_hyperx_cloud2_kingston_conn(r).is_some()
            })
        {
            if parse_hyperx_cloud2_kingston_conn(&resp) == Some(false) {
                return Some(HidBatteryReading::disconnected(
                    "Headset HyperX desligado ou fora do alcance.",
                ));
            }
        }
    }

    let batt_req = build_kingston_cloud2_cmd(0x02);
    let batt_resp = write_and_read_report(handle.0, event.0, &col.caps, &batt_req, 40, |r| {
        parse_hyperx_cloud2_kingston_battery(r).is_some()
    })?;
    let pct = parse_hyperx_cloud2_kingston_battery(&batt_resp)?;
    let chg_req = build_kingston_cloud2_cmd(0x03);
    let charging = write_and_read_report(handle.0, event.0, &col.caps, &chg_req, 30, |r| {
        parse_hyperx_cloud2_kingston_charging(r).is_some()
    })
    .and_then(|r| parse_hyperx_cloud2_kingston_charging(&r));

    if pct == 0 && charging != Some(true) {
        return Some(HidBatteryReading::disconnected(
            "Headset HyperX desligado ou em suspensão.",
        ));
    }
    Some(HidBatteryReading::measured(pct, charging))
}

fn query_hyperx_cloud_alpha(col: &HidCollection) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;
    let conn_resp =
        write_and_read_report(handle.0, event.0, &col.caps, &[0x21, 0xBB, 0x03], 20, |r| {
            parse_hyperx_cloud_alpha_conn(r).is_some()
        })?;
    if parse_hyperx_cloud_alpha_conn(&conn_resp) == Some(false) {
        return Some(HidBatteryReading::disconnected(
            "Headset HyperX Cloud Alpha desligado ou desconectado.",
        ));
    }

    let charging =
        write_and_read_report(handle.0, event.0, &col.caps, &[0x21, 0xBB, 0x0C], 20, |r| {
            parse_hyperx_cloud_alpha_charging(r).is_some()
        })
        .and_then(|r| parse_hyperx_cloud_alpha_charging(&r));

    if charging == Some(true) {
        return Some(HidBatteryReading::unavailable(
            Some(true),
            "Carregando via USB; nível percentual indisponível durante a carga.",
        ));
    }

    let batt_resp =
        write_and_read_report(handle.0, event.0, &col.caps, &[0x21, 0xBB, 0x0B], 20, |r| {
            parse_hyperx_cloud_alpha_battery(r).is_some()
        })?;
    let pct = parse_hyperx_cloud_alpha_battery(&batt_resp)?;
    Some(HidBatteryReading::measured(pct, charging))
}

fn query_hyperx_cloud_flight(col: &HidCollection) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;
    let resp = write_and_read_report(handle.0, event.0, &col.caps, &[0x21, 0xFF, 0x05], 20, |r| {
        parse_hyperx_cloud_flight_battery(r).is_some()
    })?;
    parse_hyperx_cloud_flight_battery(&resp)
}

fn query_hyperx_cloud3(col: &HidCollection) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;
    let resp = write_and_read_report(handle.0, event.0, &col.caps, &[0x66, 0x89], 20, |r| {
        parse_hyperx_cloud3_battery(r).is_some()
    })?;
    let pct = parse_hyperx_cloud3_battery(&resp)?;
    let charging = write_and_read_report(handle.0, event.0, &col.caps, &[0x66, 0x8A], 20, |r| {
        parse_hyperx_cloud3_charging(r).is_some()
    })
    .and_then(|r| parse_hyperx_cloud3_charging(&r));
    Some(HidBatteryReading::measured(pct, charging))
}

fn query_hyperx_cloud3s(col: &HidCollection, proto: HyperXProtocol) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;
    let expected_online = if proto == HyperXProtocol::Stinger3 {
        0x02
    } else {
        0x01
    };

    if matches!(proto, HyperXProtocol::Mix2 | HyperXProtocol::Stinger3) {
        if let Some(conn_resp) = write_and_read_report(
            handle.0,
            event.0,
            &col.caps,
            &[0x0C, 0x02, 0x03, 0x01, 0x00, 0x02],
            20,
            |r| parse_hyperx_cloud3s_conn(r, expected_online).is_some(),
        ) {
            if parse_hyperx_cloud3s_conn(&conn_resp, expected_online) == Some(false) {
                return Some(HidBatteryReading::disconnected(
                    "Headset HyperX desligado ou fora do alcance.",
                ));
            }
        }
    }

    let batt_resp = write_and_read_report(
        handle.0,
        event.0,
        &col.caps,
        &[0x0C, 0x02, 0x03, 0x01, 0x00, 0x06],
        20,
        |r| parse_hyperx_cloud3s_battery(r).is_some(),
    )?;
    let pct = parse_hyperx_cloud3s_battery(&batt_resp)?;
    let charging = if proto == HyperXProtocol::Cloud3S {
        write_and_read_report(
            handle.0,
            event.0,
            &col.caps,
            &[0x0C, 0x02, 0x03, 0x01, 0x00, 0x48],
            20,
            |r| parse_hyperx_cloud3s_charging(r).is_some(),
        )
        .and_then(|r| parse_hyperx_cloud3s_charging(&r))
    } else {
        None
    };
    if pct == 0 && charging != Some(true) {
        return Some(HidBatteryReading::disconnected(
            "Headset HyperX desligado ou em suspensão.",
        ));
    }
    Some(HidBatteryReading::measured(pct, charging))
}

fn query_hyperx_cloud_alpha2(col: &HidCollection) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;
    let resp = write_and_read_report(handle.0, event.0, &col.caps, &[0x50, 0x02], 20, |r| {
        parse_hyperx_cloud_alpha2_battery(r).is_some()
    })?;
    parse_hyperx_cloud_alpha2_battery(&resp)
}

// ---------------------------------------------------------------------------
// Razer (VID 0x1532)
// ---------------------------------------------------------------------------

fn collect_razer_devices(
    collections: &[HidCollection],
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
) {
    for col in collections {
        if col.vid != RAZER_VID || col.caps.FeatureReportByteLength < 90 {
            continue;
        }
        let dev_id = format!("razer-{:04x}", col.pid);
        if seen_ids.contains(&dev_id) {
            continue;
        }
        let Some(handle) = open_feature_handle(col) else {
            continue;
        };
        let feat_len = (col.caps.FeatureReportByteLength as usize).max(91);
        let bat_req = build_razer_feature_request(feat_len, 0x07, 0x80, 0x02);
        if !set_feature(
            handle.0,
            &bat_req[..col.caps.FeatureReportByteLength as usize],
        ) {
            continue;
        }
        std::thread::sleep(Duration::from_millis(12));
        let Some(bat_resp) = get_feature(handle.0, 0x00, col.caps.FeatureReportByteLength as usize)
        else {
            continue;
        };
        let Some(raw_pct) = parse_razer_battery_response(&bat_resp) else {
            continue;
        };

        let chg_req = build_razer_feature_request(feat_len, 0x07, 0x84, 0x02);
        let charging = if set_feature(
            handle.0,
            &chg_req[..col.caps.FeatureReportByteLength as usize],
        ) {
            std::thread::sleep(Duration::from_millis(6));
            get_feature(handle.0, 0x00, col.caps.FeatureReportByteLength as usize)
                .and_then(|r| parse_razer_charging_response(&r))
        } else {
            None
        };

        let reading = interpret_razer_reading(Some(raw_pct), charging);
        let name = resolve_razer_name(col.pid, col.product_string.as_deref());
        push_reading(
            devices,
            seen_ids,
            dev_id,
            name,
            PeripheralKind::Mouse,
            reading,
            "Razer HID (0x1532)",
        );
    }
}

// ---------------------------------------------------------------------------
// SteelSeries (VID 0x1038)
// ---------------------------------------------------------------------------

fn collect_steelseries_devices(
    collections: &[HidCollection],
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
) {
    for col in collections {
        if col.vid != STEELSERIES_VID {
            continue;
        }
        let proto = steelseries_protocol_for_pid(col.pid);
        let dev_id = format!("steelseries-{:04x}", col.pid);
        if seen_ids.contains(&dev_id) {
            continue;
        }

        let reading = match proto {
            SteelSeriesProtocol::NovaDiscrete
            | SteelSeriesProtocol::NovaPercent
            | SteelSeriesProtocol::Nova5
            | SteelSeriesProtocol::Arctis7Plus => {
                if col.caps.UsagePage != 0xFFC0
                    || col.caps.Usage != 0x0001
                    || col.caps.OutputReportByteLength < 2
                    || col.caps.InputReportByteLength < 4
                {
                    continue;
                }
                let Some(handle) = open_io_handle(col) else {
                    continue;
                };
                let Ok(event) = create_manual_event() else {
                    continue;
                };
                write_and_read_report(handle.0, event.0, &col.caps, &[0x00, 0xB0], 10, |r| {
                    parse_steelseries_response(proto, r).is_some()
                })
                .and_then(|r| parse_steelseries_response(proto, &r))
            }
            SteelSeriesProtocol::Arctis7Legacy => {
                if col.caps.UsagePage != 0xFF43
                    || col.caps.Usage != 0x0202
                    || col.caps.OutputReportByteLength < 2
                    || col.caps.InputReportByteLength < 8
                {
                    continue;
                }
                let Some(handle) = open_io_handle(col) else {
                    continue;
                };
                let Ok(event) = create_manual_event() else {
                    continue;
                };
                write_and_read_report(handle.0, event.0, &col.caps, &[0x06, 0x18], 10, |r| {
                    parse_steelseries_response(proto, r).is_some()
                })
                .and_then(|r| parse_steelseries_response(proto, &r))
            }
            SteelSeriesProtocol::MouseFeature => {
                if col.caps.UsagePage < 0xFF00 || col.caps.FeatureReportByteLength < 4 {
                    continue;
                }
                let Some(handle) = open_feature_handle(col) else {
                    continue;
                };
                let feat_len = (col.caps.FeatureReportByteLength as usize).max(4);
                let mut req = vec![0u8; feat_len];
                req[0] = 0x00;
                req[1] = 0x90;
                if !set_feature(handle.0, &req) {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(10));
                get_feature(handle.0, 0x00, feat_len)
                    .and_then(|r| parse_steelseries_response(proto, &r))
            }
        };

        let Some(reading) = reading else {
            continue;
        };
        let fallback_kind = match proto {
            SteelSeriesProtocol::MouseFeature => PeripheralKind::Mouse,
            _ => PeripheralKind::Headset,
        };
        let name = resolve_steelseries_name(col.pid, col.product_string.as_deref());
        push_reading(
            devices,
            seen_ids,
            dev_id,
            name,
            fallback_kind,
            reading,
            "SteelSeries HID (0x1038)",
        );
    }
}

// ---------------------------------------------------------------------------
// Corsair (VID 0x1B1C)
// ---------------------------------------------------------------------------

fn collect_corsair_devices(
    collections: &[HidCollection],
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
) {
    for col in collections {
        if col.vid != CORSAIR_VID {
            continue;
        }
        let proto = corsair_protocol_for_pid(col.pid);
        let dev_id = format!("corsair-{:04x}", col.pid);
        if seen_ids.contains(&dev_id) {
            continue;
        }

        let reading = match proto {
            CorsairProtocol::Void => {
                if col.caps.UsagePage != 0xFFC5
                    || col.caps.Usage != 0x0001
                    || col.caps.OutputReportByteLength < 2
                    || col.caps.InputReportByteLength < 5
                {
                    continue;
                }
                let Some(handle) = open_io_handle(col) else {
                    continue;
                };
                let Ok(event) = create_manual_event() else {
                    continue;
                };
                write_and_read_report(handle.0, event.0, &col.caps, &[0xC9, 0x64], 15, |r| {
                    parse_corsair_void_response(r).is_some()
                })
                .and_then(|r| parse_corsair_void_response(&r))
            }
            CorsairProtocol::Bragi => {
                if col.caps.UsagePage != 0xFF42
                    || col.caps.Usage != 0x0001
                    || col.caps.OutputReportByteLength < 64
                    || col.caps.InputReportByteLength < 64
                {
                    continue;
                }
                query_corsair_bragi(col)
            }
            CorsairProtocol::SlipstreamFeature => {
                if col.caps.UsagePage < 0xFF00 || col.caps.FeatureReportByteLength < 4 {
                    continue;
                }
                let Some(handle) = open_feature_handle(col) else {
                    continue;
                };
                let feat_len = (col.caps.FeatureReportByteLength as usize).max(4);
                let mut req = vec![0u8; feat_len];
                req[0] = 0x00;
                req[1] = 0x08;
                req[2] = 0x02;
                if !set_feature(handle.0, &req) {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(10));
                get_feature(handle.0, 0x00, feat_len)
                    .and_then(|r| parse_corsair_slipstream_feature(&r))
            }
        };

        let Some(reading) = reading else {
            continue;
        };
        let fallback_kind = match proto {
            CorsairProtocol::SlipstreamFeature => PeripheralKind::Mouse,
            CorsairProtocol::Void | CorsairProtocol::Bragi => PeripheralKind::Headset,
        };
        let name = resolve_corsair_name(col.pid, col.product_string.as_deref());
        push_reading(
            devices,
            seen_ids,
            dev_id,
            name,
            fallback_kind,
            reading,
            "Corsair HID (0x1B1C)",
        );
    }
}

fn query_corsair_bragi(col: &HidCollection) -> Option<HidBatteryReading> {
    let handle = open_io_handle(col)?;
    let event = create_manual_event().ok()?;

    // 0x09 = headset atrás do receptor; 0x08 = conexão direta/cabo USB.
    for target in [0x09u8, 0x08u8] {
        let lvl_req = [0x02, target, 0x02, 0x0F];
        let Some(lvl_resp) =
            write_and_read_report(handle.0, event.0, &col.caps, &lvl_req, 10, |r| {
                parse_corsair_bragi_reply(r, 0x02).is_some()
            })
        else {
            continue;
        };
        match parse_corsair_bragi_reply(&lvl_resp, 0x02)? {
            Ok(tenths) if tenths <= 1000 => {
                let chg_req = [0x02, target, 0x02, 0x10];
                let charging =
                    write_and_read_report(handle.0, event.0, &col.caps, &chg_req, 10, |r| {
                        parse_corsair_bragi_reply(r, 0x02).is_some()
                    })
                    .and_then(|r| parse_corsair_bragi_reply(&r, 0x02))
                    .and_then(Result::ok)
                    .map(|st| st == 2);
                return Some(HidBatteryReading::measured((tenths / 10) as u8, charging));
            }
            _ => continue,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Sony DualSense / DualShock 4 (VID 0x054C)
// ---------------------------------------------------------------------------

fn collect_sony_controllers(
    collections: &[HidCollection],
    devices: &mut Vec<PeripheralBatteryDevice>,
    seen_ids: &mut HashSet<String>,
) {
    for col in collections {
        if col.vid != SONY_VID || col.caps.UsagePage != 0x0001 || col.caps.Usage != 0x0005 {
            continue;
        }
        let Some((model_name, is_dualsense)) = is_sony_controller_pid(col.pid) else {
            continue;
        };
        let dev_id = format!("sony-{:04x}", col.pid);
        if seen_ids.contains(&dev_id) {
            continue;
        }

        let Some(handle) = open_hid_handle(&col.path_wide_z, GENERIC_READ.0, true).ok() else {
            continue;
        };
        let input_len = (col.caps.InputReportByteLength as usize).max(78);

        let mut reading_opt = None;
        if let Ok(event) = create_manual_event() {
            let mut buf = vec![0u8; input_len];
            if let Ok(n) = read_overlapped(handle.0, event.0, &mut buf, 80) {
                if n > 0 {
                    reading_opt = parse_sony_battery_report(&buf[..n], is_dualsense);
                }
            }
        }

        if reading_opt.is_none() {
            let report_ids: &[u8] = if is_dualsense {
                &[0x31, 0x01]
            } else {
                &[0x11, 0x01]
            };
            for &rid in report_ids {
                if let Some(buf) = get_input_report(handle.0, rid, input_len) {
                    if let Some(reading) = parse_sony_battery_report(&buf, is_dualsense) {
                        reading_opt = Some(reading);
                        break;
                    }
                }
            }
        }

        if let Some(reading) = reading_opt {
            push_reading(
                devices,
                seen_ids,
                dev_id,
                model_name.to_owned(),
                PeripheralKind::Controller,
                reading,
                "Sony HID (DualSense/DualShock 4)",
            );
        }
    }
}
