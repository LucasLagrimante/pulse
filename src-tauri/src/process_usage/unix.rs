//! CPU por processo em Linux e macOS via `sysinfo`.
//!
//! O `sysinfo` calcula o delta de tempo de CPU entre dois `refresh` do mesmo
//! processo, em "% de um núcleo". Aqui ele é normalizado pelo número de
//! processadores lógicos (100% = máquina inteira, mesma semântica do
//! Gerenciador de Tarefas e do backend Windows).
//!
//! O primeiro `refresh` de um PID não tem delta e o `sysinfo` devolve 0: esse
//! zero não é medição. Só entram no mapa PIDs amostrados na chamada anterior
//! com o mesmo instante de início (protege contra reuso de PID).

use super::round_usage;
use std::collections::HashMap;
use std::sync::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

struct CpuState {
    system: System,
    /// PID → instante de início, da chamada anterior.
    previous: HashMap<u32, u64>,
}

static STATE: Mutex<Option<CpuState>> = Mutex::new(None);

/// Acima disso o `System` interno é recriado para podar PIDs mortos que
/// saíram da lista pedida (o `sysinfo` só remove mortos entre os pedidos).
const MAX_TRACKED: usize = 2048;

pub fn cpu_percent_by_pid(pids: &[u32]) -> HashMap<u32, f64> {
    let logical_processors = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1) as f64;

    let Ok(mut guard) = STATE.lock() else {
        return HashMap::new();
    };
    let state = guard.get_or_insert_with(|| CpuState {
        system: System::new(),
        previous: HashMap::new(),
    });
    if state.system.processes().len() > MAX_TRACKED {
        state.system = System::new();
        state.previous.clear();
    }

    let targets: Vec<Pid> = pids.iter().map(|pid| Pid::from_u32(*pid)).collect();
    state.system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&targets),
        true,
        ProcessRefreshKind::nothing().with_cpu().without_tasks(),
    );

    let mut usage = HashMap::new();
    let mut current = HashMap::with_capacity(pids.len());
    for &pid in pids {
        let Some(process) = state.system.process(Pid::from_u32(pid)) else {
            continue;
        };
        let started = process.start_time();
        if state.previous.get(&pid) == Some(&started) {
            let percent = process.cpu_usage() as f64 / logical_processors;
            if percent.is_finite() {
                usage.insert(pid, round_usage(percent.clamp(0.0, 100.0)));
            }
        }
        current.insert(pid, started);
    }
    state.previous = current;
    usage
}

#[cfg(test)]
mod tests {
    use super::cpu_percent_by_pid;

    // Um único teste: o estado é global e testes paralelos trocariam a
    // lista de PIDs amostrados entre si.
    #[test]
    fn mede_a_partir_da_segunda_amostra_e_nunca_inventa_zero() {
        let own = std::process::id();
        let missing = i32::MAX as u32 - 7;
        let _ = cpu_percent_by_pid(&[own, missing]);
        let mut acc = 0u64;
        for i in 0..5_000_000u64 {
            acc = acc.wrapping_add(i * i);
        }
        std::hint::black_box(acc);
        std::thread::sleep(std::time::Duration::from_millis(250));
        let second = cpu_percent_by_pid(&[own, missing]);
        let value = second.get(&own).copied().expect("segunda amostra mede");
        assert!((0.0..=100.0).contains(&value));
        assert!(!second.contains_key(&missing));
    }
}
