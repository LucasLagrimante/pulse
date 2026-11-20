//! Coleta do resumo do sistema em Linux e macOS.
//!
//! - CPU, RAM e processos vêm do `sysinfo` (que lê `/proc` no Linux e as APIs
//!   do kernel no macOS), mantido num `System` persistente para que a CPU seja
//!   sempre um delta entre amostras reais.
//! - O disco equivalente ao "Disco C:" do Windows é o ponto de montagem `/`,
//!   lido por `statvfs` (usado = total − livre, mesma semântica do
//!   `GetDiskFreeSpaceExW`).
//! - Processos são agrupados por executável e usuário: as várias instâncias de
//!   um navegador viram um item, mas o `bash` do root não se mistura com o do
//!   usuário (encerrar o grupo nunca deve atravessar a fronteira de usuário).

use super::TopProcess;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};

const MAX_TOP_PROCESSES: usize = 25;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MIB: f64 = 1024.0 * 1024.0;
/// Contas novas são raras: recarregar `/etc/passwd` no máximo a cada 60 s.
const USERS_TTL: Duration = Duration::from_secs(60);

struct Collector {
    system: System,
    cpu_primed: bool,
    users: Users,
    users_loaded_at: Instant,
}

impl Collector {
    fn new() -> Self {
        Self {
            system: System::new(),
            cpu_primed: false,
            users: Users::new_with_refreshed_list(),
            users_loaded_at: Instant::now(),
        }
    }

    fn user_name(&mut self, uid: &sysinfo::Uid) -> Option<String> {
        if self.users.get_user_by_id(uid).is_none() && self.users_loaded_at.elapsed() > USERS_TTL {
            self.users.refresh();
            self.users_loaded_at = Instant::now();
        }
        self.users
            .get_user_by_id(uid)
            .map(|user| user.name().to_owned())
    }
}

static COLLECTOR: Mutex<Option<Collector>> = Mutex::new(None);

fn with_collector<T>(work: impl FnOnce(&mut Collector) -> T) -> Option<T> {
    let mut guard = COLLECTOR.lock().ok()?;
    let collector = guard.get_or_insert_with(Collector::new);
    Some(work(collector))
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Uso global de CPU (0–100) desde a amostra anterior. Na primeira chamada
/// do processo faz duas leituras separadas pelo intervalo mínimo do
/// `sysinfo`, porque a primeira sozinha não tem delta (seria um zero falso).
pub fn read_cpu_percent() -> Option<f64> {
    with_collector(|collector| {
        collector.system.refresh_cpu_usage();
        if !collector.cpu_primed {
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            collector.system.refresh_cpu_usage();
            collector.cpu_primed = true;
        }
        if collector.system.cpus().is_empty() {
            return None;
        }
        let value = collector.system.global_cpu_usage() as f64;
        value.is_finite().then(|| round1(value.clamp(0.0, 100.0)))
    })
    .flatten()
}

/// `(usado, total)` em GB. Usado = total − disponível (mesma semântica do
/// `GlobalMemoryStatusEx`: cache recuperável não conta como uso).
pub fn read_memory_gb() -> Option<(f64, f64)> {
    with_collector(|collector| {
        collector.system.refresh_memory();
        let total = collector.system.total_memory();
        if total == 0 {
            return None;
        }
        let available = collector.system.available_memory().min(total);
        Some((
            round1((total - available) as f64 / GIB),
            round1(total as f64 / GIB),
        ))
    })
    .flatten()
}

/// `(usado, total)` em GB do sistema de arquivos montado em `/`.
// TODO(macos): validar em hardware — no APFS `/` é o volume de sistema
// selado; o espaço do container é compartilhado com /System/Volumes/Data.
pub fn read_root_disk_gb() -> Option<(f64, f64)> {
    let path = std::ffi::CString::new("/").ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)] // os tipos variam entre Linux e macOS
    let fragment = stat.f_frsize as u64;
    #[allow(clippy::unnecessary_cast)]
    let (blocks, free) = (stat.f_blocks as u64, stat.f_bfree as u64);
    let total = blocks.saturating_mul(fragment);
    if total == 0 {
        return None;
    }
    let free = free.saturating_mul(fragment).min(total);
    Some((
        round1((total - free) as f64 / GIB),
        round1(total as f64 / GIB),
    ))
}

/// Um processo lido do sistema, antes do agrupamento. Separado do `sysinfo`
/// para o agrupamento ser testável com dados sintéticos.
#[derive(Debug, Clone, PartialEq)]
pub struct RawProcess {
    pub pid: u32,
    pub name: String,
    pub exe: Option<String>,
    pub uid: Option<u32>,
    pub user: Option<String>,
    pub memory_bytes: u64,
}

/// O kernel acrescenta " (deleted)" ao link de `/proc/<pid>/exe` quando o
/// binário foi substituído (ex.: atualização de pacote com o app aberto).
pub fn clean_exe_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches(" (deleted)");
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn group_label(process: &RawProcess) -> String {
    process
        .exe
        .as_deref()
        .and_then(|exe| Path::new(exe).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| process.name.clone())
}

/// Agrupa por (executável, usuário), ordena por memória somada e mantém os
/// 25 maiores. O representante do grupo é o membro com mais memória.
pub fn group_processes(processes: Vec<RawProcess>) -> Vec<TopProcess> {
    let mut groups: HashMap<(String, Option<u32>), Vec<RawProcess>> = HashMap::new();
    for process in processes {
        let key = (group_label(&process).to_lowercase(), process.uid);
        groups.entry(key).or_default().push(process);
    }

    let mut top: Vec<TopProcess> = groups
        .into_values()
        .map(|mut members| {
            members.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes).then(a.pid.cmp(&b.pid)));
            let representative = &members[0];
            let total: u64 = members.iter().map(|member| member.memory_bytes).sum();
            TopProcess {
                name: group_label(representative),
                id: representative.pid,
                memory_mb: round1(total as f64 / MIB),
                process_ids: members.iter().map(|member| member.pid).collect(),
                process_count: members.len(),
                display_name: None,
                publisher: None,
                cpu_percent: None,
                gpu_percent: None,
                icon: None,
                path: representative.exe.clone(),
                system_kind: None,
                user: representative.user.clone(),
            }
        })
        .collect();

    top.sort_by(|a, b| {
        b.memory_mb
            .partial_cmp(&a.memory_mb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    top.truncate(MAX_TOP_PROCESSES);
    top
}

/// Lista os processos (sem threads) e devolve os 25 grupos com mais memória.
pub fn collect_top_processes() -> Result<Vec<TopProcess>, String> {
    let raw = with_collector(|collector| {
        collector.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_user(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        );
        struct PendingProcess {
            pid: u32,
            name: String,
            exe: Option<String>,
            uid: Option<sysinfo::Uid>,
            memory_bytes: u64,
        }

        let mut raw = Vec::with_capacity(collector.system.processes().len());
        let mut entries: Vec<PendingProcess> =
            Vec::with_capacity(collector.system.processes().len());
        for (pid, process) in collector.system.processes() {
            // Threads e threads de kernel não são processos para o usuário.
            if process.thread_kind().is_some() {
                continue;
            }
            let memory = process.memory();
            if memory == 0 {
                continue;
            }
            entries.push(PendingProcess {
                pid: pid.as_u32(),
                name: process.name().to_string_lossy().into_owned(),
                exe: process
                    .exe()
                    .and_then(|exe| clean_exe_path(&exe.to_string_lossy())),
                uid: process.user_id().cloned(),
                memory_bytes: memory,
            });
        }
        for entry in entries {
            let user = entry.uid.as_ref().and_then(|uid| collector.user_name(uid));
            raw.push(RawProcess {
                pid: entry.pid,
                name: entry.name,
                exe: entry.exe,
                uid: entry.uid.map(|uid| *uid),
                user,
                memory_bytes: entry.memory_bytes,
            });
        }
        raw
    })
    .ok_or_else(|| "O coletor de processos está indisponível (lock envenenado)".to_owned())?;

    if raw.is_empty() {
        return Err("O sistema não listou nenhum processo legível".to_owned());
    }
    Ok(group_processes(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pid: u32, name: &str, exe: Option<&str>, uid: u32, mb: u64) -> RawProcess {
        RawProcess {
            pid,
            name: name.to_owned(),
            exe: exe.map(str::to_owned),
            uid: Some(uid),
            user: Some(format!("u{uid}")),
            memory_bytes: mb * 1024 * 1024,
        }
    }

    #[test]
    fn agrupa_por_executavel_e_soma_memoria() {
        let top = group_processes(vec![
            raw(10, "firefox", Some("/usr/lib/firefox/firefox"), 1000, 300),
            raw(
                11,
                "Isolated Web Co",
                Some("/usr/lib/firefox/firefox"),
                1000,
                500,
            ),
            raw(12, "bash", Some("/usr/bin/bash"), 1000, 5),
        ]);
        assert_eq!(top[0].name, "firefox");
        assert_eq!(top[0].id, 11, "representante é o membro com mais memória");
        assert_eq!(top[0].process_ids, vec![11, 10]);
        assert_eq!(top[0].process_count, 2);
        assert_eq!(top[0].memory_mb, 800.0);
        assert_eq!(top[0].path.as_deref(), Some("/usr/lib/firefox/firefox"));
        assert_eq!(top[0].user.as_deref(), Some("u1000"));
    }

    #[test]
    fn nao_mistura_usuarios_no_mesmo_grupo() {
        let top = group_processes(vec![
            raw(1, "bash", Some("/usr/bin/bash"), 0, 5),
            raw(2, "bash", Some("/usr/bin/bash"), 1000, 6),
        ]);
        assert_eq!(top.len(), 2);
        assert!(top.iter().all(|group| group.process_count == 1));
    }

    #[test]
    fn sem_executavel_usa_o_nome_do_processo() {
        let top = group_processes(vec![raw(7, "Xorg", None, 0, 90)]);
        assert_eq!(top[0].name, "Xorg");
        assert!(top[0].path.is_none());
    }

    #[test]
    fn limita_a_vinte_e_cinco_grupos() {
        let processes = (0..40)
            .map(|i| raw(i, &format!("p{i}"), None, 1000, u64::from(i) + 1))
            .collect();
        let top = group_processes(processes);
        assert_eq!(top.len(), 25);
        assert_eq!(top[0].name, "p39");
    }

    #[test]
    fn limpa_sufixo_de_binario_apagado() {
        assert_eq!(
            clean_exe_path("/usr/bin/code (deleted)").as_deref(),
            Some("/usr/bin/code")
        );
        assert_eq!(clean_exe_path("  "), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn coleta_real_tem_forma_sana() {
        let (used, total) = read_memory_gb().expect("memória");
        assert!(total > 0.0 && used <= total);
        let (disk_used, disk_total) = read_root_disk_gb().expect("disco raiz");
        assert!(disk_total > 0.0 && disk_used <= disk_total);
        let cpu = read_cpu_percent().expect("cpu");
        assert!((0.0..=100.0).contains(&cpu));
        let top = collect_top_processes().expect("processos");
        assert!(!top.is_empty() && top.len() <= 25);
        assert!(top
            .iter()
            .all(|p| p.process_ids.contains(&p.id) && p.memory_mb >= 0.0));
        let own = std::process::id();
        assert!(
            top.iter().any(|p| p.process_ids.contains(&own)) || top.len() == 25,
            "o próprio processo de teste deve aparecer quando há menos de 25 grupos"
        );
    }
}
