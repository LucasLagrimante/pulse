//! Feed leve de telemetria em segundo plano para os widgets da Barra Superior.
//!
//! Coleta a cada ~2s sem subprocessos pesados e sem bloquear a thread Win32 da
//! Barra Superior. Nunca inventa zeros: leituras ausentes ficam `None`.

use crate::beta_features::BetaFeaturesConfig;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    Foundation::{FILETIME, HWND, LPARAM, WPARAM},
    Media::Audio::{
        eConsole, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator,
    },
    NetworkManagement::{
        IpHelper::{
            FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_ROW2, MIB_IF_TABLE2,
        },
        Ndis::IfOperStatusUp,
    },
    System::{
        Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED},
        Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetSystemTimes,
    },
    UI::WindowsAndMessaging::PostMessageW,
};

const IF_TYPE_TUNNEL_CONST: u32 = 131;
const POLL_INTERVAL: Duration = Duration::from_millis(2000);
const PERIPHERAL_POLL_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default)]
pub struct QuotaAlertSummary {
    pub label: String,
    pub tooltip: String,
    /// `0` = normal, `1` = alerta laranja (<= 20%), `2` = crítico vermelho (esgotada / 0%).
    pub severity: u8,
}

#[derive(Clone, Debug)]
pub struct PeripheralSummary {
    pub label: String,
    pub tooltip: String,
    pub low_battery: bool,
    pub kind: crate::peripheral_battery::PeripheralKind,
}

#[derive(Clone, Debug, Default)]
pub struct BatterySummary {
    pub percent: u8,
    pub charging: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ShellTelemetrySnapshot {
    pub cpu_percent: Option<f64>,
    pub ram_percent: Option<f64>,
    pub ram_detail: Option<String>,
    pub gpu_percent: Option<f64>,
    pub net_rx_bps: Option<f64>,
    pub net_tx_bps: Option<f64>,
    pub volume_percent: Option<u8>,
    pub volume_muted: Option<bool>,
    pub battery: Option<BatterySummary>,
    pub peripherals: Vec<PeripheralSummary>,
    pub peripheral_enabled: bool,
    pub quotas: Vec<QuotaAlertSummary>,
}

struct FeedState {
    snapshot: ShellTelemetrySnapshot,
    config: BetaFeaturesConfig,
    notify_hwnd: isize,
    notify_msg: u32,
    force_periph_refresh: bool,
}

struct FeedController {
    stop_flag: Arc<AtomicBool>,
    wake: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

static STATE: OnceLock<Mutex<FeedState>> = OnceLock::new();
static CONTROLLER: OnceLock<Mutex<FeedController>> = OnceLock::new();

fn state() -> &'static Mutex<FeedState> {
    STATE.get_or_init(|| {
        Mutex::new(FeedState {
            snapshot: ShellTelemetrySnapshot::default(),
            config: BetaFeaturesConfig::default(),
            notify_hwnd: 0,
            notify_msg: 0,
            force_periph_refresh: true,
        })
    })
}

fn controller() -> &'static Mutex<FeedController> {
    CONTROLLER.get_or_init(|| {
        Mutex::new(FeedController {
            stop_flag: Arc::new(AtomicBool::new(false)),
            wake: Arc::new((Mutex::new(false), Condvar::new())),
            thread: None,
        })
    })
}

pub fn bind_topbar(hwnd: HWND, notify_msg: u32) {
    if let Ok(mut guard) = state().lock() {
        guard.notify_hwnd = hwnd.0 as isize;
        guard.notify_msg = notify_msg;
    }
}

pub fn update_config(config: &BetaFeaturesConfig) {
    if let Ok(mut guard) = state().lock() {
        guard.config = config.clone();
        guard.snapshot.peripheral_enabled = config.peripheral_battery;
        guard.force_periph_refresh = true;
        if !config.peripheral_battery {
            guard.snapshot.peripherals.clear();
        }
    }
    if let Ok(ctrl) = controller().lock() {
        let (lock, cvar) = &*ctrl.wake;
        if let Ok(mut signaled) = lock.lock() {
            *signaled = true;
            cvar.notify_all();
        }
    }
}

/// Força re-leitura imediata dos periféricos (ex.: após renomear ou trocar ícone).
pub fn invalidate_peripherals() {
    if let Ok(mut guard) = state().lock() {
        guard.force_periph_refresh = true;
    }
    if let Ok(ctrl) = controller().lock() {
        let (lock, cvar) = &*ctrl.wake;
        if let Ok(mut signaled) = lock.lock() {
            *signaled = true;
            cvar.notify_all();
        }
    }
}

pub fn latest_snapshot() -> ShellTelemetrySnapshot {
    state()
        .lock()
        .map(|g| g.snapshot.clone())
        .unwrap_or_default()
}

pub fn start(initial_config: &BetaFeaturesConfig) {
    update_config(initial_config);
    let mut ctrl = controller().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref handle) = ctrl.thread {
        if !handle.is_finished() {
            return;
        }
    }
    if let Some(old) = ctrl.thread.take() {
        let _ = old.join();
    }
    ctrl.stop_flag.store(false, Ordering::SeqCst);
    let stop = Arc::clone(&ctrl.stop_flag);
    let wake = Arc::clone(&ctrl.wake);
    if let Ok(handle) = thread::Builder::new()
        .name("pulse-shell-telemetry".to_owned())
        .spawn(move || run_feed_loop(stop, wake))
    {
        ctrl.thread = Some(handle);
    }
}

pub fn stop() {
    let taken = {
        let mut ctrl = controller().lock().unwrap_or_else(|e| e.into_inner());
        ctrl.stop_flag.store(true, Ordering::SeqCst);
        let (lock, cvar) = &*ctrl.wake;
        if let Ok(mut signaled) = lock.lock() {
            *signaled = true;
            cvar.notify_all();
        }
        ctrl.thread.take()
    };
    if let Some(handle) = taken {
        let _ = handle.join();
    }
    if let Ok(mut guard) = state().lock() {
        guard.notify_hwnd = 0;
        guard.snapshot = ShellTelemetrySnapshot::default();
    }
}

pub fn adjust_master_volume_step(delta: f32) {
    if let Some((current, _)) = read_master_volume() {
        let next = (current + delta).clamp(0.0, 1.0);
        let _ = crate::audio::set_audio_mixer_master(None, Some(next), None);
        refresh_volume_only();
    }
}

pub fn toggle_master_mute() {
    if let Some((_, muted)) = read_master_volume() {
        let _ = crate::audio::set_audio_mixer_master(None, None, Some(!muted));
        refresh_volume_only();
    }
}

fn refresh_volume_only() {
    let vol = read_master_volume();
    let (hwnd_raw, msg) = {
        let Ok(mut guard) = state().lock() else {
            return;
        };
        guard.snapshot.volume_percent = vol.map(|(lvl, _)| (lvl * 100.0).round() as u8);
        guard.snapshot.volume_muted = vol.map(|(_, m)| m);
        (guard.notify_hwnd, guard.notify_msg)
    };
    if hwnd_raw != 0 && msg != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

struct CpuSample {
    idle: u64,
    kernel: u64,
    user: u64,
}

struct NetSample {
    rx: u64,
    tx: u64,
    at: Instant,
}

fn run_feed_loop(stop: Arc<AtomicBool>, wake: Arc<(Mutex<bool>, Condvar)>) {
    let mut prev_cpu: Option<CpuSample> = None;
    let mut prev_net: Option<NetSample> = None;
    let mut last_periph_poll: Option<Instant> = None;
    let mut cached_periph: Vec<PeripheralSummary> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        let (cfg, forced_periph) = if let Ok(mut guard) = state().lock() {
            let forced = guard.force_periph_refresh;
            guard.force_periph_refresh = false;
            (guard.config.clone(), forced)
        } else {
            (BetaFeaturesConfig::default(), false)
        };

        let widgets = &cfg.top_bar_widgets;
        let want_cpu = widgets.iter().any(|w| w == "cpu");
        let want_ram = widgets.iter().any(|w| w == "ram");
        let want_gpu = widgets.iter().any(|w| w == "gpu");
        let want_net = widgets.iter().any(|w| w == "network");
        let want_periph = cfg.peripheral_battery && widgets.iter().any(|w| w == "peripherals");
        let want_quotas = widgets.iter().any(|w| w == "quotas");

        let cpu_percent = if want_cpu {
            sample_cpu_percent(&mut prev_cpu)
        } else {
            None
        };

        let (ram_percent, ram_detail) = if want_ram {
            sample_ram_status()
        } else {
            (None, None)
        };

        let gpu_percent = if want_gpu { sample_gpu_percent() } else { None };

        let (net_rx_bps, net_tx_bps) = if want_net {
            sample_network_bps(&mut prev_net)
        } else {
            (None, None)
        };

        let volume = read_master_volume();
        let battery = read_notebook_battery();

        if want_periph {
            let should_poll = forced_periph
                || last_periph_poll.is_none_or(|at| at.elapsed() >= PERIPHERAL_POLL_INTERVAL);
            if should_poll {
                last_periph_poll = Some(Instant::now());
                cached_periph = sample_peripherals(&cfg.top_bar_peripheral_devices);
            }
        } else {
            cached_periph.clear();
            last_periph_poll = None;
        }

        let quotas = if want_quotas {
            sample_cached_quotas(&cfg.top_bar_quota_providers)
        } else {
            Vec::new()
        };

        let next_snapshot = ShellTelemetrySnapshot {
            cpu_percent,
            ram_percent,
            ram_detail,
            gpu_percent,
            net_rx_bps,
            net_tx_bps,
            volume_percent: volume.map(|(lvl, _)| (lvl * 100.0).round() as u8),
            volume_muted: volume.map(|(_, muted)| muted),
            battery,
            peripherals: cached_periph.clone(),
            peripheral_enabled: cfg.peripheral_battery,
            quotas,
        };

        let (hwnd_raw, msg) = {
            if let Ok(mut guard) = state().lock() {
                guard.snapshot = next_snapshot;
                (guard.notify_hwnd, guard.notify_msg)
            } else {
                (0, 0)
            }
        };

        if hwnd_raw != 0 && msg != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), msg, WPARAM(0), LPARAM(0));
            }
        }

        let (lock, cvar) = &*wake;
        if let Ok(mut guard) = lock.lock() {
            if !*guard && !stop.load(Ordering::Relaxed) {
                let (new_guard, _) = cvar
                    .wait_timeout(guard, POLL_INTERVAL)
                    .unwrap_or_else(|e| e.into_inner());
                guard = new_guard;
            }
            *guard = false;
        }
    }
}

fn filetime_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

fn sample_cpu_percent(prev: &mut Option<CpuSample>) -> Option<f64> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
    }
    let cur = CpuSample {
        idle: filetime_u64(idle),
        kernel: filetime_u64(kernel),
        user: filetime_u64(user),
    };
    let result = if let Some(ref p) = *prev {
        let idle_d = cur.idle.saturating_sub(p.idle);
        let kernel_d = cur.kernel.saturating_sub(p.kernel);
        let user_d = cur.user.saturating_sub(p.user);
        let total = kernel_d.saturating_add(user_d);
        if total > 0 {
            let busy = total.saturating_sub(idle_d);
            Some(((busy as f64 / total as f64) * 100.0).clamp(0.0, 100.0))
        } else {
            None
        }
    } else {
        None
    };
    *prev = Some(cur);
    result
}

fn sample_ram_status() -> (Option<f64>, Option<String>) {
    let mut mem = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut mem) }.is_err() || mem.ullTotalPhys == 0 {
        return (None, None);
    }
    let gib = 1024.0 * 1024.0 * 1024.0;
    let total = mem.ullTotalPhys as f64 / gib;
    let used = mem.ullTotalPhys.saturating_sub(mem.ullAvailPhys) as f64 / gib;
    let pct = ((used / total) * 100.0).clamp(0.0, 100.0);
    (Some(pct), Some(format!("RAM: {used:.1} / {total:.1} GB")))
}

fn sample_gpu_percent() -> Option<f64> {
    let hw = crate::hardware_telemetry::collect_hardware_telemetry_sync();
    if let Some(pct) = hw.gpu.usage_percent.filter(|v| v.is_finite()) {
        return Some(pct.clamp(0.0, 100.0));
    }
    crate::process_usage::gpu_usage()
        .map(|g| g.total_percent.clamp(0.0, 100.0))
        .filter(|v| v.is_finite())
}

fn sample_network_bps(prev: &mut Option<NetSample>) -> (Option<f64>, Option<f64>) {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) }.is_err() || table.is_null() {
        return (None, None);
    }

    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    let mut has_up = false;

    unsafe {
        let count = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts(
            std::ptr::addr_of!((*table).Table).cast::<MIB_IF_ROW2>(),
            count,
        );
        for row in rows {
            if row.Type == IF_TYPE_SOFTWARE_LOOPBACK || row.Type == IF_TYPE_TUNNEL_CONST {
                continue;
            }
            if row.OperStatus != IfOperStatusUp {
                continue;
            }
            if row.InterfaceAndOperStatusFlags._bitfield & 0x01 == 0 {
                continue;
            }
            has_up = true;
            rx_total = rx_total.saturating_add(row.InOctets);
            tx_total = tx_total.saturating_add(row.OutOctets);
        }
        FreeMibTable(table.cast());
    }

    if !has_up {
        *prev = None;
        return (None, None);
    }

    let now = Instant::now();
    let out = if let Some(ref p) = *prev {
        let dt = now.saturating_duration_since(p.at).as_secs_f64();
        if dt > 0.1 && rx_total >= p.rx && tx_total >= p.tx {
            let rx_bps = (rx_total - p.rx) as f64 / dt;
            let tx_bps = (tx_total - p.tx) as f64 / dt;
            (Some(rx_bps), Some(tx_bps))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };
    *prev = Some(NetSample {
        rx: rx_total,
        tx: tx_total,
        at: now,
    });
    out
}

fn read_master_volume() -> Option<(f32, bool)> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let need_uninit = hr.is_ok();
        let result = (|| -> Option<(f32, bool)> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
            let scalar = endpoint.GetMasterVolumeLevelScalar().ok()?.clamp(0.0, 1.0);
            let muted = endpoint.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            Some((scalar, muted))
        })();
        if need_uninit {
            CoUninitialize();
        }
        result
    }
}

fn read_notebook_battery() -> Option<BatterySummary> {
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return None;
    }
    // Regra estrita: exibir somente se BatteryFlag != 128 (sem bateria) e <= 100%.
    if (status.BatteryFlag & 128) != 0
        || status.BatteryFlag == 255
        || status.BatteryLifePercent > 100
    {
        return None;
    }
    Some(BatterySummary {
        percent: status.BatteryLifePercent,
        charging: status.ACLineStatus == 1 || (status.BatteryFlag & 8) != 0,
    })
}

/// Compacta o nome (personalizado ou original) do periférico para caber
/// confortavelmente na Barra Superior sem crescer demais.
fn short_device_label(name: &str, kind: crate::peripheral_battery::PeripheralKind) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return match kind {
            crate::peripheral_battery::PeripheralKind::Mouse => "Mouse".to_owned(),
            crate::peripheral_battery::PeripheralKind::Keyboard => "Teclado".to_owned(),
            crate::peripheral_battery::PeripheralKind::Headset => "Fone".to_owned(),
            crate::peripheral_battery::PeripheralKind::Controller => "Controle".to_owned(),
            crate::peripheral_battery::PeripheralKind::Stylus => "Caneta".to_owned(),
            crate::peripheral_battery::PeripheralKind::Other => "Perif.".to_owned(),
        };
    }
    const MAX_CHARS: usize = 12;
    let count = trimmed.chars().count();
    if count <= MAX_CHARS {
        trimmed.to_owned()
    } else {
        let cut: String = trimmed.chars().take(MAX_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

fn sample_peripherals(selected_ids: &[String]) -> Vec<PeripheralSummary> {
    let snapshot =
        tauri::async_runtime::block_on(crate::peripheral_battery::get_peripheral_batteries());
    let mut connected: Vec<&crate::peripheral_battery::PeripheralBatteryDevice> = Vec::new();
    let mut all_lines: Vec<String> = Vec::new();

    for dev in &snapshot.devices {
        if !dev.connected {
            continue;
        }
        if let Some(pct) = dev.battery_percent.value {
            all_lines.push(format!("{}: {pct}%", dev.name));
            connected.push(dev);
        }
    }

    if connected.is_empty() {
        return Vec::new();
    }

    // Se o usuário escolheu dispositivos específicos, exibe cada um deles na ordem escolhida.
    if !selected_ids.is_empty() {
        let mut out = Vec::new();
        for want_id in selected_ids {
            if let Some(dev) = connected.iter().find(|d| d.id == *want_id) {
                if let Some(pct) = dev.battery_percent.value {
                    let short = short_device_label(&dev.name, dev.kind);
                    out.push(PeripheralSummary {
                        label: format!("{short} {pct}%"),
                        tooltip: format!("{}: {pct}%", dev.name),
                        low_battery: pct <= 20,
                        kind: dev.kind,
                    });
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    // Padrão automático: exibe o dispositivo conectado com menor bateria.
    let lowest = connected
        .iter()
        .min_by_key(|d| d.battery_percent.value.unwrap_or(101));
    if let Some(dev) = lowest {
        if let Some(pct) = dev.battery_percent.value {
            let short = short_device_label(&dev.name, dev.kind);
            return vec![PeripheralSummary {
                label: format!("{short} {pct}%"),
                tooltip: all_lines.join("\n"),
                low_battery: pct <= 20,
                kind: dev.kind,
            }];
        }
    }
    Vec::new()
}

/// Lê o último snapshot cacheado de cotas de IA sem disparar requisições HTTP.
pub fn sample_cached_quotas(selected_ids: &[String]) -> Vec<QuotaAlertSummary> {
    let providers = crate::subscription_usage::cached_providers_snapshot();
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    summarize_quotas_list(&providers, now_secs, selected_ids)
}

fn summarize_single_provider(
    p: &crate::subscription_usage::ProviderUsage,
    now_unix_seconds: i64,
) -> Option<QuotaAlertSummary> {
    if p.disabled || p.status != "available" {
        return None;
    }
    let short_name: String = p
        .provider
        .split_whitespace()
        .next()
        .unwrap_or("IA")
        .chars()
        .take(10)
        .collect();

    let mut min_rem: Option<f64> = None;
    let mut tip_lines: Vec<String> = Vec::new();
    for w in &p.windows {
        if w.unavailable_reason.is_some() {
            continue;
        }
        let Some(raw_rem) = w.remaining_percent.filter(|v| v.is_finite()) else {
            continue;
        };
        let expired = w
            .resets_at_unix_seconds
            .is_some_and(|r| r <= now_unix_seconds);
        let rem = if expired {
            100.0
        } else {
            raw_rem.clamp(0.0, 100.0)
        };
        tip_lines.push(format!("{} ({}): {:.0}%", p.provider, w.label, rem));
        if min_rem.is_none_or(|best| rem < best) {
            min_rem = Some(rem);
        }
    }

    if let Some(rem) = min_rem {
        let severity = if rem <= 0.5 {
            2
        } else if rem <= 20.0 {
            1
        } else {
            0
        };
        return Some(QuotaAlertSummary {
            label: format!("{short_name} {:.0}%", rem),
            tooltip: tip_lines.join("\n"),
            severity,
        });
    }

    if let Some(credits) = p.credits_remaining.filter(|v| v.is_finite() && *v >= 0.0) {
        let severity = if credits <= 0.01 {
            2
        } else if credits < 1.0 {
            1
        } else {
            0
        };
        return Some(QuotaAlertSummary {
            label: format!("{short_name} ${credits:.2}"),
            tooltip: format!("{}: ${credits:.2}", p.provider),
            severity,
        });
    }

    None
}

pub fn summarize_quotas_list(
    providers: &[crate::subscription_usage::ProviderUsage],
    now_unix_seconds: i64,
    selected_ids: &[String],
) -> Vec<QuotaAlertSummary> {
    if !selected_ids.is_empty() {
        let mut out = Vec::new();
        for want_id in selected_ids {
            if let Some(p) = providers
                .iter()
                .find(|prov| prov.id.eq_ignore_ascii_case(want_id))
            {
                if let Some(summary) = summarize_single_provider(p, now_unix_seconds) {
                    out.push(summary);
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    summarize_quotas(providers, now_unix_seconds)
        .into_iter()
        .collect()
}

pub fn summarize_quotas(
    providers: &[crate::subscription_usage::ProviderUsage],
    now_unix_seconds: i64,
) -> Option<QuotaAlertSummary> {
    let mut min_entry: Option<(f64, String, String)> = None;
    let mut credits_entry: Option<(f64, String)> = None;
    let mut tip_lines: Vec<String> = Vec::new();

    for p in providers {
        if p.disabled || p.status != "available" {
            continue;
        }
        for w in &p.windows {
            if w.unavailable_reason.is_some() {
                continue;
            }
            let Some(raw_rem) = w.remaining_percent.filter(|v| v.is_finite()) else {
                continue;
            };
            let expired = w
                .resets_at_unix_seconds
                .is_some_and(|r| r <= now_unix_seconds);
            let rem = if expired {
                100.0
            } else {
                raw_rem.clamp(0.0, 100.0)
            };
            tip_lines.push(format!("{} ({}): {:.0}%", p.provider, w.label, rem));
            if min_entry.as_ref().is_none_or(|(best, _, _)| rem < *best) {
                min_entry = Some((rem, p.provider.clone(), w.label.clone()));
            }
        }

        if p.windows.is_empty() {
            if let Some(credits) = p.credits_remaining.filter(|v| v.is_finite() && *v >= 0.0) {
                tip_lines.push(format!("{}: ${credits:.2}", p.provider));
                if credits_entry
                    .as_ref()
                    .is_none_or(|(best, _)| credits < *best)
                {
                    credits_entry = Some((credits, p.provider.clone()));
                }
            }
        }
    }

    if let Some((rem, provider_name, _)) = min_entry {
        let severity = if rem <= 0.5 {
            2
        } else if rem <= 20.0 {
            1
        } else {
            0
        };
        let short_name: String = provider_name
            .split_whitespace()
            .next()
            .unwrap_or("IA")
            .chars()
            .take(10)
            .collect();
        let label = if severity > 0 {
            format!("{short_name} {:.0}%", rem)
        } else {
            format!("IA {:.0}%", rem)
        };
        return Some(QuotaAlertSummary {
            label,
            tooltip: tip_lines.join("\n"),
            severity,
        });
    }

    if let Some((credits, provider_name)) = credits_entry {
        let severity = if credits <= 0.01 {
            2
        } else if credits < 1.0 {
            1
        } else {
            0
        };
        let short_name: String = provider_name
            .split_whitespace()
            .next()
            .unwrap_or("IA")
            .chars()
            .take(10)
            .collect();
        return Some(QuotaAlertSummary {
            label: format!("{short_name} ${credits:.2}"),
            tooltip: tip_lines.join("\n"),
            severity,
        });
    }

    None
}

pub fn format_rate_compact(bytes_per_sec: f64) -> String {
    if !bytes_per_sec.is_finite() || bytes_per_sec < 0.0 {
        return "—".to_owned();
    }
    if bytes_per_sec >= 1_048_576.0 {
        format!("{:.1}M", bytes_per_sec / 1_048_576.0)
    } else if bytes_per_sec >= 1024.0 {
        format!("{:.0}K", bytes_per_sec / 1024.0)
    } else {
        format!("{:.0}B", bytes_per_sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription_usage::{ProviderUsage, QuotaWindow};

    fn make_provider(id: &str, name: &str, rem: f64) -> ProviderUsage {
        ProviderUsage {
            id: id.into(),
            provider: name.into(),
            subtitle: String::new(),
            plan: None,
            environment: None,
            status: "available".into(),
            windows: vec![QuotaWindow {
                id: "5h".into(),
                label: "5h".into(),
                used_percent: Some(100.0 - rem),
                remaining_percent: Some(rem),
                window_minutes: Some(300),
                resets_at_unix_seconds: Some(2_000_000),
                reset_description: None,
                unavailable_reason: None,
            }],
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
    fn summarize_quotas_highlights_low_and_exhausted_without_fake_data() {
        assert!(summarize_quotas(&[], 1_000_000).is_none());

        let healthy = summarize_quotas(&[make_provider("claude", "Claude", 65.0)], 1_000_000)
            .expect("quota disponível");
        assert_eq!(healthy.severity, 0);
        assert_eq!(healthy.label, "IA 65%");

        let low = summarize_quotas(
            &[
                make_provider("claude", "Claude", 65.0),
                make_provider("codex", "Codex", 14.0),
            ],
            1_000_000,
        )
        .expect("alerta laranja");
        assert_eq!(low.severity, 1);
        assert_eq!(low.label, "Codex 14%");

        let exhausted = summarize_quotas(&[make_provider("codex", "Codex", 0.0)], 1_000_000)
            .expect("alerta crítico");
        assert_eq!(exhausted.severity, 2);
        assert_eq!(exhausted.label, "Codex 0%");
    }
}
