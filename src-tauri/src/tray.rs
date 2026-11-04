use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf, time::Duration};
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewWindow,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TrayPreferences {
    minimize_on_close: bool,
}

fn preferences_file() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Pulse")
        .join("tray-preferences.json")
}

fn load_preferences() -> TrayPreferences {
    fs::read_to_string(preferences_file())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_preferences(preferences: &TrayPreferences) -> Result<(), String> {
    let file = preferences_file();
    let directory = file.parent().expect("preferences file has a parent");
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let content = serde_json::to_string_pretty(preferences).map_err(|error| error.to_string())?;
    fs::write(file, content).map_err(|error| error.to_string())
}

pub fn should_minimize_on_close() -> bool {
    load_preferences().minimize_on_close
}

fn tray_icon_for_theme(theme: &str) -> tauri::image::Image<'static> {
    const SIZE: u32 = 32;
    const SCALE: f32 = 0.6;
    const OFFSET: f32 = 1.6;

    let rail = if theme == "dark" {
        [205, 221, 235]
    } else {
        [61, 82, 101]
    };
    let signal = [239, 143, 18];
    let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];

    let transform = |(x, y): (f32, f32)| (OFFSET + x * SCALE, OFFSET + y * SCALE);
    let rail_path = [
        (8.0, 34.0),
        (8.0, 14.0),
        (8.0, 10.7),
        (10.7, 8.0),
        (14.0, 8.0),
        (34.0, 8.0),
        (37.3, 8.0),
        (40.0, 10.7),
        (40.0, 14.0),
        (40.0, 34.0),
        (40.0, 37.3),
        (37.3, 40.0),
        (34.0, 40.0),
        (20.0, 40.0),
    ]
    .map(transform);

    draw_polyline(&mut rgba, SIZE, &rail_path, 2.8 * SCALE, rail);

    // Colunas de telemetria com esmaecimento da amostra mais antiga para a mais recente,
    // na mesma linguagem visual dos gráficos de CPU e memória do aplicativo.
    const COLUMNS: [(f32, f32); 4] = [(0.34, 0.34), (0.62, 0.55), (0.46, 0.78), (0.82, 1.0)];
    const COLUMN_WIDTH: f32 = 4.5;
    const COLUMN_GAP: f32 = 2.5;
    const BASE_Y: f32 = 37.5;
    const TOP_Y: f32 = 13.0;
    let span =
        COLUMN_WIDTH * COLUMNS.len() as f32 + COLUMN_GAP * (COLUMNS.len().saturating_sub(1)) as f32;
    let start_x = 24.0 - span / 2.0;
    for (index, (height, alpha)) in COLUMNS.iter().enumerate() {
        let x0 = start_x + index as f32 * (COLUMN_WIDTH + COLUMN_GAP);
        let column_top = BASE_Y - height * (BASE_Y - TOP_Y);
        draw_rect(
            &mut rgba,
            SIZE,
            transform((x0, column_top)),
            transform((x0 + COLUMN_WIDTH, BASE_Y)),
            signal,
            *alpha,
        );
    }

    tauri::image::Image::new_owned(rgba, SIZE, SIZE)
}

fn draw_polyline(rgba: &mut [u8], size: u32, points: &[(f32, f32)], width: f32, color: [u8; 3]) {
    let half_width = width / 2.0;
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mut distance = f32::MAX;

            for pair in points.windows(2) {
                let (x1, y1) = pair[0];
                let (x2, y2) = pair[1];
                let dx = x2 - x1;
                let dy = y2 - y1;
                let length_squared = dx * dx + dy * dy;
                let projection = if length_squared == 0.0 {
                    0.0
                } else {
                    ((px - x1) * dx + (py - y1) * dy) / length_squared
                };
                let t = projection.clamp(0.0, 1.0);
                let closest_x = x1 + t * dx;
                let closest_y = y1 + t * dy;
                distance = distance.min((px - closest_x).hypot(py - closest_y));
            }

            let coverage = (half_width + 0.8 - distance).clamp(0.0, 1.0);
            blend_pixel(rgba, size, x, y, color, coverage);
        }
    }
}

fn draw_rect(
    rgba: &mut [u8],
    size: u32,
    (x0, y0): (f32, f32),
    (x1, y1): (f32, f32),
    color: [u8; 3],
    alpha: f32,
) {
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = (x0 - px).max(px - x1).max(0.0);
            let dy = (y0 - py).max(py - y1).max(0.0);
            let coverage = (0.8 - dx.hypot(dy)).clamp(0.0, 1.0) * alpha;
            blend_pixel(rgba, size, x, y, color, coverage);
        }
    }
}

fn blend_pixel(rgba: &mut [u8], size: u32, x: u32, y: u32, color: [u8; 3], coverage: f32) {
    if coverage <= 0.0 {
        return;
    }

    let offset = ((y * size + x) * 4) as usize;
    let existing_alpha = rgba[offset + 3] as f32 / 255.0;
    let source_alpha = coverage.clamp(0.0, 1.0);
    let output_alpha = source_alpha + existing_alpha * (1.0 - source_alpha);
    for channel in 0..3 {
        let existing = rgba[offset + channel] as f32 / 255.0;
        let source = color[channel] as f32 / 255.0;
        rgba[offset + channel] = ((source * source_alpha
            + existing * existing_alpha * (1.0 - source_alpha))
            / output_alpha
            * 255.0) as u8;
    }
    rgba[offset + 3] = (output_alpha * 255.0) as u8;
}

fn set_tray_icon<R: tauri::Runtime>(app: &AppHandle<R>, theme: &str) -> Result<(), String> {
    let tray = app
        .tray_by_id("main-tray")
        .ok_or_else(|| "O ícone da bandeja ainda não está disponível.".to_owned())?;
    tray.set_icon(Some(tray_icon_for_theme(theme)))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_tray_icon_theme(app: AppHandle, theme: String) -> Result<(), String> {
    let normalized = if theme == "dark" { "dark" } else { "light" };
    apply_window_theme(&app, normalized);
    set_tray_icon(&app, normalized)
}

/// Alinha a moldura nativa ao tema do shell: o Windows desenha a barra de
/// título com o tema do sistema, então a caption e o fundo do WebView ficavam
/// destoando do restante da janela.
fn apply_window_theme<R: tauri::Runtime>(app: &AppHandle<R>, theme: &str) {
    let window_theme = if theme == "dark" {
        tauri::Theme::Dark
    } else {
        tauri::Theme::Light
    };
    let background = if theme == "dark" {
        tauri::window::Color(24, 27, 32, 255)
    } else {
        tauri::window::Color(230, 237, 245, 255)
    };

    for window in app.webview_windows().values() {
        let _ = window.set_theme(Some(window_theme));
        let _ = window.set_background_color(Some(background));
    }
}

use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};

pub fn build_tray_menu<R: tauri::Runtime>(manager: &AppHandle<R>) -> Result<Menu<R>, tauri::Error> {
    let audio_inventory = crate::audio::get_audio_devices();
    let minimize = should_minimize_on_close();

    let open_main_item =
        MenuItemBuilder::with_id("open_main", "Abrir aplicativo").build(manager)?;
    let open_flyout_item =
        MenuItemBuilder::with_id("open_flyout", "Painel rápido").build(manager)?;

    // Submenu de Saída de Áudio
    let mut out_submenu_builder = SubmenuBuilder::new(manager, "Saída de áudio");
    if audio_inventory.output.is_empty() {
        let msg = audio_inventory
            .unavailable_reason
            .as_deref()
            .unwrap_or("Nenhum dispositivo encontrado");
        out_submenu_builder = out_submenu_builder.item(
            &MenuItemBuilder::with_id("audio_out_none", msg)
                .enabled(false)
                .build(manager)?,
        );
    } else {
        for device in &audio_inventory.output {
            let item =
                CheckMenuItemBuilder::with_id(format!("audio_out:{}", device.id), &device.name)
                    .checked(device.is_default)
                    .build(manager)?;
            out_submenu_builder = out_submenu_builder.item(&item);
        }
    }
    let out_submenu = out_submenu_builder.build()?;

    // Submenu de Entrada de Áudio
    let mut in_submenu_builder = SubmenuBuilder::new(manager, "Entrada de áudio");
    if audio_inventory.input.is_empty() {
        let msg = audio_inventory
            .unavailable_reason
            .as_deref()
            .unwrap_or("Nenhum dispositivo encontrado");
        in_submenu_builder = in_submenu_builder.item(
            &MenuItemBuilder::with_id("audio_in_none", msg)
                .enabled(false)
                .build(manager)?,
        );
    } else {
        for device in &audio_inventory.input {
            let item =
                CheckMenuItemBuilder::with_id(format!("audio_in:{}", device.id), &device.name)
                    .checked(device.is_default)
                    .build(manager)?;
            in_submenu_builder = in_submenu_builder.item(&item);
        }
    }
    let in_submenu = in_submenu_builder.build()?;

    let minimize_item = CheckMenuItemBuilder::with_id(
        "toggle_minimize_on_close",
        "Minimizar para a bandeja ao fechar",
    )
    .checked(minimize)
    .build(manager)?;

    let exit_item = MenuItemBuilder::with_id("exit", "Sair do Pulse").build(manager)?;

    MenuBuilder::new(manager)
        .item(&open_main_item)
        .item(&open_flyout_item)
        .separator()
        .item(&out_submenu)
        .item(&in_submenu)
        .separator()
        .item(&minimize_item)
        .separator()
        .item(&exit_item)
        .build()
}

pub fn update_tray_menu<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        if let Ok(menu) = build_tray_menu(app) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

pub fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle();
    let menu = build_tray_menu(handle)?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Pulse — Windows Observability")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                "open_main" => {
                    let _ = show_main_internal(app);
                }
                "open_flyout" => {
                    let _ = toggle_flyout_internal(app);
                }
                "toggle_minimize_on_close" => {
                    let mut prefs = load_preferences();
                    prefs.minimize_on_close = !prefs.minimize_on_close;
                    let _ = save_preferences(&prefs);
                    update_tray_menu(app);
                }
                "exit" => {
                    app.exit(0);
                }
                _ => {
                    if let Some(device_id) = id.strip_prefix("audio_out:") {
                        let _ = crate::audio::set_default_audio_device_sync("output", device_id);
                        update_tray_menu(app);
                        if let Some(flyout) = app.get_webview_window("tray-flyout") {
                            let _ = flyout.emit("pulse://audio-changed", ());
                        }
                    } else if let Some(device_id) = id.strip_prefix("audio_in:") {
                        let _ = crate::audio::set_default_audio_device_sync("input", device_id);
                        update_tray_menu(app);
                        if let Some(flyout) = app.get_webview_window("tray-flyout") {
                            let _ = flyout.emit("pulse://audio-changed", ());
                        }
                    }
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            match event {
                // Clique esquerdo simples abre o painel rápido
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    let app = tray.app_handle();
                    let _ = toggle_flyout_internal(app);
                }
                // Clique duplo esquerdo abre a janela principal completa
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    let app = tray.app_handle();
                    let _ = show_main_internal(app);
                }
                _ => {}
            }
        });

    builder = builder.icon(tray_icon_for_theme("light"));

    builder.build(app)?;

    // Tooltip com as temperaturas: é a única superfície de texto do ícone
    // no Windows (o desenho do ícone é estático). Thread dedicada a cada
    // 60 s para não acoplar ao polling da UI nem acordar o app à toa; entre
    // os ciclos, o comando `get_performance_details` atualiza de carona cada
    // leitura que a UI já pediu — zero subprocesso extra.
    // As cotas entram no mesmo tooltip (tempo até cada janela renovar); a
    // contagem regressiva é recalculada a cada ciclo a partir do cache, sem
    // nova consulta aos provedores.
    let tooltip_app = handle.clone();
    let _tooltip_thread = std::thread::spawn(move || {
        refresh_quota_tooltip(
            &tooltip_app,
            &crate::subscription_usage::cached_providers_snapshot(),
        );
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let details = crate::performance::get_performance_details_sync();
            if let Ok(mut thermal) = LAST_THERMAL_LINE.lock() {
                *thermal = thermal_readings(&details.thermal_sensors);
            }
            refresh_quota_tooltip(
                &tooltip_app,
                &crate::subscription_usage::cached_providers_snapshot(),
            );
        }
    });

    Ok(())
}

/// Tooltip da bandeja: prefere o par CPU + GPU
/// (`Pulse — CPU Package 54 °C · RTX 4070 SUPER 67 °C`); sem par
/// identificado, cai para os dois sensores mais quentes.
/// Sem sensores, mantém o texto estático — nunca um valor inventado.
fn thermal_readings(sensors: &[crate::performance::ThermalSensor]) -> Option<String> {
    let is_cpu = |sensor: &crate::performance::ThermalSensor| {
        sensor.label.to_lowercase().contains("cpu")
    };
    let is_gpu = |sensor: &crate::performance::ThermalSensor| {
        sensor.id.starts_with("gpu-")
            || (sensor.label.to_lowercase().contains("gpu")
                && sensor.source.to_lowercase().contains("hardwaremonitor"))
    };
    let mut picks: Vec<&crate::performance::ThermalSensor> = Vec::new();
    if let Some(cpu) = sensors.iter().find(|sensor| is_cpu(sensor)) {
        picks.push(cpu);
    }
    if let Some(gpu) = sensors
        .iter()
        .find(|sensor| is_gpu(sensor) && !picks.iter().any(|picked| picked.id == sensor.id))
    {
        picks.push(gpu);
    }
    if picks.len() < 2 {
        let mut hottest: Vec<&crate::performance::ThermalSensor> = sensors.iter().collect();
        hottest.sort_by(|a, b| {
            b.celsius
                .partial_cmp(&a.celsius)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for sensor in hottest {
            if picks.len() >= 2 {
                break;
            }
            if !picks.iter().any(|picked| picked.id == sensor.id) {
                picks.push(sensor);
            }
        }
    }
    if picks.is_empty() {
        return None;
    }
    Some(
        picks
            .iter()
            .map(|sensor| format!("{} {}", sensor.label, format_celsius(sensor.celsius)))
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

/// Tooltip só com temperaturas (sem cotas confirmadas).
pub(crate) fn tray_tooltip_for_sensors(sensors: &[crate::performance::ThermalSensor]) -> String {
    match thermal_readings(sensors) {
        Some(readings) => format!("Pulse — {readings}"),
        None => DEFAULT_TOOLTIP.to_owned(),
    }
}

const DEFAULT_TOOLTIP: &str = "Pulse — Windows Observability";
/// `NOTIFYICONDATAW::szTip` comporta 128 unidades UTF-16 com o terminador.
const TOOLTIP_MAX_UTF16: usize = 127;

static LAST_THERMAL_LINE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Tempo até a renovação em formato curto: `45min`, `2h30`, `4d2h`.
fn format_countdown(seconds: i64) -> String {
    let minutes = ((seconds.max(0) + 59) / 60).max(1);
    let (days, hours, mins) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    if days > 0 && hours == 0 {
        format!("{days}d")
    } else if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins:02}")
    } else {
        format!("{mins}min")
    }
}

fn short_window_label(window: &crate::subscription_usage::QuotaWindow) -> String {
    match window.window_minutes.filter(|minutes| *minutes > 0) {
        Some(minutes) if minutes % 1440 == 0 => format!("{}d", minutes / 1440),
        Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{minutes}min"),
        None => window.label.chars().take(6).collect(),
    }
}

/// Mesma seleção do flyout: a janela mais crítica de cada duração, da menor
/// para a maior, no máximo duas por provedor.
fn select_tooltip_windows(
    windows: &[crate::subscription_usage::QuotaWindow],
) -> Vec<&crate::subscription_usage::QuotaWindow> {
    let mut picked: Vec<&crate::subscription_usage::QuotaWindow> = Vec::new();
    for window in windows
        .iter()
        .filter(|window| window.remaining_percent.is_some() && window.unavailable_reason.is_none())
    {
        let remaining = window.remaining_percent.unwrap_or(100.0);
        match picked
            .iter_mut()
            .find(|current| current.window_minutes == window.window_minutes)
        {
            Some(current) if current.remaining_percent.unwrap_or(100.0) > remaining => {
                *current = window
            }
            Some(_) => {}
            None => picked.push(window),
        }
    }
    picked.sort_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));
    picked.truncate(2);
    picked
}

/// Uma linha por provedor: `Codex 5h 46% ↻2h30 · 7d 91% ↻6d21h`.
/// Nunca inventa valor: provedor sem janela medida (ou desativado) fica de fora.
pub(crate) fn quota_tooltip_lines(
    providers: &[crate::subscription_usage::ProviderUsage],
    now_unix_seconds: i64,
) -> Vec<String> {
    providers
        .iter()
        .filter(|provider| !provider.disabled && provider.status == "available")
        .filter_map(|provider| {
            let windows = select_tooltip_windows(&provider.windows);
            if windows.is_empty() {
                let credits = provider.credits_remaining.filter(|value| value.is_finite())?;
                return (provider.id == "openrouter")
                    .then(|| format!("{} $ {}", provider.provider, format!("{credits:.2}").replace('.', ",")));
            }
            let parts = windows
                .iter()
                .map(|window| {
                    let reset = window.resets_at_unix_seconds;
                    let expired = reset.is_some_and(|reset| reset <= now_unix_seconds);
                    let remaining = if expired {
                        100.0
                    } else {
                        window.remaining_percent.unwrap_or(0.0)
                    };
                    let countdown = match reset {
                        _ if expired => " ↻renovado".to_owned(),
                        Some(reset) => format!(" ↻{}", format_countdown(reset - now_unix_seconds)),
                        None => String::new(),
                    };
                    format!("{} {:.0}%{}", short_window_label(window), remaining, countdown)
                })
                .collect::<Vec<_>>()
                .join(" · ");
            Some(format!("{} {}", provider.provider, parts))
        })
        .collect()
}

/// Monta o tooltip respeitando o limite do Windows: cotas primeiro (é o que
/// o usuário consulta no hover), temperaturas só se ainda couberem.
pub(crate) fn compose_tooltip(quota_lines: &[String], thermal: Option<&str>) -> String {
    let utf16_len = |text: &str| text.encode_utf16().count();
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    for line in quota_lines {
        let cost = utf16_len(line) + usize::from(!lines.is_empty());
        if used + cost > TOOLTIP_MAX_UTF16 {
            break;
        }
        used += cost;
        lines.push(line.clone());
    }
    if let Some(thermal) = thermal {
        let line = if lines.is_empty() {
            format!("Pulse — {thermal}")
        } else {
            thermal.to_owned()
        };
        if used + utf16_len(&line) + usize::from(!lines.is_empty()) <= TOOLTIP_MAX_UTF16 {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        DEFAULT_TOOLTIP.to_owned()
    } else {
        lines.join("\n")
    }
}

pub fn refresh_quota_tooltip<R: tauri::Runtime>(
    app: &AppHandle<R>,
    providers: &[crate::subscription_usage::ProviderUsage],
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let thermal = LAST_THERMAL_LINE
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    let tooltip = compose_tooltip(&quota_tooltip_lines(providers, now), thermal.as_deref());
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

fn format_celsius(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0} °C")
    } else {
        format!("{rounded:.1} °C")
    }
}

fn show_main_internal(app: &AppHandle) -> Result<(), String> {
    if let Some(flyout) = app.get_webview_window("tray-flyout") {
        let _ = flyout.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.unminimize().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        let _ = window.emit("pulse://foreground-changed", true);
    }
    Ok(())
}

fn toggle_flyout_internal(app: &AppHandle) -> Result<(), String> {
    if let Some(flyout) = app.get_webview_window("tray-flyout") {
        let is_visible = flyout.is_visible().unwrap_or(false);
        if is_visible {
            let _ = flyout.emit("pulse://foreground-changed", false);
            let _ = flyout.hide();
        } else {
            // Posiciona a janela no canto inferior direito próximo à barra de tarefas
            if let Ok(Some(monitor)) = flyout.current_monitor() {
                let size = monitor.size();
                let scale = monitor.scale_factor();
                let flyout_w = 390.0;
                let flyout_h = 740.0;
                let margin_right = 16.0;
                let margin_bottom = 56.0; // Espaço para a barra de tarefas do Windows

                let screen_w = size.width as f64 / scale;
                let screen_h = size.height as f64 / scale;
                let pos_x = (screen_w - flyout_w - margin_right).max(0.0);
                let pos_y = (screen_h - flyout_h - margin_bottom).max(0.0);

                let _ = flyout.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                    pos_x, pos_y,
                )));
            }
            flyout.show().map_err(|e| e.to_string())?;
            flyout.set_focus().map_err(|e| e.to_string())?;
            let _ = flyout.emit("pulse://foreground-changed", true);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_current_window_label(window: WebviewWindow) -> String {
    window.label().to_string()
}

#[tauri::command]
pub fn is_current_window_foreground(window: WebviewWindow) -> bool {
    window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
}

#[tauri::command]
pub fn minimize_to_tray(window: WebviewWindow) -> Result<(), String> {
    let _ = window.emit("pulse://foreground-changed", false);
    window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn minimize_to_tray_and_remember(window: WebviewWindow) -> Result<(), String> {
    save_preferences(&TrayPreferences {
        minimize_on_close: true,
    })?;
    minimize_to_tray(window)
}

#[tauri::command]
pub fn get_minimize_on_close() -> bool {
    should_minimize_on_close()
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), String> {
    show_main_internal(&app)
}

/// Abre a janela principal já no módulo pedido (ex.: clique num card de
/// cota no flyout → "Cotas de IA").
#[tauri::command]
pub fn show_main_module(app: AppHandle, module: String) -> Result<(), String> {
    show_main_internal(&app)?;
    app.emit_to("main", "pulse://navigate-area", module)
        .map_err(|error| error.to_string())
}

/// Chamado pelo frontend após a primeira pintura do React. Exibe a janela
/// `main` já com o tamanho restaurado pelo plugin window-state e o tema
/// aplicado, eliminando o clarão branco e o salto de resize do arranque.
/// Respeita o arranque minimizado (inicialização com o Windows / --minimized).
#[tauri::command]
pub fn frontend_ready(app: AppHandle) -> Result<(), String> {
    if crate::startup::should_start_minimized(&std::env::args().collect::<Vec<_>>()) {
        return Ok(());
    }
    show_main_internal(&app)
}

#[tauri::command]
pub fn toggle_tray_flyout(app: AppHandle) -> Result<(), String> {
    toggle_flyout_internal(&app)
}

#[tauri::command]
pub fn hide_tray_flyout(app: AppHandle) -> Result<(), String> {
    if let Some(flyout) = app.get_webview_window("tray-flyout") {
        let _ = flyout.emit("pulse://foreground-changed", false);
        flyout.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn exit_app(app: AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::{
        compose_tooltip, format_celsius, format_countdown, quota_tooltip_lines,
        tray_icon_for_theme, tray_tooltip_for_sensors,
    };
    use crate::performance::ThermalSensor;
    use crate::subscription_usage::{ProviderUsage, QuotaWindow};

    fn window(id: &str, minutes: u64, remaining: f64, reset: Option<i64>) -> QuotaWindow {
        QuotaWindow {
            id: id.into(),
            label: id.into(),
            used_percent: Some(100.0 - remaining),
            remaining_percent: Some(remaining),
            window_minutes: Some(minutes),
            resets_at_unix_seconds: reset,
            reset_description: None,
            unavailable_reason: None,
        }
    }

    fn provider(id: &str, name: &str, windows: Vec<QuotaWindow>) -> ProviderUsage {
        ProviderUsage {
            id: id.into(),
            provider: name.into(),
            subtitle: String::new(),
            plan: None,
            environment: None,
            status: "available".into(),
            windows,
            credits_remaining: None,
            credits_total: None,
            credits_used: None,
            reset_credits_available: None,
            sampled_at_unix_ms: None,
            source: String::new(),
            unavailable_reason: None,
            disabled: false,
        }
    }

    #[test]
    fn countdown_is_compact() {
        assert_eq!(format_countdown(45 * 60), "45min");
        assert_eq!(format_countdown(2 * 3600 + 30 * 60), "2h30");
        assert_eq!(format_countdown(4 * 86400 + 2 * 3600), "4d2h");
    }

    #[test]
    fn tooltip_lists_most_critical_window_per_duration_with_countdown() {
        let now = 1_000_000;
        let providers = vec![
            provider(
                "antigravity",
                "Antigravity",
                vec![
                    window("gem-7d", 10080, 92.0, Some(now + 4 * 86400 + 2 * 3600)),
                    window("gem-5h", 300, 97.0, Some(now + 3 * 3600 + 53 * 60)),
                    window("argon-7d", 10080, 40.0, Some(now + 7 * 86400)),
                ],
            ),
            ProviderUsage { disabled: true, ..provider("codex", "Codex", vec![window("s", 300, 10.0, None)]) },
        ];
        assert_eq!(
            quota_tooltip_lines(&providers, now),
            vec!["Antigravity 5h 97% ↻3h53 · 7d 40% ↻7d".to_owned()]
        );
    }

    #[test]
    fn tooltip_respects_windows_length_limit_and_keeps_quotas_first() {
        let lines = vec!["A".repeat(60), "B".repeat(60), "C".repeat(60)];
        let tooltip = compose_tooltip(&lines, Some("CPU 50 °C"));
        assert!(tooltip.encode_utf16().count() <= 127);
        assert!(tooltip.starts_with(&"A".repeat(60)));
        assert!(!tooltip.contains('C'));
        assert_eq!(compose_tooltip(&[], None), "Pulse — Windows Observability");
        assert_eq!(compose_tooltip(&[], Some("CPU 50 °C")), "Pulse — CPU 50 °C");
    }

    #[test]
    fn tray_icon_is_transparent_and_theme_aware() {
        let light = tray_icon_for_theme("light");
        let dark = tray_icon_for_theme("dark");

        assert_eq!((light.width(), light.height()), (32, 32));
        assert!(light.rgba().chunks_exact(4).any(|pixel| pixel[3] > 0));
        assert_ne!(light.rgba(), dark.rgba());
        assert!(light.rgba().chunks_exact(4).any(|pixel| pixel[3] == 0));
    }

    #[test]
    fn tray_icon_columns_fade_towards_the_latest_sample() {
        const SIZE: u32 = 32;
        let icon = tray_icon_for_theme("dark");
        let mut older = 0u64;
        let mut latest = 0u64;

        for (index, pixel) in icon.rgba().chunks_exact(4).enumerate() {
            let is_signal =
                pixel[3] > 0 && pixel[0] > 180 && (90..190).contains(&pixel[1]) && pixel[2] < 80;
            if !is_signal {
                continue;
            }
            if index as u32 % SIZE < SIZE / 2 {
                older += pixel[3] as u64;
            } else {
                latest += pixel[3] as u64;
            }
        }

        assert!(older > 0, "as colunas de telemetria devem existir");
        assert!(
            latest > older,
            "a metade recente das colunas deve concentrar mais sinal"
        );
    }

    #[test]
    fn tooltip_prefers_cpu_and_gpu_pair() {
        let sensors = vec![
            ThermalSensor { id: "acpi-tz00".into(), label: "Zona térmica TZ00".into(), celsius: 45.0, source: "ACPI".into() },
            ThermalSensor { id: "gpu-0".into(), label: "RTX 4070 SUPER".into(), celsius: 67.25, source: "nvidia-smi".into() },
            ThermalSensor { id: "lhm-cpu".into(), label: "CPU Package".into(), celsius: 54.0, source: "LibreHardwareMonitor".into() },
        ];
        assert_eq!(
            tray_tooltip_for_sensors(&sensors),
            "Pulse — CPU Package 54 °C · RTX 4070 SUPER 67.3 °C"
        );
        assert_eq!(
            tray_tooltip_for_sensors(&[]),
            "Pulse — Windows Observability"
        );
    }

    #[test]
    fn tooltip_falls_back_to_two_hottest_sensors() {
        let sensors = vec![
            ThermalSensor { id: "acpi-tz00".into(), label: "Zona térmica TZ00".into(), celsius: 45.0, source: "ACPI".into() },
            ThermalSensor { id: "probe-0".into(), label: "Sonda térmica".into(), celsius: 41.0, source: "WMI".into() },
        ];
        assert_eq!(
            tray_tooltip_for_sensors(&sensors),
            "Pulse — Zona térmica TZ00 45 °C · Sonda térmica 41 °C"
        );
    }

    #[test]
    fn formats_celsius_without_trailing_zero() {
        assert_eq!(format_celsius(67.0), "67 °C");
        assert_eq!(format_celsius(67.25), "67.3 °C");
    }
}
