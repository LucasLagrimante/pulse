use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInventory {
    pub input: Vec<AudioDevice>,
    pub output: Vec<AudioDevice>,
    pub input_default_id: Option<String>,
    pub output_default_id: Option<String>,
    pub unavailable_reason: Option<String>,
}

/// Uma entrada do mixer de volume: sessões de áudio agrupadas por app
/// (igual ao mixer de volume do Windows 11, que agrupa por executável).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMixerApp {
    /// Chave estável para comandos (`chrome.exe`, `spotify.exe` ou `system`).
    pub app_name: String,
    pub display_name: String,
    pub process_ids: Vec<u32>,
    pub session_count: u32,
    /// Nível 0.0..=1.0 (nunca NaN, nunca fora da faixa).
    pub volume: f32,
    pub muted: bool,
    pub is_system_sounds: bool,
    /// Data URI PNG do ícone do executável, quando o shell fornece.
    /// Ausência = fallback visual no frontend, nunca ícone fictício.
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioMixerInventory {
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub master_volume: Option<f32>,
    pub master_muted: Option<bool>,
    pub apps: Vec<AudioMixerApp>,
    pub unavailable_reason: Option<String>,
}

#[cfg(windows)]
mod windows_impl {
    use super::{AudioDevice, AudioDeviceInventory, AudioMixerApp, AudioMixerInventory};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use windows::core::{Interface, GUID, HRESULT, PCWSTR, PWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, S_OK};
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE, IAudioSessionControl,
        IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED, STGM_READ,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const RPC_E_CHANGED_MODE: HRESULT = HRESULT(0x8001_0106u32 as i32);
    const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    #[repr(C)]
    pub struct IPolicyConfig_Vtbl {
        base__: windows::core::IUnknown_Vtbl,
        // IPolicyConfig's methods between IUnknown and SetDefaultEndpoint.
        get_mix_format: usize,
        get_device_format: usize,
        reset_device_format: usize,
        set_device_format: usize,
        get_processing_period: usize,
        set_processing_period: usize,
        get_share_mode: usize,
        set_share_mode: usize,
        get_property_value: usize,
        set_property_value: usize,
        set_default_endpoint:
            unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, i32) -> HRESULT,
        set_endpoint_visibility: usize,
    }

    windows::core::imp::define_interface!(
        IPolicyConfig,
        IPolicyConfig_Vtbl,
        0xf8679f50_850a_41cf_9c72_430f290290c8
    );

    impl IPolicyConfig {
        unsafe fn set_default_endpoint(&self, id: PCWSTR, role: i32) -> windows::core::Result<()> {
            (windows::core::Interface::vtable(self).set_default_endpoint)(
                windows::core::Interface::as_raw(self),
                id,
                role,
            )
            .ok()
        }
    }

    struct ComGuard(bool);

    impl ComGuard {
        fn initialize() -> Result<Self, String> {
            // Tauri commands normally run on a fresh worker thread. If a caller
            // already initialized that thread with another apartment model, COM
            // is still usable for these calls and must not be uninitialized here.
            let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if result.is_ok() {
                Ok(Self(true))
            } else if result == RPC_E_CHANGED_MODE {
                Ok(Self(false))
            } else {
                Err(format!("Falha ao inicializar o Core Audio: {result}"))
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn copy_pwstr_and_free(value: windows::core::PWSTR) -> Result<String, String> {
        if value.is_null() {
            return Err("O Windows retornou um identificador de áudio vazio.".into());
        }
        let result = unsafe { value.to_string() }
            .map_err(|error| format!("Não foi possível ler o texto do dispositivo: {error}"));
        unsafe {
            CoTaskMemFree(Some(value.as_ptr() as *const core::ffi::c_void));
        }
        result
    }

    fn friendly_name(device: &IMMDevice) -> Result<String, String> {
        let store = unsafe { device.OpenPropertyStore(STGM_READ) }
            .map_err(|error| format!("Não foi possível ler o nome do dispositivo: {error}"))?;
        let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }
            .map_err(|error| format!("Não foi possível ler o nome do dispositivo: {error}"))?;
        let text = unsafe { PropVariantToStringAlloc(&value) }.map_err(|error| {
            format!("Não foi possível converter o nome do dispositivo: {error}")
        })?;
        copy_pwstr_and_free(text)
    }

    fn endpoint_id(device: &IMMDevice) -> Result<String, String> {
        let id = unsafe { device.GetId() }.map_err(|error| {
            format!("Não foi possível ler o identificador do dispositivo: {error}")
        })?;
        copy_pwstr_and_free(id)
    }

    fn enumerate_flow(
        enumerator: &IMMDeviceEnumerator,
        flow: EDataFlow,
    ) -> Result<(Vec<AudioDevice>, Option<String>), String> {
        let collection = unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }
            .map_err(|error| {
                format!("Não foi possível enumerar os dispositivos de áudio: {error}")
            })?;
        let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }
            .ok()
            .and_then(|device| endpoint_id(&device).ok());
        let count = unsafe { collection.GetCount() }.map_err(|error| {
            format!("Não foi possível contar os dispositivos de áudio: {error}")
        })?;
        let mut devices = Vec::with_capacity(count as usize);
        for index in 0..count {
            let device = unsafe { collection.Item(index) }.map_err(|error| {
                format!("Não foi possível ler um dispositivo de áudio: {error}")
            })?;
            let id = endpoint_id(&device)?;
            let name =
                friendly_name(&device).unwrap_or_else(|_| "Dispositivo de áudio sem nome".into());
            devices.push(AudioDevice {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name,
            });
        }
        Ok((devices, default_id))
    }

    pub fn list() -> AudioDeviceInventory {
        let guard = match ComGuard::initialize() {
            Ok(guard) => guard,
            Err(reason) => {
                return AudioDeviceInventory {
                    unavailable_reason: Some(reason),
                    ..Default::default()
                }
            }
        };
        let enumerator = match unsafe {
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        } {
            Ok(enumerator) => enumerator,
            Err(error) => {
                return AudioDeviceInventory {
                    unavailable_reason: Some(format!("O Core Audio não está disponível: {error}")),
                    ..Default::default()
                }
            }
        };
        let result = match (
            enumerate_flow(&enumerator, eCapture),
            enumerate_flow(&enumerator, eRender),
        ) {
            (Ok((input, input_default_id)), Ok((output, output_default_id))) => {
                AudioDeviceInventory {
                    input,
                    output,
                    input_default_id,
                    output_default_id,
                    unavailable_reason: None,
                }
            }
            (input, output) => AudioDeviceInventory {
                unavailable_reason: Some(format!(
                    "Não foi possível enumerar todos os dispositivos (entrada: {}; saída: {}).",
                    input.err().unwrap_or_else(|| "ok".into()),
                    output.err().unwrap_or_else(|| "ok".into())
                )),
                ..Default::default()
            },
        };
        drop(guard);
        result
    }

    pub fn set_default(direction: &str, device_id: &str) -> Result<AudioDeviceInventory, String> {
        let inventory = list();
        if let Some(reason) = inventory.unavailable_reason.as_deref() {
            return Err(reason.into());
        }
        let devices = match direction {
            "input" => &inventory.input,
            "output" => &inventory.output,
            _ => return Err("Tipo de dispositivo inválido; use entrada ou saída.".into()),
        };
        if !devices.iter().any(|device| device.id == device_id) {
            return Err("O dispositivo selecionado não está mais disponível.".into());
        }

        let guard = ComGuard::initialize()?;
        let policy: IPolicyConfig =
            unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }.map_err(
                |error| format!("O Windows não permitiu alterar o dispositivo padrão: {error}"),
            )?;
        let wide_id: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
        let mut last_error = None;
        let mut changed = false;
        for role in [eConsole, eMultimedia, eCommunications] {
            match unsafe { policy.set_default_endpoint(PCWSTR(wide_id.as_ptr()), role.0) } {
                Ok(()) => changed = true,
                Err(error) => last_error = Some(error),
            }
        }
        drop(guard);
        if !changed {
            return Err(format!(
                "O Windows não permitiu alterar o dispositivo padrão: {}",
                last_error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "erro desconhecido".into())
            ));
        }
        Ok(list())
    }

    fn clamp_level(level: f32) -> f32 {
        if level.is_nan() {
            return 0.0;
        }
        level.clamp(0.0, 1.0)
    }

    /// Caminho completo do executável (ex: `C:\...\chrome.exe`) a partir
    /// do PID, via OpenProcess + QueryFullProcessImageNameW. Retorna None
    /// se o processo já morreu ou não permite consulta (PID 0, acesso negado).
    fn process_exe_path(pid: u32) -> Option<PathBuf> {
        if pid == 0 {
            return None;
        }
        let handle: HANDLE =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
        };
        unsafe {
            let _ = CloseHandle(handle);
        }
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        let path = PathBuf::from(path.trim());
        if path.as_os_str().is_empty() {
            None
        } else {
            Some(path)
        }
    }

    struct RawSession {
        key: String,
        app_name: String,
        display_name: String,
        exe_path: Option<PathBuf>,
        pid: u32,
        volume: f32,
        muted: bool,
        is_system: bool,
    }

    fn read_session(control: &IAudioSessionControl) -> Option<RawSession> {
        let simple: ISimpleAudioVolume = control.cast().ok()?;
        let session2: IAudioSessionControl2 = control.cast().ok()?;
        let volume = unsafe { simple.GetMasterVolume() }.ok().map(clamp_level)?;
        let muted = unsafe { simple.GetMute() }
            .map(|mute| mute.as_bool())
            .unwrap_or(false);
        let is_system = unsafe { session2.IsSystemSoundsSession() } == S_OK;
        let pid = unsafe { session2.GetProcessId() }.unwrap_or(0);
        let display = unsafe { session2.GetDisplayName() }
            .ok()
            .and_then(|name| copy_pwstr_and_free(name).ok())
            .unwrap_or_default();
        let exe_path = if is_system {
            None
        } else {
            process_exe_path(pid)
        };
        let exe = exe_path.as_ref().and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        });
        let (key, app_name, display_name) = if is_system {
            (
                "system".to_string(),
                "system".to_string(),
                if display.is_empty() {
                    "Sons do sistema".to_string()
                } else {
                    display
                },
            )
        } else if let Some(exe) = exe {
            let fallback = exe.clone();
            (
                exe.to_lowercase(),
                exe,
                if display.is_empty() { fallback } else { display },
            )
        } else if pid != 0 {
            (
                format!("processo-{pid}"),
                format!("processo-{pid}"),
                if display.is_empty() {
                    format!("Processo {pid}")
                } else {
                    display
                },
            )
        } else {
            return None;
        };
        Some(RawSession {
            key,
            app_name,
            display_name,
            exe_path,
            pid,
            volume,
            muted,
            is_system,
        })
    }

    fn merge_sessions(raw: Vec<RawSession>) -> Vec<AudioMixerApp> {
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, AudioMixerApp> = HashMap::new();
        for session in raw {
            let entry = groups.entry(session.key.clone()).or_insert_with(|| {
                order.push(session.key.clone());
                AudioMixerApp {
                    app_name: session.app_name.clone(),
                    display_name: session.display_name.clone(),
                    process_ids: Vec::new(),
                    session_count: 0,
                    volume: session.volume,
                    muted: session.muted,
                    is_system_sounds: session.is_system,
                    icon: None,
                }
            });
            if entry.display_name.is_empty() && !session.display_name.is_empty() {
                entry.display_name = session.display_name.clone();
            }
            if session.pid != 0 && !entry.process_ids.contains(&session.pid) {
                entry.process_ids.push(session.pid);
            }
            entry.session_count += 1;
            entry.muted = entry.muted && session.muted;
            entry.is_system_sounds = entry.is_system_sounds || session.is_system;
        }
        for app in groups.values_mut() {
            app.process_ids.sort_unstable();
        }
        let mut apps: Vec<AudioMixerApp> = order
            .into_iter()
            .filter_map(|key| groups.remove(&key))
            .collect();
        // Sons do sistema por último, demais apps em ordem alfabética,
        // igual à apresentação do mixer do Windows.
        apps.sort_by(|a, b| match (a.is_system_sounds, b.is_system_sounds) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a
                .display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase()),
        });
        apps
    }

    fn resolve_render_device(
        enumerator: &IMMDeviceEnumerator,
        device_id: Option<&str>,
    ) -> Result<(IMMDevice, String, String), String> {
        let device = match device_id.filter(|id| !id.is_empty()) {
            Some(id) => {
                let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
                unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
                    .or_else(|_| unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) })
                    .map_err(|error| {
                        format!("Não foi possível abrir o dispositivo de saída: {error}")
                    })?
            }
            None => unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(|error| {
                    format!("Nenhum dispositivo de saída padrão no Windows: {error}")
                })?,
        };
        let id = endpoint_id(&device)?;
        let name = friendly_name(&device).unwrap_or_else(|_| "Dispositivo de saída".into());
        Ok((device, id, name))
    }

    fn create_enumerator() -> Result<(ComGuard, IMMDeviceEnumerator), String> {
        let guard = ComGuard::initialize()?;
        let enumerator =
            unsafe { CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| format!("O Core Audio não está disponível: {error}"))?;
        Ok((guard, enumerator))
    }

    pub fn mixer(device_id: Option<&str>) -> AudioMixerInventory {
        let unavailable = |reason: String| AudioMixerInventory {
            unavailable_reason: Some(reason),
            ..Default::default()
        };
        let (_guard, enumerator) = match create_enumerator() {
            Ok(pair) => pair,
            Err(reason) => return unavailable(reason),
        };
        let (device, resolved_id, resolved_name) =
            match resolve_render_device(&enumerator, device_id) {
                Ok(found) => found,
                Err(reason) => return unavailable(reason),
            };
        let manager: IAudioSessionManager2 = match unsafe { device.Activate(CLSCTX_ALL, None) } {
            Ok(manager) => manager,
            Err(error) => {
                return unavailable(format!(
                    "O Windows não permitiu ler as sessões de áudio: {error}"
                ));
            }
        };
        let session_list = match unsafe { manager.GetSessionEnumerator() } {
            Ok(list) => list,
            Err(error) => {
                return unavailable(format!(
                    "O Windows não permitiu enumerar as sessões de áudio: {error}"
                ));
            }
        };
        let count = unsafe { session_list.GetCount() }.unwrap_or(0);
        let mut raw = Vec::new();
        for index in 0..count {
            let control = match unsafe { session_list.GetSession(index) } {
                Ok(control) => control,
                Err(_) => continue,
            };
            if let Some(session) = read_session(&control) {
                raw.push(session);
            }
        }
        // O volume geral é opcional: se o endpoint não responder, o mixer
        // por app continua funcionando (nunca zero fictício: usa None).
        let endpoint: Result<IAudioEndpointVolume, _> =
            unsafe { device.Activate(CLSCTX_ALL, None) };
        let (master_volume, master_muted) = match endpoint {
            Ok(endpoint) => (
                unsafe { endpoint.GetMasterVolumeLevelScalar() }
                    .ok()
                    .map(clamp_level),
                unsafe { endpoint.GetMute() }.ok().map(|mute| mute.as_bool()),
            ),
            Err(_) => (None, None),
        };
        // Caminho do executável da primeira sessão de cada app, para o ícone.
        let mut first_path: HashMap<String, PathBuf> = HashMap::new();
        for session in &raw {
            if let Some(path) = session.exe_path.as_ref() {
                first_path
                    .entry(session.key.clone())
                    .or_insert_with(|| path.clone());
            }
        }
        let mut apps = merge_sessions(raw);
        // Ícones com o cache compartilhado da Inicialização: baratos no
        // polling do mixer. Sem ícone = None (fallback no frontend).
        for app in &mut apps {
            let key = if app.is_system_sounds {
                "system".to_string()
            } else {
                app.app_name.to_lowercase()
            };
            if let Some(path) = first_path.get(&key) {
                app.icon = crate::app_icons::icon_for_path(path);
            }
        }
        AudioMixerInventory {
            device_id: Some(resolved_id),
            device_name: Some(resolved_name),
            master_volume,
            master_muted,
            apps,
            unavailable_reason: None,
        }
    }

    fn each_matching_session(
        device_id: Option<&str>,
        app_name: &str,
        mut apply: impl FnMut(&ISimpleAudioVolume) -> windows::core::Result<()>,
    ) -> Result<u32, String> {
        let (_guard, enumerator) =
            create_enumerator().map_err(|reason: String| reason)?;
        let (device, _, _) = resolve_render_device(&enumerator, device_id)?;
        let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| format!("O Windows não permitiu ler as sessões de áudio: {error}"))?;
        let session_list = unsafe { manager.GetSessionEnumerator() }
            .map_err(|error| format!("O Windows não permitiu enumerar as sessões de áudio: {error}"))?;
        let count = unsafe { session_list.GetCount() }
            .map_err(|error| format!("O Windows não permitiu contar as sessões de áudio: {error}"))?;
        let wanted = app_name.to_lowercase();
        let mut matched = 0u32;
        for index in 0..count {
            let control: IAudioSessionControl = match unsafe { session_list.GetSession(index) } {
                Ok(control) => control,
                Err(_) => continue,
            };
            let Some(session) = read_session(&control) else {
                continue;
            };
            if session.key != wanted {
                continue;
            }
            let simple: ISimpleAudioVolume = control
                .cast()
                .map_err(|error| format!("O Windows não permitiu ajustar o volume: {error}"))?;
            apply(&simple)
                .map_err(|error| format!("O Windows não permitiu ajustar o volume: {error}"))?;
            matched += 1;
        }
        if matched == 0 {
            return Err(
                "Nenhuma sessão de áudio encontrada para esse app. Ele pode ter sido fechado.".into(),
            );
        }
        Ok(matched)
    }

    pub fn set_mixer_app_volume(
        device_id: Option<&str>,
        app_name: &str,
        volume: f32,
    ) -> Result<AudioMixerInventory, String> {
        let level = clamp_level(volume);
        each_matching_session(device_id, app_name, |simple| unsafe {
            simple.SetMasterVolume(level, std::ptr::null())
        })?;
        Ok(mixer(device_id))
    }

    pub fn set_mixer_app_muted(
        device_id: Option<&str>,
        app_name: &str,
        muted: bool,
    ) -> Result<AudioMixerInventory, String> {
        each_matching_session(device_id, app_name, |simple| unsafe {
            simple.SetMute(muted, std::ptr::null())
        })?;
        Ok(mixer(device_id))
    }

    pub fn set_mixer_master(
        device_id: Option<&str>,
        volume: Option<f32>,
        muted: Option<bool>,
    ) -> Result<AudioMixerInventory, String> {
        if volume.is_none() && muted.is_none() {
            return Err("Nada para ajustar no volume geral.".into());
        }
        let (_guard, enumerator) =
            create_enumerator().map_err(|reason: String| reason)?;
        let (device, _, _) = resolve_render_device(&enumerator, device_id)?;
        let endpoint: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| format!("O Windows não permitiu ajustar o volume geral: {error}"))?;
        if let Some(level) = volume {
            unsafe { endpoint.SetMasterVolumeLevelScalar(clamp_level(level), std::ptr::null()) }
                .map_err(|error| format!("O Windows não permitiu ajustar o volume geral: {error}"))?;
        }
        if let Some(mute) = muted {
            unsafe { endpoint.SetMute(mute, std::ptr::null()) }
                .map_err(|error| format!("O Windows não permitiu ajustar o volume geral: {error}"))?;
        }
        Ok(mixer(device_id))
    }
}

#[tauri::command]
pub fn get_audio_devices() -> AudioDeviceInventory {
    #[cfg(windows)]
    {
        windows_impl::list()
    }
    #[cfg(not(windows))]
    AudioDeviceInventory {
        unavailable_reason: Some(
            "A troca de dispositivos de áudio exige o Windows Core Audio.".into(),
        ),
        ..Default::default()
    }
}

pub fn set_default_audio_device_sync(
    direction: &str,
    device_id: &str,
) -> Result<AudioDeviceInventory, String> {
    #[cfg(windows)]
    {
        windows_impl::set_default(direction, device_id)
    }
    #[cfg(not(windows))]
    {
        let _ = (direction, device_id);
        Err("A troca de dispositivos de áudio exige o Windows Core Audio.".into())
    }
}

#[tauri::command]
pub fn set_default_audio_device(
    app: tauri::AppHandle,
    direction: String,
    device_id: String,
) -> Result<AudioDeviceInventory, String> {
    let result = set_default_audio_device_sync(&direction, &device_id)?;
    crate::tray::update_tray_menu(&app);
    Ok(result)
}

#[allow(dead_code)]
fn mixer_unavailable() -> AudioMixerInventory {
    AudioMixerInventory {
        unavailable_reason: Some(
            "O mixer de volume por app exige o Windows Core Audio.".into(),
        ),
        ..Default::default()
    }
}

/// Sessões de áudio da saída atual (ou do `device_id` pedido), agrupadas por
/// app, mais o volume geral do endpoint. Recolhido por padrão no flyout.
#[tauri::command]
pub fn get_audio_mixer(device_id: Option<String>) -> AudioMixerInventory {
    #[cfg(windows)]
    {
        windows_impl::mixer(device_id.as_deref())
    }
    #[cfg(not(windows))]
    {
        let _ = device_id;
        mixer_unavailable()
    }
}

#[tauri::command]
pub fn set_audio_mixer_app_volume(
    device_id: Option<String>,
    app_name: String,
    volume: f32,
) -> Result<AudioMixerInventory, String> {
    #[cfg(windows)]
    {
        windows_impl::set_mixer_app_volume(device_id.as_deref(), &app_name, volume)
    }
    #[cfg(not(windows))]
    {
        let _ = (device_id, app_name, volume);
        Err("O mixer de volume por app exige o Windows Core Audio.".into())
    }
}

#[tauri::command]
pub fn set_audio_mixer_app_muted(
    device_id: Option<String>,
    app_name: String,
    muted: bool,
) -> Result<AudioMixerInventory, String> {
    #[cfg(windows)]
    {
        windows_impl::set_mixer_app_muted(device_id.as_deref(), &app_name, muted)
    }
    #[cfg(not(windows))]
    {
        let _ = (device_id, app_name, muted);
        Err("O mixer de volume por app exige o Windows Core Audio.".into())
    }
}

#[tauri::command]
pub fn set_audio_mixer_master(
    device_id: Option<String>,
    volume: Option<f32>,
    muted: Option<bool>,
) -> Result<AudioMixerInventory, String> {
    #[cfg(windows)]
    {
        windows_impl::set_mixer_master(device_id.as_deref(), volume, muted)
    }
    #[cfg(not(windows))]
    {
        let _ = (device_id, volume, muted);
        Err("O mixer de volume por app exige o Windows Core Audio.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{get_audio_devices, get_audio_mixer};

    #[test]
    fn audio_inventory_never_uses_placeholder_devices() {
        let inventory = get_audio_devices();
        if inventory.unavailable_reason.is_none() {
            assert!(inventory
                .input
                .iter()
                .all(|device| !device.id.is_empty() && !device.name.is_empty()));
            assert!(inventory
                .output
                .iter()
                .all(|device| !device.id.is_empty() && !device.name.is_empty()));
        }
    }

    #[test]
    fn audio_mixer_never_reports_fake_zero_volumes() {
        let mixer = get_audio_mixer(None);
        if let Some(reason) = mixer.unavailable_reason.as_deref() {
            assert!(!reason.is_empty());
            assert!(mixer.apps.is_empty());
            assert!(mixer.master_volume.is_none());
        } else {
            for app in &mixer.apps {
                assert!(!app.app_name.is_empty());
                assert!(!app.display_name.is_empty());
                assert!((0.0..=1.0).contains(&app.volume));
                assert!(!app.volume.is_nan());
                if let Some(icon) = app.icon.as_deref() {
                    assert!(icon.starts_with("data:image/png;base64,"));
                }
            }
            if let Some(master) = mixer.master_volume {
                assert!((0.0..=1.0).contains(&master));
            }
        }
    }
}
