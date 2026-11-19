//! Uso de GPU por processo no Linux.
//!
//! - Intel (i915/xe), AMD (amdgpu) e demais drivers DRM modernos publicam o
//!   tempo ocupado de cada engine por cliente em `/proc/<pid>/fdinfo/<fd>`
//!   (`drm-engine-<nome>: <ns> ns`). O uso é o delta desse tempo dividido
//!   pela janela entre coletas (e pela `drm-engine-capacity-*`, quando a
//!   classe tem mais de uma engine). O `xe` publica ciclos em vez de ns
//!   (`drm-cycles-*` / `drm-total-cycles-*`), tratados do mesmo jeito.
//! - NVIDIA (driver proprietário) não preenche o fdinfo: o uso vem de
//!   `nvidia-smi pmon -c 1 -s u` (SM/encoder/decoder por processo). Se a GPU
//!   dedicada estiver suspensa (runtime PM em notebooks híbridos), o
//!   `nvidia-smi` não é chamado — ele acordaria a placa — e a GPU conta como
//!   ociosa, que é o estado real informado pelo kernel.
//!
//! Engines viram `GpuEngineSample` e passam pela mesma agregação do Windows
//! (engine mais ocupada por processo; soma dos processos por engine).

use super::{aggregate_gpu_usage, GpuEngineSample, GpuUsage};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Releitura completa das tabelas de descritores (cara): a cada 15 s. Entre
/// elas só o fdinfo dos descritores DRM já conhecidos é relido.
const FD_RESCAN_INTERVAL: Duration = Duration::from_secs(15);
/// Primeira coleta sem histórico: segunda leitura após esta janela.
const FIRST_WINDOW: Duration = Duration::from_millis(150);

/// Leitura de uma engine num cliente DRM.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineReading {
    /// Tempo ocupado (ns) ou ciclos ocupados.
    pub busy: u64,
    /// Ciclos totais da GPU (só drivers baseados em ciclos, como `xe`).
    pub total_cycles: Option<u64>,
    /// Quantas engines da classe o valor soma (`drm-engine-capacity-*`).
    pub capacity: u64,
}

/// Conteúdo relevante de um `fdinfo` DRM.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmFdInfo {
    pub pdev: String,
    pub client_id: u64,
    pub engines: HashMap<String, EngineReading>,
}

/// Interpreta um `/proc/<pid>/fdinfo/<fd>`. `None` quando o descritor não é
/// de um cliente DRM com contabilidade de engines.
pub fn parse_drm_fdinfo(text: &str) -> Option<DrmFdInfo> {
    let mut pdev = None;
    let mut client_id = None;
    let mut busy_ns: HashMap<String, u64> = HashMap::new();
    let mut cycles: HashMap<String, u64> = HashMap::new();
    let mut total_cycles: HashMap<String, u64> = HashMap::new();
    let mut capacity: HashMap<String, u64> = HashMap::new();

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        let number = || value.split_whitespace().next()?.parse::<u64>().ok();
        if key == "drm-pdev" {
            pdev = Some(value.to_owned());
        } else if key == "drm-client-id" {
            client_id = number();
        } else if let Some(engine) = key.strip_prefix("drm-engine-capacity-") {
            if let Some(n) = number() {
                capacity.insert(engine.to_owned(), n.max(1));
            }
        } else if let Some(engine) = key.strip_prefix("drm-engine-") {
            if value.ends_with("ns") {
                if let Some(n) = number() {
                    busy_ns.insert(engine.to_owned(), n);
                }
            }
        } else if let Some(engine) = key.strip_prefix("drm-total-cycles-") {
            if let Some(n) = number() {
                total_cycles.insert(engine.to_owned(), n);
            }
        } else if let Some(engine) = key.strip_prefix("drm-cycles-") {
            if let Some(n) = number() {
                cycles.insert(engine.to_owned(), n);
            }
        }
    }

    let mut engines = HashMap::new();
    for (engine, busy) in busy_ns {
        let capacity = capacity.get(&engine).copied().unwrap_or(1);
        engines.insert(
            engine,
            EngineReading {
                busy,
                total_cycles: None,
                capacity,
            },
        );
    }
    for (engine, busy) in cycles {
        let Some(total) = total_cycles.get(&engine).copied() else {
            continue;
        };
        let capacity = capacity.get(&engine).copied().unwrap_or(1);
        engines.insert(
            engine,
            EngineReading {
                busy,
                total_cycles: Some(total),
                capacity,
            },
        );
    }

    if engines.is_empty() {
        return None;
    }
    Some(DrmFdInfo {
        pdev: pdev?,
        client_id: client_id?,
        engines,
    })
}

/// Percentual de uma engine entre duas leituras do mesmo cliente.
pub fn engine_percent(old: &EngineReading, new: &EngineReading, elapsed: Duration) -> Option<f64> {
    let busy = new.busy.checked_sub(old.busy)? as f64;
    let ratio = match (old.total_cycles, new.total_cycles) {
        (Some(old_total), Some(new_total)) => {
            let total = new_total.checked_sub(old_total)? as f64;
            if total <= 0.0 {
                return None;
            }
            busy / total
        }
        (None, None) => {
            let window = elapsed.as_nanos() as f64;
            if window <= 0.0 {
                return None;
            }
            busy / window
        }
        _ => return None,
    };
    let percent = ratio / new.capacity.max(1) as f64 * 100.0;
    percent.is_finite().then(|| percent.clamp(0.0, 100.0))
}

/// Interpreta `nvidia-smi pmon -c 1 -s u`. Colunas com `-` são ignoradas
/// (sem amostra no intervalo), nunca convertidas em zero.
pub fn parse_nvidia_pmon(text: &str) -> Vec<GpuEngineSample> {
    let mut header: Vec<String> = Vec::new();
    let mut samples = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            if header.is_empty() {
                header = rest.split_whitespace().map(str::to_lowercase).collect();
            }
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        let column = |name: &str| header.iter().position(|h| h == name);
        let (Some(gpu_col), Some(pid_col)) = (column("gpu"), column("pid")) else {
            continue;
        };
        let (Some(gpu), Some(pid)) = (
            fields.get(gpu_col).and_then(|v| v.parse::<u32>().ok()),
            fields.get(pid_col).and_then(|v| v.parse::<u32>().ok()),
        ) else {
            continue;
        };
        for engine in ["sm", "enc", "dec", "jpg", "ofa"] {
            let Some(value) = column(engine)
                .and_then(|index| fields.get(index))
                .and_then(|v| v.parse::<f64>().ok())
            else {
                continue;
            };
            if value.is_finite() && value >= 0.0 {
                samples.push(GpuEngineSample {
                    pid,
                    engine_key: format!("nvidia{gpu}/{engine}"),
                    percent: value.min(100.0),
                });
            }
        }
    }
    samples
}

type DrmClientKey = (String, u64);
type DrmClientMap = HashMap<DrmClientKey, (u32, DrmFdInfo)>;

#[derive(Default)]
struct DrmState {
    /// (pid, fd) de descritores DRM conhecidos.
    fds: Vec<(u32, u32)>,
    scanned_at: Option<Instant>,
    /// Última leitura por cliente: (pdev, client-id) → (pid, engines).
    previous: Option<(Instant, DrmClientMap)>,
}

static DRM_STATE: Mutex<Option<DrmState>> = Mutex::new(None);

/// Algum dispositivo DRM usa driver que publica uso por cliente no fdinfo.
fn has_fdinfo_capable_gpu() -> bool {
    const DRIVERS: [&str; 6] = ["i915", "xe", "amdgpu", "msm", "panfrost", "v3d"];
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            return false;
        }
        std::fs::read_link(entry.path().join("device/driver"))
            .ok()
            .and_then(|driver| driver.file_name().map(|n| n.to_string_lossy().into_owned()))
            .is_some_and(|driver| DRIVERS.contains(&driver.as_str()))
    })
}

/// Varre `/proc/*/fd` atrás de descritores abertos em `/dev/dri/*`.
fn scan_drm_fds() -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return found;
    };
    for (index, entry) in procs.flatten().enumerate() {
        if index % 64 == 63 {
            std::thread::yield_now();
        }
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Some(fd_number) = fd.file_name().to_str().and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if std::fs::read_link(fd.path()).is_ok_and(|target| target.starts_with("/dev/dri/")) {
                found.push((pid, fd_number));
            }
        }
    }
    found
}

fn read_clients(fds: &[(u32, u32)]) -> DrmClientMap {
    let mut clients = HashMap::new();
    for &(pid, fd) in fds {
        let path = format!("/proc/{pid}/fdinfo/{fd}");
        let Ok(text) = std::fs::read_to_string(Path::new(&path)) else {
            continue;
        };
        let Some(info) = parse_drm_fdinfo(&text) else {
            continue;
        };
        // O mesmo cliente pode estar em vários descritores (dup/herança):
        // conta uma vez, atribuído ao primeiro PID visto.
        clients
            .entry((info.pdev.clone(), info.client_id))
            .or_insert((pid, info));
    }
    clients
}

fn drm_samples() -> Option<Vec<GpuEngineSample>> {
    if !has_fdinfo_capable_gpu() {
        return None;
    }
    let mut guard = DRM_STATE.lock().ok()?;
    let state = guard.get_or_insert_with(DrmState::default);

    let rescan = state
        .scanned_at
        .is_none_or(|at| at.elapsed() >= FD_RESCAN_INTERVAL);
    if rescan {
        state.fds = scan_drm_fds();
        state.scanned_at = Some(Instant::now());
    }

    if state.previous.is_none() {
        state.previous = Some((Instant::now(), read_clients(&state.fds)));
        std::thread::sleep(FIRST_WINDOW);
    }

    let now = Instant::now();
    let current = read_clients(&state.fds);
    let (previous_at, previous) = state.previous.take()?;
    let elapsed = now.duration_since(previous_at);

    let mut samples = Vec::new();
    for (key, (pid, info)) in &current {
        let Some((_, old)) = previous.get(key) else {
            continue;
        };
        for (engine, reading) in &info.engines {
            let Some(old_reading) = old.engines.get(engine) else {
                continue;
            };
            if let Some(percent) = engine_percent(old_reading, reading, elapsed) {
                samples.push(GpuEngineSample {
                    pid: *pid,
                    engine_key: format!("{}/{engine}", info.pdev),
                    percent,
                });
            }
        }
    }
    // Descritores que sumiram são podados naturalmente: só o que foi lido
    // agora vira a base da próxima janela.
    state
        .fds
        .retain(|(pid, fd)| Path::new(&format!("/proc/{pid}/fdinfo/{fd}")).exists());
    state.previous = Some((now, current));
    Some(samples)
}

fn nvidia_samples() -> Option<Vec<GpuEngineSample>> {
    match crate::performance::linux::nvidia_power_state() {
        crate::performance::linux::NvidiaPower::Absent => None,
        crate::performance::linux::NvidiaPower::Suspended => Some(Vec::new()),
        crate::performance::linux::NvidiaPower::Awake => {
            let text = crate::performance::linux::run_nvidia_smi(&["pmon", "-c", "1", "-s", "u"])?;
            Some(parse_nvidia_pmon(&text))
        }
    }
}

/// Uso de GPU por PID e total do sistema. `None` quando nenhuma fonte mede
/// (sem driver com fdinfo e sem `nvidia-smi`).
pub fn gpu_usage() -> Option<GpuUsage> {
    let drm = drm_samples();
    let nvidia = nvidia_samples();
    if drm.is_none() && nvidia.is_none() {
        return None;
    }
    let mut samples = drm.unwrap_or_default();
    samples.extend(nvidia.unwrap_or_default());
    Some(aggregate_gpu_usage(&samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    const I915: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t2100\nino:\t732\ndrm-driver:\ti915\ndrm-client-id:\t544\ndrm-pdev:\t0000:00:02.0\ndrm-total-system0:\t0\ndrm-engine-render:\t1500000 ns\ndrm-engine-copy:\t0 ns\ndrm-engine-video:\t0 ns\ndrm-engine-capacity-video:\t2\ndrm-engine-video-enhance:\t0 ns\n";

    #[test]
    fn interpreta_fdinfo_do_i915() {
        let info = parse_drm_fdinfo(I915).unwrap();
        assert_eq!(info.pdev, "0000:00:02.0");
        assert_eq!(info.client_id, 544);
        assert_eq!(info.engines["render"].busy, 1_500_000);
        assert_eq!(info.engines["video"].capacity, 2);
        assert_eq!(info.engines["video-enhance"].busy, 0);
        assert_eq!(info.engines.len(), 4);
    }

    #[test]
    fn interpreta_fdinfo_do_xe_por_ciclos() {
        let text = "drm-driver:\txe\ndrm-client-id:\t9\ndrm-pdev:\t0000:00:02.0\ndrm-cycles-rcs:\t500\ndrm-total-cycles-rcs:\t1000\ndrm-engine-capacity-rcs:\t1\n";
        let info = parse_drm_fdinfo(text).unwrap();
        assert_eq!(info.engines["rcs"].total_cycles, Some(1000));
    }

    #[test]
    fn descritor_sem_engines_nao_e_cliente() {
        assert!(parse_drm_fdinfo("pos:\t0\nflags:\t02100002\nmnt_id:\t27\nino:\t856\n").is_none());
    }

    #[test]
    fn calcula_percentual_por_tempo_e_capacidade() {
        let old = EngineReading {
            busy: 0,
            total_cycles: None,
            capacity: 1,
        };
        let new = EngineReading {
            busy: 50_000_000,
            total_cycles: None,
            capacity: 1,
        };
        assert_eq!(
            engine_percent(&old, &new, Duration::from_millis(100)),
            Some(50.0)
        );
        let new2 = EngineReading { capacity: 2, ..new };
        assert_eq!(
            engine_percent(&old, &new2, Duration::from_millis(100)),
            Some(25.0)
        );
        // Contador que voltou (cliente reaberto) não vira número.
        assert_eq!(engine_percent(&new, &old, Duration::from_millis(100)), None);
    }

    #[test]
    fn calcula_percentual_por_ciclos() {
        let old = EngineReading {
            busy: 100,
            total_cycles: Some(1000),
            capacity: 1,
        };
        let new = EngineReading {
            busy: 400,
            total_cycles: Some(2000),
            capacity: 1,
        };
        assert_eq!(
            engine_percent(&old, &new, Duration::from_millis(1)),
            Some(30.0)
        );
    }

    #[test]
    fn interpreta_pmon_da_nvidia() {
        let text = "# gpu         pid   type     sm    mem    enc    dec    jpg    ofa    command \n# Idx           #    C/G      %      %      %      %      %      %    name \n    0       1367     G      8      8      -      -      -      -    Xorg           \n    0       5867     G      -      -      -      -      -      -    kwalletd5      \n    0       7000     C     40     10      3      -      -      -    python3\n";
        let samples = parse_nvidia_pmon(text);
        assert_eq!(
            samples,
            vec![
                GpuEngineSample {
                    pid: 1367,
                    engine_key: "nvidia0/sm".into(),
                    percent: 8.0
                },
                GpuEngineSample {
                    pid: 7000,
                    engine_key: "nvidia0/sm".into(),
                    percent: 40.0
                },
                GpuEngineSample {
                    pid: 7000,
                    engine_key: "nvidia0/enc".into(),
                    percent: 3.0
                },
            ]
        );
        assert!(parse_nvidia_pmon("").is_empty());
    }

    #[test]
    fn coleta_real_tem_forma_sana() {
        // Nesta máquina há i915 + NVIDIA; em CI sem GPU o resultado é None.
        if let Some(usage) = gpu_usage() {
            assert!((0.0..=100.0).contains(&usage.total_percent));
            assert!(usage.per_pid.values().all(|v| (0.0..=100.0).contains(v)));
        }
    }
}
