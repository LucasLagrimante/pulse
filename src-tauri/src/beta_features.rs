//! Preferências persistidas dos módulos experimentais (Recursos Beta).
//!
//! Todos os recursos nascem desligados (`false`) para garantir custo zero no
//! boot e no Tray Flyout enquanto o usuário não os ativar explicitamente.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex, sync::OnceLock};
use tauri::{AppHandle, Emitter};

pub const DEFAULT_DOCK_ICON_SIZE: u32 = 44;
pub const MIN_DOCK_ICON_SIZE: u32 = 32;
pub const MAX_DOCK_ICON_SIZE: u32 = 64;
const MAX_PINNED_APPS: usize = 24;

pub const ALL_TOP_BAR_WIDGETS: [&str; 9] = [
    "cpu",
    "ram",
    "gpu",
    "network",
    "peripherals",
    "quotas",
    "tray",
    "volume",
    "clock",
];

pub fn default_top_bar_widgets() -> Vec<String> {
    vec![
        "cpu".to_owned(),
        "ram".to_owned(),
        "gpu".to_owned(),
        "peripherals".to_owned(),
        "quotas".to_owned(),
        "tray".to_owned(),
        "volume".to_owned(),
        "clock".to_owned(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct BetaFeaturesConfig {
    /// Monitor de bateria de periféricos conectados (Bluetooth, BLE, XInput, sysfs).
    pub peripheral_battery: bool,
    /// Pulse Shell completo (Barra Superior + Dock nativa estilo macOS em Win32 + Direct2D).
    /// Quando ativo, oculta automaticamente a barra de tarefas nativa do Windows com watchdog.
    pub custom_dock: bool,
    /// Pulse Runner (launcher rápido estilo Raycast/Spotlight com tecla Windows).
    pub pulse_runner: bool,
    /// Intercepta toque isolado na tecla Windows para abrir o Pulse Runner (preserva Win+E/R/V/L/D).
    pub pulse_runner_win_key: bool,
    /// Recolhe a dock automaticamente e revela ao encostar na borda inferior.
    pub dock_auto_hide: bool,
    /// Mantido no JSON por retrocompatibilidade; internamente sempre espelha `custom_dock`.
    pub dock_hide_native_taskbar: bool,
    /// Ampliação suave dos ícones ao passar o cursor sobre a dock.
    pub dock_magnification: bool,
    /// Tamanho base dos ícones da dock em pixels lógicos (32..=64).
    pub dock_icon_size: u32,
    /// Tema visual do Pulse Shell (`"light"` ou `"dark"`), sincronizado com o app.
    pub dock_theme: String,
    /// Caminhos absolutos de executáveis (ou IDs `shell:AppsFolder\...`) fixados na dock.
    pub dock_pinned_apps: Vec<String>,
    /// Widgets fixados na Barra Superior na ordem exibida (`cpu`, `ram`, `gpu`, `network`,
    /// `peripherals`, `quotas`, `tray`, `volume`, `clock`).
    pub top_bar_widgets: Vec<String>,
    /// Provedores de Cotas de IA fixados na Barra Superior (vazio = automático: menor cota).
    pub top_bar_quota_providers: Vec<String>,
    /// IDs de dispositivos periféricos fixados na Barra Superior (vazio = automático: menor bateria).
    pub top_bar_peripheral_devices: Vec<String>,
}

impl Default for BetaFeaturesConfig {
    fn default() -> Self {
        Self {
            peripheral_battery: false,
            custom_dock: false,
            pulse_runner: false,
            pulse_runner_win_key: true,
            dock_auto_hide: false,
            dock_hide_native_taskbar: false,
            dock_magnification: true,
            dock_icon_size: DEFAULT_DOCK_ICON_SIZE,
            dock_theme: "light".to_owned(),
            dock_pinned_apps: Vec::new(),
            top_bar_widgets: default_top_bar_widgets(),
            top_bar_quota_providers: Vec::new(),
            top_bar_peripheral_devices: Vec::new(),
        }
    }
}

impl BetaFeaturesConfig {
    pub fn sanitized(mut self) -> Self {
        // Sem toggle separado: com o Pulse Shell ativo, a barra nativa do Windows fica sempre oculta.
        self.dock_hide_native_taskbar = self.custom_dock;
        self.dock_icon_size = self
            .dock_icon_size
            .clamp(MIN_DOCK_ICON_SIZE, MAX_DOCK_ICON_SIZE);
        self.dock_theme = if self.dock_theme == "dark" {
            "dark".to_owned()
        } else {
            "light".to_owned()
        };

        let mut unique_apps = Vec::with_capacity(self.dock_pinned_apps.len().min(MAX_PINNED_APPS));
        for raw in self.dock_pinned_apps {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_lowercase();
            if unique_apps
                .iter()
                .any(|existing: &String| existing.to_lowercase() == lower)
            {
                continue;
            }
            unique_apps.push(trimmed.to_owned());
            if unique_apps.len() >= MAX_PINNED_APPS {
                break;
            }
        }
        self.dock_pinned_apps = unique_apps;

        let mut unique_widgets =
            Vec::with_capacity(self.top_bar_widgets.len().min(ALL_TOP_BAR_WIDGETS.len()) + 2);
        for raw in self.top_bar_widgets {
            let normalized = raw.trim().to_ascii_lowercase();
            if !ALL_TOP_BAR_WIDGETS.contains(&normalized.as_str()) {
                continue;
            }
            if unique_widgets.contains(&normalized) {
                continue;
            }
            unique_widgets.push(normalized);
        }
        // Retrocompatibilidade e invariante: `tray` ("Ícones de outros apps") é
        // sempre obrigatório na Barra Superior. Se o JSON salvo ainda não tinha
        // `tray`, também preserva `volume` logo após `tray` para não sumir o áudio
        // de usuários migrando de versões anteriores.
        let had_tray = unique_widgets.iter().any(|w| w == "tray");
        if !had_tray {
            let insert_at = unique_widgets
                .iter()
                .position(|w| w == "volume" || w == "clock")
                .unwrap_or(unique_widgets.len());
            unique_widgets.insert(insert_at, "tray".to_owned());
            if !unique_widgets.iter().any(|w| w == "volume") {
                unique_widgets.insert(insert_at + 1, "volume".to_owned());
            }
        }
        self.top_bar_widgets = unique_widgets;

        let mut unique_quotas = Vec::new();
        for raw in self.top_bar_quota_providers {
            let clean = raw.trim().to_ascii_lowercase();
            if !clean.is_empty() && !unique_quotas.contains(&clean) {
                unique_quotas.push(clean);
                if unique_quotas.len() >= 12 {
                    break;
                }
            }
        }
        self.top_bar_quota_providers = unique_quotas;

        let mut unique_periphs = Vec::new();
        for raw in self.top_bar_peripheral_devices {
            let clean = raw.trim().to_owned();
            if !clean.is_empty() && !unique_periphs.contains(&clean) {
                unique_periphs.push(clean);
                if unique_periphs.len() >= 12 {
                    break;
                }
            }
        }
        self.top_bar_peripheral_devices = unique_periphs;

        self
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DockSettingsPatch {
    pub dock_auto_hide: Option<bool>,
    pub dock_hide_native_taskbar: Option<bool>,
    pub dock_magnification: Option<bool>,
    pub dock_icon_size: Option<u32>,
    pub dock_pinned_apps: Option<Vec<String>>,
    pub top_bar_widgets: Option<Vec<String>>,
    pub top_bar_quota_providers: Option<Vec<String>>,
    pub top_bar_peripheral_devices: Option<Vec<String>>,
}

static CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock() -> &'static Mutex<()> {
    CONFIG_LOCK.get_or_init(|| Mutex::new(()))
}

fn config_file() -> PathBuf {
    crate::storage::paths::pulse_local_dir().join("beta-features.json")
}

pub fn load() -> BetaFeaturesConfig {
    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
    load_unlocked()
}

fn load_unlocked() -> BetaFeaturesConfig {
    fs::read_to_string(config_file())
        .ok()
        .and_then(|content| serde_json::from_str::<BetaFeaturesConfig>(&content).ok())
        .unwrap_or_default()
        .sanitized()
}

fn save_unlocked(config: &BetaFeaturesConfig) -> Result<(), String> {
    let file = config_file();
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(file, content).map_err(|error| error.to_string())
}

/// Atualiza o tema do Pulse Shell quando o usuário alterna Claro/Escuro no Pulse.
pub fn sync_theme(theme: &str) {
    let normalized = if theme == "dark" { "dark" } else { "light" };
    let updated = {
        let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut config = load_unlocked();
        if config.dock_theme == normalized {
            return;
        }
        config.dock_theme = normalized.to_owned();
        let _ = save_unlocked(&config);
        config
    };
    if updated.custom_dock {
        let _ = std::thread::Builder::new()
            .name("pulse-shell-theme".to_owned())
            .spawn(move || {
                let _ = crate::custom_dock::apply_config(&updated);
            });
    }
}

/// Adiciona ou remove um aplicativo fixado a partir do menu nativo da Dock.
pub fn toggle_pinned_app(exe_path: &str) -> Result<BetaFeaturesConfig, String> {
    let trimmed = exe_path.trim();
    if trimmed.is_empty() {
        return Err("Caminho do aplicativo inválido.".to_owned());
    }
    let updated = {
        let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut config = load_unlocked();
        let lower = trimmed.to_lowercase();
        if let Some(pos) = config
            .dock_pinned_apps
            .iter()
            .position(|entry| entry.to_lowercase() == lower)
        {
            config.dock_pinned_apps.remove(pos);
        } else {
            config.dock_pinned_apps.push(trimmed.to_owned());
        }
        let sanitized = config.sanitized();
        save_unlocked(&sanitized)?;
        sanitized
    };
    Ok(updated)
}

/// Salva uma nova ordem de widgets da Barra Superior a partir do drag-and-drop nativo.
pub fn save_top_bar_widgets(widgets: Vec<String>) -> Result<BetaFeaturesConfig, String> {
    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut config = load_unlocked();
    config.top_bar_widgets = widgets;
    let sanitized = config.sanitized();
    save_unlocked(&sanitized)?;
    Ok(sanitized)
}

/// Alterna o auto-ocultar da Dock a partir do menu contextual da Barra Superior.
pub fn toggle_dock_auto_hide() -> Result<BetaFeaturesConfig, String> {
    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut config = load_unlocked();
    config.dock_auto_hide = !config.dock_auto_hide;
    let sanitized = config.sanitized();
    save_unlocked(&sanitized)?;
    Ok(sanitized)
}

/// Desliga a flag do Pulse Shell quando o usuário pede "Restaurar barra do Windows"
/// a partir do menu nativo da Barra Superior / Dock ou da UI de emergência.
pub fn disable_custom_dock_flag() -> Result<BetaFeaturesConfig, String> {
    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut config = load_unlocked();
    config.custom_dock = false;
    config.dock_hide_native_taskbar = false;
    let sanitized = config.sanitized();
    save_unlocked(&sanitized)?;
    Ok(sanitized)
}

#[tauri::command]
pub fn get_beta_features() -> BetaFeaturesConfig {
    load()
}

#[tauri::command]
pub async fn set_beta_feature_enabled(
    app: AppHandle,
    feature: String,
    enabled: bool,
) -> Result<BetaFeaturesConfig, String> {
    let updated = {
        let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut config = load_unlocked();
        match feature.as_str() {
            "peripheralBattery" => {
                config.peripheral_battery = enabled;
            }
            "customDock" => {
                if enabled && !crate::custom_dock::is_supported() {
                    return Err(
                        "A Dock nativa em Win32 + Direct2D está disponível apenas no Windows 10/11."
                            .to_owned(),
                    );
                }
                config.custom_dock = enabled;
                config.dock_hide_native_taskbar = enabled;
            }
            "pulseRunner" => {
                config.pulse_runner = enabled;
            }
            "pulseRunnerWinKey" => {
                config.pulse_runner_win_key = enabled;
            }
            other => {
                return Err(format!("Recurso Beta desconhecido: {other}"));
            }
        }
        let sanitized = config.sanitized();
        save_unlocked(&sanitized)?;
        sanitized
    };

    // Nunca executa apply_config na thread principal da UI do Tauri para não
    // bloquear o loop de mensagens durante ABM_SETSTATE / AppBar / TrayProxy.
    if feature == "customDock" {
        let cfg_apply = updated.clone();
        let res = tauri::async_runtime::spawn_blocking(move || {
            crate::custom_dock::apply_config(&cfg_apply)
        })
        .await
        .map_err(|e| format!("Falha na tarefa do Pulse Shell: {e}"))?;
        if let Err(err) = res {
            let _ = disable_custom_dock_flag();
            return Err(err);
        }
    } else if feature == "peripheralBattery" && updated.custom_dock {
        let cfg_apply = updated.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let _ = crate::custom_dock::apply_config(&cfg_apply);
        })
        .await;
    } else if feature == "pulseRunner" || feature == "pulseRunnerWinKey" {
        let hook_active = updated.pulse_runner && updated.pulse_runner_win_key;
        let _ = tauri::async_runtime::spawn_blocking(move || {
            crate::launcher::apply_win_key_hook(hook_active);
        })
        .await;
    }

    let _ = app.emit("pulse://beta-features-changed", &updated);
    Ok(updated)
}

#[tauri::command]
pub async fn update_dock_settings(
    app: AppHandle,
    patch: DockSettingsPatch,
) -> Result<BetaFeaturesConfig, String> {
    let updated = {
        let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut config = load_unlocked();
        if let Some(auto_hide) = patch.dock_auto_hide {
            config.dock_auto_hide = auto_hide;
        }
        // `dock_hide_native_taskbar` permanece vinculado a `custom_dock` em `sanitized()`.
        let _ = patch.dock_hide_native_taskbar;
        if let Some(mag) = patch.dock_magnification {
            config.dock_magnification = mag;
        }
        if let Some(icon_size) = patch.dock_icon_size {
            config.dock_icon_size = icon_size;
        }
        if let Some(pinned) = patch.dock_pinned_apps {
            config.dock_pinned_apps = pinned;
        }
        if let Some(widgets) = patch.top_bar_widgets {
            config.top_bar_widgets = widgets;
        }
        if let Some(quotas) = patch.top_bar_quota_providers {
            config.top_bar_quota_providers = quotas;
        }
        if let Some(periphs) = patch.top_bar_peripheral_devices {
            config.top_bar_peripheral_devices = periphs;
        }
        let sanitized = config.sanitized();
        save_unlocked(&sanitized)?;
        sanitized
    };

    if updated.custom_dock {
        let cfg_apply = updated.clone();
        tauri::async_runtime::spawn_blocking(move || crate::custom_dock::apply_config(&cfg_apply))
            .await
            .map_err(|e| format!("Falha ao aplicar configuração do Pulse Shell: {e}"))??;
    }

    let _ = app.emit("pulse://beta-features-changed", &updated);
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_disables_all_beta_features_and_sets_top_bar_widgets() {
        let cfg = BetaFeaturesConfig::default();
        assert!(!cfg.peripheral_battery);
        assert!(!cfg.custom_dock);
        assert!(!cfg.dock_hide_native_taskbar);
        assert!(cfg.dock_magnification);
        assert_eq!(cfg.dock_icon_size, DEFAULT_DOCK_ICON_SIZE);
        assert_eq!(
            cfg.top_bar_widgets,
            vec![
                "cpu",
                "ram",
                "gpu",
                "peripherals",
                "quotas",
                "tray",
                "volume",
                "clock"
            ]
        );
    }

    #[test]
    fn sanitize_binds_hide_taskbar_to_custom_dock_and_deduplicates_lists() {
        let cfg = BetaFeaturesConfig {
            custom_dock: true,
            dock_hide_native_taskbar: false,
            dock_icon_size: 200,
            dock_theme: "invalid".to_owned(),
            dock_pinned_apps: vec![
                "  C:\\App\\One.exe ".to_owned(),
                "c:\\app\\one.exe".to_owned(),
                "".to_owned(),
                "C:\\App\\Two.exe".to_owned(),
            ],
            top_bar_widgets: vec![
                " CPU ".to_owned(),
                "quotas".to_owned(),
                "cpu".to_owned(),
                "invalido".to_owned(),
                "network".to_owned(),
            ],
            ..BetaFeaturesConfig::default()
        }
        .sanitized();

        assert!(cfg.dock_hide_native_taskbar);
        assert_eq!(cfg.dock_icon_size, MAX_DOCK_ICON_SIZE);
        assert_eq!(cfg.dock_theme, "light");
        assert_eq!(
            cfg.dock_pinned_apps,
            vec!["C:\\App\\One.exe".to_owned(), "C:\\App\\Two.exe".to_owned()]
        );
        assert_eq!(
            cfg.top_bar_widgets,
            vec![
                "cpu".to_owned(),
                "quotas".to_owned(),
                "network".to_owned(),
                "tray".to_owned(),
                "volume".to_owned()
            ]
        );
    }

    #[test]
    fn deserializing_legacy_config_keeps_default_top_bar_widgets() {
        let legacy =
            r#"{"peripheralBattery":false,"customDock":true,"dockHideNativeTaskbar":false}"#;
        let parsed: BetaFeaturesConfig = serde_json::from_str(legacy).unwrap();
        let clean = parsed.sanitized();
        assert!(clean.custom_dock);
        assert!(clean.dock_hide_native_taskbar);
        assert_eq!(clean.top_bar_widgets, default_top_bar_widgets());
    }
}
