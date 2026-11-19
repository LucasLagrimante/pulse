use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(windows, target_os = "linux"))]
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::disk_analyzer::{get_cached_disk_scan, DiskItemNode};

#[cfg(windows)]
pub(crate) const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiskItemSummary {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub size_formatted: String,
    pub is_dir: bool,
    pub file_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDiskAnalysisPreview {
    pub provider: String,
    pub provider_name: String,
    pub drive_path: String,
    pub command: String,
    pub prompt: String,
    pub context_file_path: String,
    pub top_directories: Vec<DiskItemSummary>,
    pub top_files: Vec<DiskItemSummary>,
    pub cleanup_candidates: Vec<DiskItemSummary>,
    pub total_candidates_bytes: u64,
    pub total_candidates_formatted: String,
    pub is_available: bool,
    pub environment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTerminalLaunchResult {
    pub success: bool,
    pub provider: String,
    pub command: String,
    pub context_file_path: String,
    pub terminal_type: String,
    pub message: String,
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub fn collect_top_directories(root: &DiskItemNode, limit: usize) -> Vec<DiskItemSummary> {
    let mut dirs = Vec::new();
    fn visit(node: &DiskItemNode, is_root: bool, acc: &mut Vec<DiskItemSummary>) {
        if node.is_dir && !is_root && node.size_bytes > 0 && !node.id.contains("[budget]") {
            acc.push(DiskItemSummary {
                path: node.path.clone(),
                name: if node.name.is_empty() {
                    node.path.clone()
                } else {
                    node.name.clone()
                },
                size_bytes: node.size_bytes,
                size_formatted: format_bytes(node.size_bytes),
                is_dir: true,
                file_count: node.file_count,
            });
        }
        for child in &node.children {
            visit(child, false, acc);
        }
    }

    visit(root, true, &mut dirs);
    dirs.sort_by_key(|item| std::cmp::Reverse(item.size_bytes));
    dirs.truncate(limit);
    dirs
}

pub fn collect_top_files(root: &DiskItemNode, limit: usize) -> Vec<DiskItemSummary> {
    let mut files = Vec::new();
    fn visit(node: &DiskItemNode, acc: &mut Vec<DiskItemSummary>) {
        if !node.is_dir && node.size_bytes > 0 && !node.id.contains("[budget]") {
            acc.push(DiskItemSummary {
                path: node.path.clone(),
                name: if node.name.is_empty() {
                    node.path.clone()
                } else {
                    node.name.clone()
                },
                size_bytes: node.size_bytes,
                size_formatted: format_bytes(node.size_bytes),
                is_dir: false,
                file_count: 1,
            });
        }
        for child in &node.children {
            visit(child, acc);
        }
    }

    visit(root, &mut files);
    files.sort_by_key(|item| std::cmp::Reverse(item.size_bytes));
    files.truncate(limit);
    files
}

pub fn is_cleanup_candidate(path: &str, name: &str, is_dir: bool) -> bool {
    let norm_path = path.to_ascii_lowercase().replace('\\', "/");
    let norm_name = name.to_ascii_lowercase();

    // Locais em que "cache"/"node_modules" fazem parte de software instalado,
    // do sistema ou de dados baixados de propósito: apagar quebra o app (npm
    // global em Program Files, ambiente do Anki, pacotes do sistema) ou força
    // re-download de modelos.
    const NEVER_CANDIDATE: &[&str] = &[
        "/program files/",
        "/program files (x86)/",
        "/appdata/roaming/npm/",
        "/ankiprogramfiles/",
        "/.cache/huggingface",
        "/.ollama/",
        "/.lmstudio/",
        "/usr/",
        "/lib/",
        "/lib64/",
        "/bin/",
        "/sbin/",
        "/etc/",
        "/var/lib/",
        "/opt/",
        "/applications/",
        "/system/library/",
        "/.local/share/flatpak/",
    ];
    let probe = format!("{norm_path}/");
    if NEVER_CANDIDATE
        .iter()
        .any(|pattern| probe.contains(pattern))
    {
        return false;
    }

    if is_dir {
        const SUSPICIOUS_DIRS: &[&str] = &[
            "/temp",
            "/tmp",
            "/.cache",
            "/caches",
            "/node_modules",
            "/.npm/_cacache",
            "/target/debug",
            "/target/release",
            "/.gradle/caches",
            "/.cargo/registry",
            "/.nuget/packages",
            "/crashdumps",
            "/logs",
            "/log",
            "/appdata/local/temp",
            "/.webpack",
            "/.next/cache",
            "/.turbo",
        ];

        for pattern in SUSPICIOUS_DIRS {
            if norm_path.ends_with(pattern) || norm_path.contains(&format!("{pattern}/")) {
                return true;
            }
        }

        matches!(
            norm_name.as_str(),
            "temp" | "tmp" | ".cache" | "cache" | "node_modules" | "crashdumps" | "logs"
        )
    } else {
        const SUSPICIOUS_EXTENSIONS: &[&str] =
            &[".tmp", ".temp", ".log", ".dmp", ".old", ".bak", ".chk"];

        for ext in SUSPICIOUS_EXTENSIONS {
            if norm_name.ends_with(ext) {
                return true;
            }
        }

        // Instaladores e imagens grandes em Downloads ou pastas temporárias
        if (norm_path.contains("/downloads/")
            || norm_path.contains("/temp/")
            || norm_path.contains("/tmp/"))
            && (norm_name.ends_with(".iso")
                || norm_name.ends_with(".msi")
                || norm_name.ends_with(".deb")
                || norm_name.ends_with(".rpm")
                || norm_name.ends_with(".dmg")
                || norm_name.ends_with(".pkg")
                || norm_name.ends_with(".appimage"))
        {
            return true;
        }

        false
    }
}

pub fn collect_cleanup_candidates(
    root: &DiskItemNode,
    limit: usize,
) -> (Vec<DiskItemSummary>, u64) {
    let mut candidates = Vec::new();
    fn visit(node: &DiskItemNode, is_root: bool, acc: &mut Vec<DiskItemSummary>) {
        if !is_root
            && node.size_bytes > 0
            && !node.id.contains("[budget]")
            && is_cleanup_candidate(&node.path, &node.name, node.is_dir)
        {
            acc.push(DiskItemSummary {
                path: node.path.clone(),
                name: if node.name.is_empty() {
                    node.path.clone()
                } else {
                    node.name.clone()
                },
                size_bytes: node.size_bytes,
                size_formatted: format_bytes(node.size_bytes),
                is_dir: node.is_dir,
                file_count: node.file_count,
            });
            // Se marcamos um diretório inteiro de cache (ex: node_modules),
            // não precisamos descer nele para não duplicar contagem de filhos.
            if node.is_dir {
                return;
            }
        }
        for child in &node.children {
            visit(child, false, acc);
        }
    }

    visit(root, true, &mut candidates);
    candidates.sort_by_key(|item| std::cmp::Reverse(item.size_bytes));
    let total_bytes: u64 = candidates.iter().map(|item| item.size_bytes).sum();
    candidates.truncate(limit);
    (candidates, total_bytes)
}

#[cfg(windows)]
fn command_exists_in_windows(cmd: &str) -> bool {
    for candidate in [
        cmd,
        &format!("{cmd}.exe"),
        &format!("{cmd}.cmd"),
        &format!("{cmd}.bat"),
    ] {
        if let Ok(output) = crate::process::hidden_command("where.exe")
            .arg(candidate)
            .output()
        {
            if output.status.success() {
                return true;
            }
        }
    }
    false
}

/// Procura um executável no `PATH` e em `~/.local/bin` (sessões gráficas
/// iniciadas por `.desktop` nem sempre herdam `~/.local/bin` no `PATH`).
#[cfg(unix)]
fn resolve_unix_program(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    if name.is_empty() || name.contains('/') {
        return None;
    }
    let is_exec = |path: &Path| {
        fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111 != 0))
    };
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let candidate = dir.join(name);
            if is_exec(&candidate) {
                return Some(candidate);
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let local_bin = home.join(".local").join("bin").join(name);
        if is_exec(&local_bin) {
            return Some(local_bin);
        }
    }
    None
}

/// Distros do WSL. Com `running_only`, lista só as que já estão ligadas
/// (`wsl -l --running` não inicia a VM); só uma ação explícita do usuário pode
/// ligar o WSL.
#[cfg(windows)]
fn wsl_distros(running_only: bool) -> Vec<String> {
    let args: &[&str] = if running_only {
        &["-l", "--running", "-q"]
    } else {
        &["-l", "-q"]
    };
    if let Ok(output) = crate::process::hidden_command("wsl.exe")
        .args(args)
        .output()
    {
        if output.status.success() {
            let text = String::from_utf16_lossy(
                &output
                    .stdout
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .collect::<Vec<_>>(),
            );
            let fallback = String::from_utf8_lossy(&output.stdout);
            let content = if text.contains('\n') || text.contains('\r') {
                text
            } else {
                fallback.into_owned()
            };
            let mut distros = content
                .lines()
                .map(|l| {
                    l.trim_matches(|c| c == '\u{feff}' || c == '\0')
                        .trim()
                        .to_owned()
                })
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>();
            distros.sort_unstable();
            distros.dedup();
            return distros;
        }
    }
    Vec::new()
}

/// Quanto a detecção pode mexer no WSL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WslProbe {
    /// Só distros já ligadas: detecção automática (botão e prévia do modal).
    RunningOnly,
    /// Pode ligar o WSL: o usuário clicou em abrir o terminal.
    AllowStart,
}

#[cfg(windows)]
fn command_exists_in_wsl(distro: &str, cmd: &str) -> bool {
    // O nome do binário vai como `$0`, nunca interpolado no script do shell.
    if let Ok(output) = crate::process::hidden_command("wsl.exe")
        .args(["-d", distro, "-e", "sh", "-lc", "command -v \"$0\"", cmd])
        .output()
    {
        return output.status.success() && !output.stdout.is_empty();
    }
    false
}

#[derive(Debug, Clone)]
pub(crate) struct CliDetection {
    pub(crate) is_available: bool,
    pub(crate) is_wsl: bool,
    pub(crate) distro: Option<String>,
    pub(crate) environment: String,
}

/// Executável de cada provedor suportado. Qualquer outro id vindo do IPC é
/// recusado: o valor nunca pode virar nome de programa arbitrário.
pub(crate) fn cli_binary(provider_id: &str) -> Result<&'static str, String> {
    match provider_id {
        "claude" => Ok("claude"),
        "antigravity" => Ok("agy"),
        "codex" => Ok("codex"),
        _ => Err("Provedor de IA não suportado.".into()),
    }
}

/// Argumentos que antecedem o prompt para abrir uma sessão interativa.
/// `claude` e `codex` aceitam o prompt posicional; o `agy` recusa argumento
/// posicional e encerra na hora (o console abre e fecha), exigindo `-i`.
fn cli_prompt_flags(cli_cmd: &str) -> &'static [&'static str] {
    match cli_cmd {
        "agy" => &["-i"],
        _ => &[],
    }
}

/// Script fixo do `sh -lc` no WSL: o CLI chega em `$0` e o prompt em `$@`.
/// `set -f` desliga glob e `IFS=` desliga a divisão de palavras, então nomes de
/// arquivos com `$(...)`, `*` ou espaços chegam literais. Não usa aspas duplas
/// para continuar íntegro quando o comando é colado no Windows PowerShell 5.1.
const WSL_EXEC_SCRIPT: &str = "set -f; IFS=; exec $0 $@";

/// Limite do prompt personalizado: mantém a linha de comando bem abaixo do
/// teto de 32 767 caracteres do `CreateProcessW`.
const MAX_PROMPT_CHARS: usize = 4000;

/// Programa e argumentos já separados (argv). Nunca passa por um shell do
/// Windows, então nenhum caractere do prompt é interpretado como código.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliInvocation {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
}

impl CliInvocation {
    /// Linha equivalente para colar no terminal (PowerShell no Windows,
    /// bash/zsh no Linux e macOS). Cada argumento vai entre aspas simples,
    /// sem expansão de `$`, `` ` `` nem `$(...)`.
    pub(crate) fn display(&self) -> String {
        let mut parts = vec![self.program.clone()];
        #[cfg(windows)]
        parts.extend(self.args.iter().map(|arg| powershell_quote(arg)));
        #[cfg(not(windows))]
        parts.extend(self.args.iter().map(|arg| posix_shell_quote(arg)));
        parts.join(" ")
    }
}

/// Literal de aspas simples do PowerShell. O PowerShell aceita as aspas
/// tipográficas (U+2018..U+201B) como aspas simples, então todas são dobradas.
#[cfg(any(windows, test))]
fn powershell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            quoted.push(ch);
        }
        quoted.push(ch);
    }
    quoted.push('\'');
    quoted
}

/// Literal de aspas simples POSIX (sh/bash/zsh): nada é expandido dentro de
/// `'...'`, e uma aspa simples literal fecha, escapa (`\'`) e reabre.
#[cfg(not(windows))]
fn posix_shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// Normaliza o prompt para trafegar como um único argumento: remove caracteres
/// de controle, troca aspas duplas (que o Windows PowerShell 5.1 não escapa ao
/// repassar argumentos) e barras finais (que escapariam a aspa de fechamento).
pub(crate) fn sanitize_prompt(prompt: &str) -> String {
    let cleaned: String = prompt
        .chars()
        .map(|ch| match ch {
            '"' => '\'',
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .filter(|ch| !ch.is_control())
        .take(MAX_PROMPT_CHARS)
        .collect();
    cleaned.trim().trim_end_matches('\\').trim_end().to_owned()
}

pub(crate) fn detect_cli(cmd_name: &str, probe: WslProbe) -> CliDetection {
    #[cfg(windows)]
    {
        if command_exists_in_windows(cmd_name) {
            return CliDetection {
                is_available: true,
                is_wsl: false,
                distro: None,
                environment: "Windows (nativo)".into(),
            };
        }

        let distros = match probe {
            WslProbe::AllowStart => wsl_distros(false),
            WslProbe::RunningOnly => {
                // Só nomes que também existem em `wsl -l -q`: o texto de "nenhuma distro
                // em execução" nunca vira nome de distro passado a `wsl -d`.
                let installed = wsl_distros(false);
                wsl_distros(true)
                    .into_iter()
                    .filter(|d| installed.contains(d))
                    .collect()
            }
        };
        for distro in distros {
            if command_exists_in_wsl(&distro, cmd_name) {
                return CliDetection {
                    is_available: true,
                    is_wsl: true,
                    distro: Some(distro.clone()),
                    environment: format!("WSL ({distro})"),
                };
            }
        }

        CliDetection {
            is_available: false,
            is_wsl: false,
            distro: None,
            environment: match probe {
                WslProbe::RunningOnly => {
                    "CLI não detectada no Windows nem no WSL em execução (o Pulse não liga o WSL sozinho)".into()
                }
                WslProbe::AllowStart => {
                    "CLI não detectada no PATH (copie o comando manualmente)".into()
                }
            },
        }
    }

    #[cfg(not(windows))]
    {
        let _ = probe;
        if resolve_unix_program(cmd_name).is_some() {
            let environment = if cfg!(target_os = "linux") {
                "Linux (nativo)"
            } else if cfg!(target_os = "macos") {
                "macOS (nativo)"
            } else {
                "Sistema local"
            };
            return CliDetection {
                is_available: true,
                is_wsl: false,
                distro: None,
                environment: environment.into(),
            };
        }

        CliDetection {
            is_available: false,
            is_wsl: false,
            distro: None,
            environment: "CLI não detectada no PATH (copie o comando manualmente)".into(),
        }
    }
}

pub fn generate_markdown_report(
    root: &DiskItemNode,
    top_dirs: &[DiskItemSummary],
    top_files: &[DiskItemSummary],
    candidates: &[DiskItemSummary],
    total_candidates_bytes: u64,
) -> String {
    let mut md = String::new();
    md.push_str("# Relatório de Armazenamento do Pulse — Análise de Disco\n\n");
    md.push_str(&format!("- **Unidade / Raiz**: `{}`\n", root.path));
    md.push_str(&format!(
        "- **Tamanho Total Catalogado**: {}\n",
        format_bytes(root.size_bytes)
    ));
    md.push_str(&format!(
        "- **Total de Itens Examinados**: {} arquivos / {} subpastas\n",
        root.file_count, root.dir_count
    ));
    md.push_str(&format!(
        "- **Estimativa de Limpeza Potencial**: {}\n\n",
        format_bytes(total_candidates_bytes)
    ));

    let os_label = if cfg!(windows) {
        "o Windows"
    } else if cfg!(target_os = "linux") {
        "o Linux"
    } else if cfg!(target_os = "macos") {
        "o macOS"
    } else {
        "o sistema"
    };
    let shell_label = if cfg!(windows) {
        "PowerShell ou CMD"
    } else {
        "terminal (bash/zsh)"
    };

    md.push_str("## 🎯 Objetivo da Análise com IA\n");
    md.push_str(&format!(
        "Avaliar a ocupação de armazenamento, identificar com precisão quais pastas e arquivos são os maiores e apontar de forma segura onde há espaço para limpar o disco e liberar armazenamento n{os_label}.\n\n"
    ));

    md.push_str("## 📂 Top Maiores Pastas\n");
    md.push_str("| Pasta | Tamanho | Arquivos |\n");
    md.push_str("| :--- | :--- | :--- |\n");
    for dir in top_dirs {
        md.push_str(&format!(
            "| `{}` | {} | {} |\n",
            dir.path, dir.size_formatted, dir.file_count
        ));
    }
    md.push('\n');

    md.push_str("## 📄 Top Maiores Arquivos\n");
    md.push_str("| Arquivo | Tamanho |\n");
    md.push_str("| :--- | :--- |\n");
    for file in top_files {
        md.push_str(&format!("| `{}` | {} |\n", file.path, file.size_formatted));
    }
    md.push('\n');

    md.push_str(&format!(
        "## 🧹 Candidatos Potenciais para Limpeza / Caches ({})\n",
        format_bytes(total_candidates_bytes)
    ));
    md.push_str("| Item / Caminho | Tamanho | Tipo |\n");
    md.push_str("| :--- | :--- | :--- |\n");
    for cand in candidates {
        let type_desc = if cand.is_dir {
            "Pasta de Cache / Temp"
        } else {
            "Arquivo Temporário / Log"
        };
        md.push_str(&format!(
            "| `{}` | {} | {} |\n",
            cand.path, cand.size_formatted, type_desc
        ));
    }
    md.push('\n');

    md.push_str("## 📋 Diretrizes para a Resposta da IA\n");
    md.push_str("1. Liste as pastas e arquivos que estão causando o maior consumo desnecessário de disco.\n");
    md.push_str(&format!(
        "2. Indique claramente quais desses itens podem ser apagados com 100% de segurança (sem quebrar programas nem {os_label}).\n"
    ));
    md.push_str("3. Alerte sobre itens que NÃO devem ser removidos sem cautela.\n");
    md.push_str(&format!(
        "4. Forneça comandos práticos e seguros ({shell_label}) prontos para executar para liberar o espaço identificado.\n"
    ));
    md.push_str(
        "5. Salve a resposta completa em um arquivo Markdown no caminho indicado no prompt.\n",
    );

    md
}

/// Sufixo de arquivo por unidade ou raiz (`C:\\` → `C`, `/` → `ROOT`,
/// `/home/user` → `HOME-USER`), comum ao contexto e ao plano.
fn drive_file_suffix(drive_path: &str) -> String {
    let trimmed = drive_path.trim();
    if trimmed == "/" {
        return "ROOT".to_owned();
    }
    let mut chars = trimmed.chars();
    if let (Some(drive), Some(':')) = (chars.next(), chars.next()) {
        if drive.is_ascii_alphabetic() {
            let cleaned = trimmed
                .replace([':', '\\', '/', '.'], "")
                .trim()
                .to_uppercase();
            if !cleaned.is_empty() {
                return cleaned;
            }
        }
    }
    let mut out = String::new();
    let mut last_dash = true;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    let cleaned = out.trim_matches('-');
    if cleaned.is_empty() {
        "DISK".to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn save_context_file(content: &str, drive_path: &str) -> Result<PathBuf, String> {
    let filename = format!("pulse-disk-analysis-{}.md", drive_file_suffix(drive_path));
    let path = env::temp_dir().join(filename);
    fs::write(&path, content)
        .map_err(|error| format!("Não foi possível salvar o arquivo de contexto: {error}"))?;
    Ok(path)
}

/// Plano de limpeza que a IA grava ao lado do relatório de contexto
/// (`pulse-disk-analysis-C.md` → `pulse-disk-cleanup-plan-C.md`).
fn cleanup_plan_path(context_file_path: &Path) -> PathBuf {
    let name = context_file_path
        .file_name()
        .map(|n| {
            n.to_string_lossy()
                .replace("pulse-disk-analysis-", "pulse-disk-cleanup-plan-")
        })
        .unwrap_or_else(|| "pulse-disk-cleanup-plan.md".into());
    context_file_path.with_file_name(name)
}

/// Converte `C:\\Users\\x` em `/mnt/c/Users/x` para o CLI que roda dentro do
/// WSL enxergar o arquivo. Caminhos sem letra de unidade seguem intactos.
pub(crate) fn to_wsl_path(windows_path: &str) -> String {
    let mut chars = windows_path.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            let rest = chars.as_str().replace('\\', "/");
            format!(
                "/mnt/{}/{}",
                drive.to_ascii_lowercase(),
                rest.trim_start_matches('/')
            )
        }
        _ => windows_path.to_owned(),
    }
}

/// Recortes do scan que alimentam o pré-prompt (agrupados para manter a
/// assinatura de `build_prompt_and_command` enxuta).
struct ScanHighlights<'a> {
    top_dirs: &'a [DiskItemSummary],
    top_files: &'a [DiskItemSummary],
    candidates: &'a [DiskItemSummary],
    total_candidates_bytes: u64,
}

fn build_prompt_and_invocation(
    cli_cmd: &str,
    drive_path: &str,
    context_file_path: &Path,
    highlights: &ScanHighlights<'_>,
    custom_prompt: Option<&str>,
    detection: &CliDetection,
) -> (String, CliInvocation) {
    let ScanHighlights {
        top_dirs,
        top_files,
        candidates,
        total_candidates_bytes,
    } = *highlights;

    let dirs_summary = top_dirs
        .iter()
        .take(5)
        .map(|d| format!("{}: {}", d.name, d.size_formatted))
        .collect::<Vec<_>>()
        .join(", ");

    let files_summary = top_files
        .iter()
        .take(5)
        .map(|f| format!("{}: {}", f.name, f.size_formatted))
        .collect::<Vec<_>>()
        .join(", ");

    let candidates_summary = candidates
        .iter()
        .take(4)
        .map(|c| format!("{}: {}", c.name, c.size_formatted))
        .collect::<Vec<_>>()
        .join(", ");

    // No WSL o CLI só enxerga o disco do Windows por `/mnt/<letra>`
    let file_path_str = path_for_cli(context_file_path, detection);
    let plan_path_str = path_for_cli(&cleanup_plan_path(context_file_path), detection);

    let (shell_cmd, temp_label) = if cfg!(windows) {
        ("PowerShell ou CMD", "a pasta TEMP")
    } else {
        ("terminal (bash/zsh)", "a pasta temporária (/tmp)")
    };

    let default_prompt = format!(
        "Analise o relatório de armazenamento do Pulse para a unidade {drive_path}. Objetivo: quais pastas e arquivos são maiores e onde tem espaço seguro para limpar o disco e liberar espaço? Relatório detalhado salvo em: '{file_path_str}'. Top pastas: [{dirs_summary}]. Top arquivos: [{files_summary}]. Candidatos a limpeza ({total_candidates}): [{candidates_summary}]. Forneça comandos de {shell_cmd} práticos e seguros para liberar espaço. Ao limpar {temp_label}, exclua dos comandos as entradas cujo nome começa com 'pulse-' e nunca apague por idade arquivos de dentro de caches de compilação (target do cargo, node_modules, .gradle): remover só parte deles quebra o próximo build; para esses, indique apagar a pasta inteira. Ao final, salve sua resposta completa (maiores consumidores, itens seguros para apagar, alertas do que não remover, comandos e ganho estimado) em Markdown no arquivo '{plan_path_str}', sobrescrevendo se existir, e informe o caminho.",
        total_candidates = format_bytes(total_candidates_bytes)
    );

    // Nomes de pastas e arquivos vêm do disco e o prompt personalizado vem da
    // UI: os dois são dados, nunca código. Por isso o prompt segue como um
    // único argumento (argv) e não é concatenado em linha de shell.
    let prompt = sanitize_prompt(
        custom_prompt
            .filter(|p| !p.trim().is_empty())
            .unwrap_or(&default_prompt),
    );

    let invocation = build_cli_invocation(cli_cmd, &prompt, detection);
    (prompt, invocation)
}

/// Monta o argv do CLI com o prompt já sanitizado: no Windows nativo chama o
/// binário direto; no WSL passa por `wsl.exe -d <distro>` com script fixo.
pub(crate) fn build_cli_invocation(
    cli_cmd: &str,
    prompt: &str,
    detection: &CliDetection,
) -> CliInvocation {
    let cli_args: Vec<String> = cli_prompt_flags(cli_cmd)
        .iter()
        .map(|flag| (*flag).to_owned())
        .chain(std::iter::once(prompt.to_owned()))
        .collect();

    match detection.distro.as_deref().filter(|_| detection.is_wsl) {
        Some(distro) => CliInvocation {
            program: "wsl.exe".into(),
            args: ["-d", distro, "-e", "sh", "-lc", WSL_EXEC_SCRIPT, cli_cmd]
                .into_iter()
                .map(str::to_owned)
                .chain(cli_args)
                .collect(),
        },
        None => CliInvocation {
            program: cli_cmd.to_owned(),
            args: cli_args,
        },
    }
}

/// Caminho que o CLI enxerga: no WSL o disco do Windows fica em `/mnt/<letra>`.
pub(crate) fn path_for_cli(path: &Path, detection: &CliDetection) -> String {
    let raw = path.display().to_string();
    if detection.is_wsl && detection.distro.is_some() {
        to_wsl_path(&raw)
    } else {
        raw
    }
}

/// Nome de exibição do provedor (o id já foi validado por `cli_binary`).
pub(crate) fn provider_display_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude Code",
        "antigravity" => "Antigravity (AGY)",
        _ => "Codex CLI",
    }
}

fn prepare_analysis(
    provider: String,
    custom_prompt: Option<String>,
    probe: WslProbe,
) -> Result<(AiDiskAnalysisPreview, CliInvocation), String> {
    let cli_cmd = cli_binary(&provider)?;
    let cache = get_cached_disk_scan().ok_or_else(|| {
        "Nenhuma análise de disco em cache foi encontrada. Realize uma varredura primeiro."
            .to_string()
    })?;

    let root = &cache.result;
    let top_dirs = collect_top_directories(root, 15);
    let top_files = collect_top_files(root, 15);
    let (candidates, total_candidates) = collect_cleanup_candidates(root, 15);

    let report_content =
        generate_markdown_report(root, &top_dirs, &top_files, &candidates, total_candidates);
    let context_file = save_context_file(&report_content, &root.path)?;

    let detection = detect_cli(cli_cmd, probe);
    // Sem consultar o WSL desligado, "não achei" não prova que o CLI sumiu
    if detection.is_available || probe == WslProbe::AllowStart {
        reconcile_provider_cache(&provider, detection.is_available);
    }
    let (prompt, invocation) = build_prompt_and_invocation(
        cli_cmd,
        &root.path,
        &context_file,
        &ScanHighlights {
            top_dirs: &top_dirs,
            top_files: &top_files,
            candidates: &candidates,
            total_candidates_bytes: total_candidates,
        },
        custom_prompt.as_deref(),
        &detection,
    );

    let provider_name = provider_display_name(&provider).to_string();

    let preview = AiDiskAnalysisPreview {
        provider,
        provider_name,
        drive_path: root.path.clone(),
        command: invocation.display(),
        prompt,
        context_file_path: context_file.display().to_string(),
        top_directories: top_dirs,
        top_files,
        cleanup_candidates: candidates,
        total_candidates_bytes: total_candidates,
        total_candidates_formatted: format_bytes(total_candidates),
        is_available: detection.is_available,
        environment: detection.environment,
    };
    Ok((preview, invocation))
}

/// Provedores na ordem exibida pela interface.
const PROVIDER_IDS: [&str; 3] = ["antigravity", "claude", "codex"];

/// A detecção chama `where.exe` e consulta só as distros WSL já ligadas (nunca
/// liga a VM). Mesmo assim o resultado fica salvo em disco
/// (`%LOCALAPPDATA%\Pulse\ai-cli-providers.json`) e vale por 24 h: abrir o
/// Armazenamento não refaz a busca a cada início do app. O modal detecta de novo
/// ao abrir e corrige o cache se algo mudou.
const PROVIDER_DETECTION_TTL_SECS: u64 = 24 * 60 * 60;
static PROVIDER_DETECTION: Mutex<Option<ProviderDetectionCache>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProviderDetectionCache {
    detected_at_unix_secs: u64,
    providers: Vec<String>,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn provider_cache_file() -> PathBuf {
    crate::disk_analyzer::pulse_data_dir().join("ai-cli-providers.json")
}

/// Só aceita ids conhecidos, na ordem da interface (o arquivo pode ter sido editado).
fn normalize_provider_ids(ids: &[String]) -> Vec<String> {
    PROVIDER_IDS
        .iter()
        .filter(|id| ids.iter().any(|candidate| candidate == *id))
        .map(|id| (*id).to_owned())
        .collect()
}

fn is_cache_fresh(cache: &ProviderDetectionCache, now: u64) -> bool {
    now >= cache.detected_at_unix_secs
        && now - cache.detected_at_unix_secs < PROVIDER_DETECTION_TTL_SECS
}

fn load_provider_cache() -> Option<ProviderDetectionCache> {
    if let Some(cache) = PROVIDER_DETECTION
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
    {
        return Some(cache);
    }
    let raw = fs::read_to_string(provider_cache_file()).ok()?;
    let mut cache: ProviderDetectionCache = serde_json::from_str(&raw).ok()?;
    cache.providers = normalize_provider_ids(&cache.providers);
    if let Ok(mut guard) = PROVIDER_DETECTION.lock() {
        *guard = Some(cache.clone());
    }
    Some(cache)
}

fn store_provider_cache(providers: Vec<String>) {
    let cache = ProviderDetectionCache {
        detected_at_unix_secs: now_unix_secs(),
        providers: normalize_provider_ids(&providers),
    };
    let path = provider_cache_file();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Falha ao gravar só custa uma nova detecção no próximo início
    if let Ok(json) = serde_json::to_string_pretty(&cache) {
        let _ = fs::write(&path, json);
    }
    if let Ok(mut guard) = PROVIDER_DETECTION.lock() {
        *guard = Some(cache);
    }
}

/// Atualiza o cache quando o modal detecta um provedor diferente do salvo
/// (CLI instalado ou removido depois da última detecção).
pub(crate) fn reconcile_provider_cache(provider_id: &str, is_available: bool) {
    let Some(cache) = load_provider_cache() else {
        return;
    };
    let listed = cache.providers.iter().any(|id| id == provider_id);
    if listed == is_available {
        return;
    }
    let mut providers = cache.providers;
    if is_available {
        providers.push(provider_id.to_owned());
    } else {
        providers.retain(|id| id != provider_id);
    }
    store_provider_cache(providers);
}

/// Ids dos provedores cujo CLI está no PATH do Windows ou de alguma distro WSL.
#[tauri::command]
pub async fn detect_ai_cli_providers() -> Result<Vec<String>, String> {
    if let Some(cache) = load_provider_cache() {
        if is_cache_fresh(&cache, now_unix_secs()) {
            return Ok(cache.providers);
        }
    }

    let ids = tauri::async_runtime::spawn_blocking(|| {
        PROVIDER_IDS
            .iter()
            .filter(|id| {
                cli_binary(id).is_ok_and(|cmd| detect_cli(cmd, WslProbe::RunningOnly).is_available)
            })
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| format!("Falha ao detectar os CLIs de IA: {error}"))?;

    store_provider_cache(ids.clone());
    Ok(ids)
}

/// Situação do plano de limpeza que a IA grava a pedido do pré-prompt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiCleanupPlanStatus {
    pub path: String,
    pub exists: bool,
    pub modified_at_unix_ms: Option<u64>,
    pub size_bytes: u64,
}

pub(crate) fn cleanup_plan_status_at(path: &Path) -> AiCleanupPlanStatus {
    let meta = fs::metadata(path)
        .ok()
        .filter(|m| m.is_file() && m.len() > 0);
    AiCleanupPlanStatus {
        path: path.display().to_string(),
        exists: meta.is_some(),
        modified_at_unix_ms: meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
        size_bytes: meta.map_or(0, |m| m.len()),
    }
}

/// Plano da unidade em cache. O caminho é sempre derivado aqui, nunca recebido
/// da UI, para o comando de abrir não virar um "abra qualquer arquivo".
fn cached_cleanup_plan_path() -> Option<PathBuf> {
    let cache = get_cached_disk_scan()?;
    let filename = format!(
        "pulse-disk-cleanup-plan-{}.md",
        drive_file_suffix(&cache.result.path)
    );
    Some(env::temp_dir().join(filename))
}

#[tauri::command]
pub fn get_ai_cleanup_plan_status() -> Option<AiCleanupPlanStatus> {
    cached_cleanup_plan_path().map(|path| cleanup_plan_status_at(&path))
}

#[tauri::command]
pub fn open_ai_cleanup_plan() -> Result<(), String> {
    let path = cached_cleanup_plan_path()
        .filter(|p| p.is_file())
        .ok_or_else(|| "O plano de limpeza ainda não foi gerado pela IA.".to_string())?;
    // Abre no app padrão de .md (o nome termina sempre em `.md`, nunca executa)
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|error| format!("Não foi possível abrir o plano de limpeza: {error}"))
}

#[tauri::command]
pub fn get_ai_disk_analysis_preview(
    provider: String,
    custom_prompt: Option<String>,
) -> Result<AiDiskAnalysisPreview, String> {
    prepare_analysis(provider, custom_prompt, WslProbe::RunningOnly).map(|(preview, _)| preview)
}

/// Caminho completo do CLI no Windows. `Command::new("claude")` só acha
/// `claude.exe`; shims do npm (`claude.cmd`) precisam do caminho com extensão.
#[cfg(windows)]
fn resolve_windows_program(name: &str) -> Option<PathBuf> {
    let output = crate::process::hidden_command("where.exe")
        .arg(name)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let candidates: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();
    let has_ext = |path: &PathBuf, ext: &str| {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
    };
    ["exe", "cmd", "bat"]
        .iter()
        .find_map(|ext| candidates.iter().find(|path| has_ext(path, ext)).cloned())
}

/// Emulador de terminal Linux e os argumentos fixos que antecedem o `argv`
/// do programa (sem `sh -c`, nunca concatenando string de shell).
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LinuxTerminalSpec {
    binary: &'static str,
    prefix_args: &'static [&'static str],
}

#[cfg(target_os = "linux")]
const LINUX_TERMINALS: &[LinuxTerminalSpec] = &[
    LinuxTerminalSpec {
        binary: "konsole",
        prefix_args: &["-e"],
    },
    LinuxTerminalSpec {
        binary: "gnome-terminal",
        prefix_args: &["--"],
    },
    LinuxTerminalSpec {
        binary: "xfce4-terminal",
        prefix_args: &["-x"],
    },
    LinuxTerminalSpec {
        binary: "kitty",
        prefix_args: &["--"],
    },
    LinuxTerminalSpec {
        binary: "alacritty",
        prefix_args: &["-e"],
    },
    LinuxTerminalSpec {
        binary: "wezterm",
        prefix_args: &["start", "--"],
    },
    LinuxTerminalSpec {
        binary: "x-terminal-emulator",
        prefix_args: &["-e"],
    },
    LinuxTerminalSpec {
        binary: "xterm",
        prefix_args: &["-e"],
    },
];

/// Ordem de preferência de terminais conforme `XDG_CURRENT_DESKTOP`.
#[cfg(target_os = "linux")]
fn select_linux_terminal<F>(
    desktop: Option<&str>,
    mut resolver: F,
) -> Option<(PathBuf, &'static [&'static str])>
where
    F: FnMut(&str) -> Option<PathBuf>,
{
    let desktop_upper = desktop.unwrap_or_default().to_ascii_uppercase();
    let preferred: &[&str] = if desktop_upper.contains("KDE") {
        &["konsole"]
    } else if desktop_upper.contains("GNOME") || desktop_upper.contains("CINNAMON") {
        &["gnome-terminal"]
    } else if desktop_upper.contains("XFCE") {
        &["xfce4-terminal"]
    } else {
        &[]
    };

    for &name in preferred {
        if let Some(spec) = LINUX_TERMINALS.iter().find(|s| s.binary == name) {
            if let Some(path) = resolver(spec.binary) {
                return Some((path, spec.prefix_args));
            }
        }
    }
    for spec in LINUX_TERMINALS {
        if preferred.contains(&spec.binary) {
            continue;
        }
        if let Some(path) = resolver(spec.binary) {
            return Some((path, spec.prefix_args));
        }
    }
    None
}

/// Abre o CLI num console novo com o argv pronto. `Ok(true)` = terminal aberto;
/// `Ok(false)` = nenhum terminal automático disponível, onde só o comando é devolvido.
pub(crate) fn spawn_cli_terminal(invocation: &CliInvocation) -> Result<bool, String> {
    #[cfg(windows)]
    {
        // Sem `powershell -Command`, `cmd /c` ou `wt.exe`: o processo recebe o
        // argv pronto e abre o próprio console (o Windows 11 usa o terminal
        // padrão configurado pelo usuário). O std do Rust escapa os argumentos,
        // inclusive para shims `.cmd`, e recusa o que não puder escapar.
        let program = if invocation.program == "wsl.exe" {
            PathBuf::from("wsl.exe")
        } else {
            resolve_windows_program(&invocation.program).ok_or_else(|| {
                format!(
                    "O CLI `{}` não foi encontrado no PATH do Windows. Copie o comando e execute-o manualmente.",
                    invocation.program
                )
            })?
        };

        Command::new(&program)
            .args(&invocation.args)
            .current_dir(env::temp_dir())
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(|error| {
                format!("Não foi possível abrir o terminal ({error}). Copie o comando e execute-o manualmente.")
            })?;
        Ok(true)
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Stdio;

        let program = resolve_unix_program(&invocation.program).ok_or_else(|| {
            format!(
                "O CLI `{}` não foi encontrado no PATH. Copie o comando e execute-o manualmente.",
                invocation.program
            )
        })?;
        let desktop = env::var("XDG_CURRENT_DESKTOP").ok();
        let Some((terminal_bin, prefix_args)) =
            select_linux_terminal(desktop.as_deref(), resolve_unix_program)
        else {
            return Ok(false);
        };

        // Passa o binário resolvido e o argv direto para o emulador (sem `sh -c`).
        let mut child = Command::new(&terminal_bin)
            .args(prefix_args)
            .arg(&program)
            .args(&invocation.args)
            .current_dir(env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                format!("Não foi possível abrir o terminal ({error}). Copie o comando e execute-o manualmente.")
            })?;
        let _ = std::thread::Builder::new()
            .name("pulse-ai-terminal-wait".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(true)
    }

    // TODO(macos): validar em hardware (Terminal.app / iTerm2 sem shell injection).
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        let _ = invocation;
        Ok(false)
    }
}

/// Resultado padrão de `spawn_cli_terminal` para a interface.
pub(crate) fn launch_result(
    launched: bool,
    provider: String,
    command: String,
    context_file_path: String,
    success_message: &str,
) -> AiTerminalLaunchResult {
    if launched {
        let terminal_type = if cfg!(windows) {
            "Console do Windows"
        } else if cfg!(target_os = "linux") {
            "Terminal do Linux"
        } else if cfg!(target_os = "macos") {
            "Terminal do macOS"
        } else {
            "Terminal"
        };
        AiTerminalLaunchResult {
            success: true,
            provider,
            command,
            context_file_path,
            terminal_type: terminal_type.into(),
            message: success_message.into(),
        }
    } else {
        let fallback_message = if cfg!(windows) {
            "Abertura automática disponível apenas no Windows. Copie o comando gerado."
        } else if cfg!(target_os = "linux") {
            "Nenhum emulador de terminal suportado foi encontrado. Copie o comando gerado."
        } else if cfg!(target_os = "macos") {
            "Abertura automática do terminal ainda indisponível no macOS. Copie o comando gerado."
        } else {
            "Abertura automática indisponível neste sistema. Copie o comando gerado."
        };
        AiTerminalLaunchResult {
            success: false,
            provider,
            command,
            context_file_path,
            terminal_type: "Manual".into(),
            message: fallback_message.into(),
        }
    }
}

#[tauri::command]
pub fn launch_ai_disk_terminal(
    provider: String,
    custom_prompt: Option<String>,
) -> Result<AiTerminalLaunchResult, String> {
    let (preview, invocation) =
        prepare_analysis(provider.clone(), custom_prompt, WslProbe::AllowStart)?;
    let launched = spawn_cli_terminal(&invocation)?;
    Ok(launch_result(
        launched,
        provider,
        preview.command,
        preview.context_file_path,
        "Terminal aberto com o CLI e o contexto de armazenamento.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_software_and_models_are_never_cleanup_candidates() {
        assert!(!is_cleanup_candidate(
            r"C:\Program Files\nodejs\node_modules",
            "node_modules",
            true
        ));
        assert!(!is_cleanup_candidate(
            r"C:\Users\dev\AppData\Roaming\npm\node_modules",
            "node_modules",
            true
        ));
        assert!(!is_cleanup_candidate(
            r"C:\Users\dev\AppData\Local\AnkiProgramFiles\cache",
            "cache",
            true
        ));
        assert!(!is_cleanup_candidate(
            r"C:\Users\dev\.cache\huggingface",
            "huggingface",
            true
        ));
        assert!(is_cleanup_candidate(
            r"C:\Users\dev\Projects\app\node_modules",
            "node_modules",
            true
        ));
        assert!(is_cleanup_candidate(r"C:\Windows\Temp", "Temp", true));
    }

    fn sample_tree() -> DiskItemNode {
        DiskItemNode {
            id: "root".into(),
            name: "C:\\".into(),
            path: "C:\\".into(),
            size_bytes: 100_000_000,
            file_count: 50,
            dir_count: 5,
            is_dir: true,
            modified_at_unix_ms: Some(1_700_000_000_000),
            children: vec![
                DiskItemNode {
                    id: "d1".into(),
                    name: "Users".into(),
                    path: "C:\\Users".into(),
                    size_bytes: 80_000_000,
                    file_count: 45,
                    dir_count: 3,
                    is_dir: true,
                    modified_at_unix_ms: Some(1_700_000_000_000),
                    children: vec![
                        DiskItemNode {
                            id: "d1_node_modules".into(),
                            name: "node_modules".into(),
                            path: "C:\\Users\\dev\\node_modules".into(),
                            size_bytes: 50_000_000,
                            file_count: 30,
                            dir_count: 2,
                            is_dir: true,
                            modified_at_unix_ms: Some(1_700_000_000_000),
                            children: vec![],
                        },
                        DiskItemNode {
                            id: "f1".into(),
                            name: "installer.iso".into(),
                            path: "C:\\Users\\dev\\Downloads\\installer.iso".into(),
                            size_bytes: 20_000_000,
                            file_count: 1,
                            dir_count: 0,
                            is_dir: false,
                            modified_at_unix_ms: Some(1_700_000_000_000),
                            children: vec![],
                        },
                    ],
                },
                DiskItemNode {
                    id: "d2_temp".into(),
                    name: "Temp".into(),
                    path: "C:\\Temp".into(),
                    size_bytes: 10_000_000,
                    file_count: 4,
                    dir_count: 0,
                    is_dir: true,
                    modified_at_unix_ms: Some(1_700_000_000_000),
                    children: vec![DiskItemNode {
                        id: "f_log".into(),
                        name: "debug.log".into(),
                        path: "C:\\Temp\\debug.log".into(),
                        size_bytes: 5_000_000,
                        file_count: 1,
                        dir_count: 0,
                        is_dir: false,
                        modified_at_unix_ms: Some(1_700_000_000_000),
                        children: vec![],
                    }],
                },
            ],
        }
    }

    #[test]
    fn formats_bytes_correctly() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50.0 MB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.00 GB");
    }

    #[test]
    fn collects_top_directories_sorted() {
        let tree = sample_tree();
        let top_dirs = collect_top_directories(&tree, 10);
        assert!(!top_dirs.is_empty());
        assert_eq!(top_dirs[0].name, "Users");
        assert_eq!(top_dirs[1].name, "node_modules");
    }

    #[test]
    fn collects_top_files_sorted() {
        let tree = sample_tree();
        let top_files = collect_top_files(&tree, 10);
        assert_eq!(top_files.len(), 2);
        assert_eq!(top_files[0].name, "installer.iso");
        assert_eq!(top_files[1].name, "debug.log");
    }

    #[test]
    fn identifies_cleanup_candidates() {
        let tree = sample_tree();
        let (candidates, total_bytes) = collect_cleanup_candidates(&tree, 10);
        assert!(candidates.iter().any(|c| c.name == "node_modules"));
        assert!(candidates.iter().any(|c| c.name == "Temp"));
        assert!(total_bytes > 0);
    }

    #[test]
    fn generates_complete_markdown_report() {
        let tree = sample_tree();
        let top_dirs = collect_top_directories(&tree, 5);
        let top_files = collect_top_files(&tree, 5);
        let (candidates, total_bytes) = collect_cleanup_candidates(&tree, 5);

        let report =
            generate_markdown_report(&tree, &top_dirs, &top_files, &candidates, total_bytes);
        assert!(report.contains("# Relatório de Armazenamento do Pulse"));
        assert!(report.contains("Top Maiores Pastas"));
        assert!(report.contains("Top Maiores Arquivos"));
        assert!(report.contains("Candidatos Potenciais para Limpeza"));
    }

    #[test]
    fn rejects_unknown_provider() {
        assert!(cli_binary("claude").is_ok());
        assert!(cli_binary("calc.exe").is_err());
        assert!(cli_binary("codex; rm -rf ~").is_err());
    }

    #[test]
    fn powershell_quote_neutralizes_expansion() {
        assert_eq!(powershell_quote("$(calc)"), "'$(calc)'");
        assert_eq!(powershell_quote("it's"), "'it''s'");
        assert_eq!(powershell_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
    }

    #[test]
    fn sanitize_prompt_strips_quotes_controls_and_trailing_backslash() {
        assert_eq!(
            sanitize_prompt("diga \"oi\"\n\u{0}fim\\\\"),
            "diga 'oi' fim"
        );
        assert_eq!(
            sanitize_prompt(&"x".repeat(MAX_PROMPT_CHARS + 50)).len(),
            MAX_PROMPT_CHARS
        );
    }

    #[test]
    fn malicious_file_names_stay_as_single_argument() {
        let tree = DiskItemNode {
            children: vec![DiskItemNode {
                id: "evil".into(),
                name: "$(Start-Process calc) \"; & calc".into(),
                path: "C:\\evil".into(),
                size_bytes: 1_000,
                file_count: 1,
                dir_count: 0,
                is_dir: true,
                modified_at_unix_ms: None,
                children: vec![],
            }],
            ..sample_tree()
        };
        let top_dirs = collect_top_directories(&tree, 5);
        let highlights = ScanHighlights {
            top_dirs: &top_dirs,
            top_files: &[],
            candidates: &[],
            total_candidates_bytes: 0,
        };
        let native = CliDetection {
            is_available: true,
            is_wsl: false,
            distro: None,
            environment: String::new(),
        };
        let (prompt, invocation) = build_prompt_and_invocation(
            "claude",
            "C:\\",
            Path::new("C:\\Temp\\ctx.md"),
            &highlights,
            None,
            &native,
        );
        assert_eq!(invocation.program, "claude");
        assert_eq!(invocation.args, vec![prompt.clone()]);
        assert!(!prompt.contains('"'));
        assert!(invocation.display().starts_with("claude '"));

        let wsl = CliDetection {
            is_wsl: true,
            distro: Some("Ubuntu".into()),
            ..native
        };
        let (_, invocation) = build_prompt_and_invocation(
            "agy",
            "C:\\",
            Path::new("C:\\Temp\\ctx.md"),
            &highlights,
            None,
            &wsl,
        );
        assert_eq!(invocation.program, "wsl.exe");
        assert_eq!(
            &invocation.args[..8],
            [
                "-d",
                "Ubuntu",
                "-e",
                "sh",
                "-lc",
                WSL_EXEC_SCRIPT,
                "agy",
                "-i"
            ]
        );
        assert_eq!(invocation.args.len(), 9);

        // `agy` recusa prompt posicional: no Windows nativo também precisa de `-i`.
        let (prompt, invocation) = build_prompt_and_invocation(
            "agy",
            "C:\\",
            Path::new("C:\\Temp\\ctx.md"),
            &highlights,
            None,
            &CliDetection {
                is_wsl: false,
                distro: None,
                ..wsl.clone()
            },
        );
        assert_eq!(invocation.program, "agy");
        assert_eq!(invocation.args, vec!["-i".to_owned(), prompt]);
    }

    #[test]
    fn default_prompt_asks_to_save_cleanup_plan_with_cli_visible_path() {
        let highlights = ScanHighlights {
            top_dirs: &[],
            top_files: &[],
            candidates: &[],
            total_candidates_bytes: 0,
        };
        let native = CliDetection {
            is_available: true,
            is_wsl: false,
            distro: None,
            environment: String::new(),
        };
        let ctx = Path::new("C:\\Temp\\pulse-disk-analysis-C.md");

        let (prompt, _) =
            build_prompt_and_invocation("claude", "C:\\", ctx, &highlights, None, &native);
        assert!(prompt.contains("'C:\\Temp\\pulse-disk-cleanup-plan-C.md'"));

        let wsl = CliDetection {
            is_wsl: true,
            distro: Some("Ubuntu".into()),
            ..native
        };
        let (prompt, _) =
            build_prompt_and_invocation("claude", "C:\\", ctx, &highlights, None, &wsl);
        assert!(prompt.contains("'/mnt/c/Temp/pulse-disk-analysis-C.md'"));
        assert!(prompt.contains("'/mnt/c/Temp/pulse-disk-cleanup-plan-C.md'"));

        // Prompt personalizado é respeitado sem instrução extra
        let (prompt, _) =
            build_prompt_and_invocation("claude", "C:\\", ctx, &highlights, Some("só liste"), &wsl);
        assert_eq!(prompt, "só liste");
    }

    #[test]
    fn cleanup_plan_status_ignores_missing_and_empty_files() {
        let dir = env::temp_dir().join(format!("pulse-plan-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let plan = dir.join("pulse-disk-cleanup-plan-C.md");

        assert!(!cleanup_plan_status_at(&plan).exists);
        fs::write(&plan, "").unwrap();
        assert!(
            !cleanup_plan_status_at(&plan).exists,
            "arquivo vazio = IA ainda escrevendo"
        );
        fs::write(&plan, "# Plano").unwrap();
        let status = cleanup_plan_status_at(&plan);
        assert!(status.exists);
        assert_eq!(status.size_bytes, 7);
        assert!(status.modified_at_unix_ms.is_some());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn drive_suffix_matches_context_and_plan_names() {
        assert_eq!(drive_file_suffix("c:\\"), "C");
        assert_eq!(
            cleanup_plan_path(Path::new("pulse-disk-analysis-C.md")),
            PathBuf::from(format!(
                "pulse-disk-cleanup-plan-{}.md",
                drive_file_suffix("C:\\")
            ))
        );
    }

    #[test]
    fn to_wsl_path_maps_drive_letter_and_keeps_others() {
        assert_eq!(to_wsl_path("D:\\Dados\\a b.md"), "/mnt/d/Dados/a b.md");
        assert_eq!(to_wsl_path("/tmp/x.md"), "/tmp/x.md");
    }

    #[test]
    fn provider_cache_keeps_only_known_ids_in_ui_order() {
        let ids = vec![
            "codex".to_owned(),
            "calc.exe".to_owned(),
            "antigravity".to_owned(),
        ];
        assert_eq!(
            normalize_provider_ids(&ids),
            vec!["antigravity".to_owned(), "codex".to_owned()]
        );
    }

    #[test]
    fn provider_cache_expires_after_ttl() {
        let cache = ProviderDetectionCache {
            detected_at_unix_secs: 1_000,
            providers: vec![],
        };
        assert!(is_cache_fresh(
            &cache,
            1_000 + PROVIDER_DETECTION_TTL_SECS - 1
        ));
        assert!(!is_cache_fresh(&cache, 1_000 + PROVIDER_DETECTION_TTL_SECS));
        // Relógio voltou no tempo: detecta de novo
        assert!(!is_cache_fresh(&cache, 999));
    }

    #[test]
    fn unix_system_dirs_are_never_cleanup_candidates_but_user_caches_are() {
        assert!(!is_cleanup_candidate(
            "/usr/lib/node_modules",
            "node_modules",
            true
        ));
        assert!(!is_cleanup_candidate(
            "/var/lib/apt/lists/cache",
            "cache",
            true
        ));
        assert!(!is_cleanup_candidate("/opt/vendor/app/logs", "logs", true));
        assert!(!is_cleanup_candidate(
            "/home/u/.local/share/flatpak/repo/tmp",
            "tmp",
            true
        ));
        assert!(!is_cleanup_candidate(
            "/Applications/App.app/Contents/cache",
            "cache",
            true
        ));
        assert!(is_cleanup_candidate(
            "/home/u/projetos/app/node_modules",
            "node_modules",
            true
        ));
        assert!(is_cleanup_candidate("/home/u/.cache/pip", "pip", true));
        assert!(is_cleanup_candidate(
            "/home/u/.npm/_cacache",
            "_cacache",
            true
        ));
        assert!(is_cleanup_candidate(
            "/home/u/Downloads/linuxmint.iso",
            "linuxmint.iso",
            false
        ));
        assert!(is_cleanup_candidate(
            "/home/u/Downloads/pacote.deb",
            "pacote.deb",
            false
        ));
    }

    #[test]
    fn drive_suffix_handles_unix_roots_and_paths_with_spaces() {
        assert_eq!(drive_file_suffix("/"), "ROOT");
        assert_eq!(drive_file_suffix("/home/lucas"), "HOME-LUCAS");
        assert_eq!(
            drive_file_suffix("/media/user/Meu Pendrive"),
            "MEDIA-USER-MEU-PENDRIVE"
        );
        assert_eq!(drive_file_suffix("///"), "DISK");
    }

    #[cfg(not(windows))]
    #[test]
    fn posix_shell_quote_escapes_single_quotes_and_prevents_expansion() {
        assert_eq!(posix_shell_quote("$(id)"), "'$(id)'");
        assert_eq!(posix_shell_quote("it's `ok`"), "'it'\\''s `ok`'");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn select_linux_terminal_prefers_desktop_default_and_falls_back() {
        let has_both = |name: &str| match name {
            "konsole" | "gnome-terminal" => Some(PathBuf::from(format!("/usr/bin/{name}"))),
            _ => None,
        };
        assert_eq!(
            select_linux_terminal(Some("KDE"), has_both),
            Some((PathBuf::from("/usr/bin/konsole"), &["-e"][..]))
        );
        assert_eq!(
            select_linux_terminal(Some("X-Cinnamon"), has_both),
            Some((PathBuf::from("/usr/bin/gnome-terminal"), &["--"][..]))
        );
        let only_xterm = |name: &str| (name == "xterm").then(|| PathBuf::from("/usr/bin/xterm"));
        assert_eq!(
            select_linux_terminal(Some("KDE"), only_xterm),
            Some((PathBuf::from("/usr/bin/xterm"), &["-e"][..]))
        );
        assert_eq!(select_linux_terminal(Some("KDE"), |_| None), None);
    }

    /// Coleta real no Linux: `detect_cli` nunca anuncia "Windows (nativo)",
    /// o cache fica sob `pulse_data_dir()` e o relatório cita Linux/bash.
    #[cfg(target_os = "linux")]
    #[test]
    fn real_linux_cli_detection_and_report_use_linux_context() {
        let found = detect_cli("sh", WslProbe::RunningOnly);
        assert!(found.is_available);
        assert!(!found.is_wsl);
        assert_eq!(found.environment, "Linux (nativo)");

        let missing = detect_cli("pulse-cli-inexistente-9999", WslProbe::RunningOnly);
        assert!(!missing.is_available);
        assert!(!missing.environment.contains("Windows"));

        assert_eq!(
            provider_cache_file(),
            crate::disk_analyzer::pulse_data_dir().join("ai-cli-providers.json")
        );

        let tree = sample_tree();
        let report = generate_markdown_report(&tree, &[], &[], &[], 0);
        assert!(report.contains("no Linux"));
        assert!(report.contains("terminal (bash/zsh)"));
        assert!(!report.contains("PowerShell"));
    }
}
