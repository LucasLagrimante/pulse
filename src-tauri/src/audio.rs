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

#[cfg(windows)]
mod windows_impl {
    use super::{AudioDevice, AudioDeviceInventory};
    use windows::core::{GUID, HRESULT, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED, STGM_READ,
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

#[tauri::command]
pub fn set_default_audio_device(
    direction: String,
    device_id: String,
) -> Result<AudioDeviceInventory, String> {
    #[cfg(windows)]
    {
        windows_impl::set_default(&direction, &device_id)
    }
    #[cfg(not(windows))]
    {
        let _ = (direction, device_id);
        Err("A troca de dispositivos de áudio exige o Windows Core Audio.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::get_audio_devices;

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
}
