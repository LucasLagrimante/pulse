use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::disk_analyzer::{get_cached_disk_scan, DiskItemNode};

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

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
                name: if node.name.is_empty() { node.path.clone() } else { node.name.clone() },
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
                name: if node.name.is_empty() { node.path.clone() } else { node.name.clone() },
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

    if is_dir {
        const SUSPICIOUS_DIRS: &[&str] = &[
            "/temp",
            "/tmp",
            "/.cache",
            "/caches",
            "/node_modules",
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
        const SUSPICIOUS_EXTENSIONS: &[&str] = &[
            ".tmp", ".temp", ".log", ".dmp", ".old", ".bak", ".chk",
        ];

        for ext in SUSPICIOUS_EXTENSIONS {
            if norm_name.ends_with(ext) {
                return true;
            }
        }

        // Instaladores e imagens grandes em Downloads ou Temp
        if (norm_path.contains("/downloads/") || norm_path.contains("/temp/"))
            && (norm_name.ends_with(".iso") || norm_name.ends_with(".msi"))
        {
            return true;
        }

        false
    }
}

pub fn collect_cleanup_candidates(root: &DiskItemNode, limit: usize) -> (Vec<DiskItemSummary>, u64) {
    let mut candidates = Vec::new();
    fn visit(node: &DiskItemNode, is_root: bool, acc: &mut Vec<DiskItemSummary>) {
        if !is_root
            && node.size_bytes > 0
            && !node.id.contains("[budget]")
            && is_cleanup_candidate(&node.path, &node.name, node.is_dir)
        {
            acc.push(DiskItemSummary {
                path: node.path.clone(),
                name: if node.name.is_empty() { node.path.clone() } else { node.name.clone() },
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

fn command_exists_in_windows(cmd: &str) -> bool {
    #[cfg(windows)]
    {
        for candidate in [cmd, &format!("{cmd}.exe"), &format!("{cmd}.cmd"), &format!("{cmd}.bat")] {
            if let Ok(output) = crate::process::hidden_command("where.exe")
                .arg(candidate)
                .output()
            {
                if output.status.success() {
                    return true;
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(output) = Command::new("which").arg(cmd).output() {
            if output.status.success() {
                return true;
            }
        }
    }
    false
}

fn wsl_distros() -> Vec<String> {
    #[cfg(windows)]
    {
        if let Ok(output) = crate::process::hidden_command("wsl.exe")
            .args(["-l", "-q"])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf16_lossy(
                    &output
                        .stdout
                        .chunks_exact(2)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect::<Vec<_>>(),
                );
                let fallback = String::from_utf8_lossy(&output.stdout);
                let content = if text.contains('\n') || text.contains('\r') { text } else { fallback.into_owned() };
                let mut distros = content
                    .lines()
                    .map(|l| l.trim_matches(|c| c == '\u{feff}' || c == '\0').trim().to_owned())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>();
                distros.sort_unstable();
                distros.dedup();
                return distros;
            }
        }
    }
    Vec::new()
}

fn command_exists_in_wsl(distro: &str, cmd: &str) -> bool {
    #[cfg(windows)]
    {
        // O nome do binário vai como `$0`, nunca interpolado no script do shell.
        if let Ok(output) = crate::process::hidden_command("wsl.exe")
            .args(["-d", distro, "-e", "sh", "-lc", "command -v \"$0\"", cmd])
            .output()
        {
            return output.status.success() && !output.stdout.is_empty();
        }
    }
    let _ = (distro, cmd);
    false
}

#[derive(Debug, Clone)]
struct CliDetection {
    is_available: bool,
    is_wsl: bool,
    distro: Option<String>,
    environment: String,
}

/// Executável de cada provedor suportado. Qualquer outro id vindo do IPC é
/// recusado: o valor nunca pode virar nome de programa arbitrário.
fn cli_binary(provider_id: &str) -> Result<&'static str, String> {
    match provider_id {
        "claude" => Ok("claude"),
        "antigravity" => Ok("agy"),
        "codex" => Ok("codex"),
        _ => Err("Provedor de IA não suportado.".into()),
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
struct CliInvocation {
    program: String,
    args: Vec<String>,
}

impl CliInvocation {
    /// Linha equivalente para colar no PowerShell. Cada argumento vai entre
    /// aspas simples, onde o PowerShell não expande `$`, `` ` `` nem `$(...)`.
    fn display(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().map(|arg| powershell_quote(arg)));
        parts.join(" ")
    }
}

/// Literal de aspas simples do PowerShell. O PowerShell aceita as aspas
/// tipográficas (U+2018..U+201B) como aspas simples, então todas são dobradas.
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

/// Normaliza o prompt para trafegar como um único argumento: remove caracteres
/// de controle, troca aspas duplas (que o Windows PowerShell 5.1 não escapa ao
/// repassar argumentos) e barras finais (que escapariam a aspa de fechamento).
fn sanitize_prompt(prompt: &str) -> String {
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

fn detect_cli(cmd_name: &str) -> CliDetection {
    if command_exists_in_windows(cmd_name) {
        return CliDetection {
            is_available: true,
            is_wsl: false,
            distro: None,
            environment: "Windows (nativo)".into(),
        };
    }

    let distros = wsl_distros();
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
        environment: "CLI não detectada no PATH (copie o comando manualmente)".into(),
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
    md.push_str(&format!("- **Tamanho Total Catalogado**: {}\n", format_bytes(root.size_bytes)));
    md.push_str(&format!("- **Total de Itens Examinados**: {} arquivos / {} subpastas\n", root.file_count, root.dir_count));
    md.push_str(&format!("- **Estimativa de Limpeza Potencial**: {}\n\n", format_bytes(total_candidates_bytes)));

    md.push_str("## 🎯 Objetivo da Análise com IA\n");
    md.push_str("Avaliar a ocupação de armazenamento, identificar com precisão quais pastas e arquivos são os maiores e apontar de forma segura onde há espaço para limpar o disco e liberar armazenamento no Windows.\n\n");

    md.push_str("## 📂 Top Maiores Pastas\n");
    md.push_str("| Pasta | Tamanho | Arquivos |\n");
    md.push_str("| :--- | :--- | :--- |\n");
    for dir in top_dirs {
        md.push_str(&format!("| `{}` | {} | {} |\n", dir.path, dir.size_formatted, dir.file_count));
    }
    md.push('\n');

    md.push_str("## 📄 Top Maiores Arquivos\n");
    md.push_str("| Arquivo | Tamanho |\n");
    md.push_str("| :--- | :--- |\n");
    for file in top_files {
        md.push_str(&format!("| `{}` | {} |\n", file.path, file.size_formatted));
    }
    md.push('\n');

    md.push_str(&format!("## 🧹 Candidatos Potenciais para Limpeza / Caches ({})\n", format_bytes(total_candidates_bytes)));
    md.push_str("| Item / Caminho | Tamanho | Tipo |\n");
    md.push_str("| :--- | :--- | :--- |\n");
    for cand in candidates {
        let type_desc = if cand.is_dir { "Pasta de Cache / Temp" } else { "Arquivo Temporário / Log" };
        md.push_str(&format!("| `{}` | {} | {} |\n", cand.path, cand.size_formatted, type_desc));
    }
    md.push('\n');

    md.push_str("## 📋 Diretrizes para a Resposta da IA\n");
    md.push_str("1. Liste as pastas e arquivos que estão causando o maior consumo desnecessário de disco.\n");
    md.push_str("2. Indique claramente quais desses itens podem ser apagados com 100% de segurança (sem quebrar programas nem o Windows).\n");
    md.push_str("3. Alerte sobre itens que NÃO devem ser removidos sem cautela.\n");
    md.push_str("4. Forneça comandos práticos e seguros (PowerShell ou CMD) prontos para executar para liberar o espaço identificado.\n");

    md
}

fn save_context_file(content: &str, drive_path: &str) -> Result<PathBuf, String> {
    let clean_drive = drive_path
        .replace([':', '\\', '/', '.'], "")
        .trim()
        .to_uppercase();
    let filename = format!("pulse-disk-analysis-{clean_drive}.md");
    let path = env::temp_dir().join(filename);
    fs::write(&path, content)
        .map_err(|error| format!("Não foi possível salvar o arquivo de contexto: {error}"))?;
    Ok(path)
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
    let ScanHighlights { top_dirs, top_files, candidates, total_candidates_bytes } = *highlights;

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

    let file_path_str = context_file_path.display().to_string();

    let default_prompt = format!(
        "Analise o relatório de armazenamento do Pulse para a unidade {drive_path}. Objetivo: quais pastas e arquivos são maiores e onde tem espaço seguro para limpar o disco e liberar espaço? Relatório detalhado salvo em: '{file_path_str}'. Top pastas: [{dirs_summary}]. Top arquivos: [{files_summary}]. Candidatos a limpeza ({total_candidates}): [{candidates_summary}]. Forneça comandos de PowerShell ou CMD práticos e seguros para liberar espaço.",
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

    let invocation = match detection.distro.as_deref().filter(|_| detection.is_wsl) {
        Some(distro) => CliInvocation {
            program: "wsl.exe".into(),
            args: vec![
                "-d".into(),
                distro.to_owned(),
                "-e".into(),
                "sh".into(),
                "-lc".into(),
                WSL_EXEC_SCRIPT.into(),
                cli_cmd.to_owned(),
                prompt.clone(),
            ],
        },
        None => CliInvocation {
            program: cli_cmd.to_owned(),
            args: vec![prompt.clone()],
        },
    };

    (prompt, invocation)
}

fn prepare_analysis(
    provider: String,
    custom_prompt: Option<String>,
) -> Result<(AiDiskAnalysisPreview, CliInvocation), String> {
    let cli_cmd = cli_binary(&provider)?;
    let cache = get_cached_disk_scan()
        .ok_or_else(|| "Nenhuma análise de disco em cache foi encontrada. Realize uma varredura primeiro.".to_string())?;

    let root = &cache.result;
    let top_dirs = collect_top_directories(root, 15);
    let top_files = collect_top_files(root, 15);
    let (candidates, total_candidates) = collect_cleanup_candidates(root, 15);

    let report_content = generate_markdown_report(root, &top_dirs, &top_files, &candidates, total_candidates);
    let context_file = save_context_file(&report_content, &root.path)?;

    let detection = detect_cli(cli_cmd);
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

    let provider_name = match provider.as_str() {
        "claude" => "Claude Code",
        "antigravity" => "Antigravity (AGY)",
        _ => "Codex CLI",
    }
    .to_string();

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

#[tauri::command]
pub fn get_ai_disk_analysis_preview(
    provider: String,
    custom_prompt: Option<String>,
) -> Result<AiDiskAnalysisPreview, String> {
    prepare_analysis(provider, custom_prompt).map(|(preview, _)| preview)
}

/// Caminho completo do CLI no Windows. `Command::new("claude")` só acha
/// `claude.exe`; shims do npm (`claude.cmd`) precisam do caminho com extensão.
#[cfg(windows)]
fn resolve_windows_program(name: &str) -> Option<PathBuf> {
    let output = crate::process::hidden_command("where.exe").arg(name).output().ok()?;
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
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(ext))
    };
    ["exe", "cmd", "bat"]
        .iter()
        .find_map(|ext| candidates.iter().find(|path| has_ext(path, ext)).cloned())
}

#[tauri::command]
pub fn launch_ai_disk_terminal(
    provider: String,
    custom_prompt: Option<String>,
) -> Result<AiTerminalLaunchResult, String> {
    let (preview, invocation) = prepare_analysis(provider.clone(), custom_prompt)?;

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

        Ok(AiTerminalLaunchResult {
            success: true,
            provider,
            command: preview.command,
            context_file_path: preview.context_file_path,
            terminal_type: "Console do Windows".into(),
            message: "Terminal aberto com o CLI e o contexto de armazenamento.".into(),
        })
    }

    #[cfg(not(windows))]
    {
        // Fora do Windows (dev/testes) não há terminal a abrir: só devolve o
        // comando para cópia manual, sem passar nada por `sh -c`.
        let _ = invocation;
        Ok(AiTerminalLaunchResult {
            success: false,
            provider,
            command: preview.command,
            context_file_path: preview.context_file_path,
            terminal_type: "Manual".into(),
            message: "Abertura automática disponível apenas no Windows. Copie o comando gerado.".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    children: vec![
                        DiskItemNode {
                            id: "f_log".into(),
                            name: "debug.log".into(),
                            path: "C:\\Temp\\debug.log".into(),
                            size_bytes: 5_000_000,
                            file_count: 1,
                            dir_count: 0,
                            is_dir: false,
                            modified_at_unix_ms: Some(1_700_000_000_000),
                            children: vec![],
                        },
                    ],
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

        let report = generate_markdown_report(&tree, &top_dirs, &top_files, &candidates, total_bytes);
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
        assert_eq!(sanitize_prompt("diga \"oi\"\n\u{0}fim\\\\"), "diga 'oi' fim");
        assert_eq!(sanitize_prompt(&"x".repeat(MAX_PROMPT_CHARS + 50)).len(), MAX_PROMPT_CHARS);
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

        let wsl = CliDetection { is_wsl: true, distro: Some("Ubuntu".into()), ..native };
        let (_, invocation) = build_prompt_and_invocation(
            "agy",
            "C:\\",
            Path::new("C:\\Temp\\ctx.md"),
            &highlights,
            None,
            &wsl,
        );
        assert_eq!(invocation.program, "wsl.exe");
        assert_eq!(&invocation.args[..7], ["-d", "Ubuntu", "-e", "sh", "-lc", WSL_EXEC_SCRIPT, "agy"]);
        assert_eq!(invocation.args.len(), 8);
    }
}
