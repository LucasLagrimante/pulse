//! Revisão de segurança dos serviços em execução com um CLI de IA local.
//!
//! Mesmo molde do `ai_disk_advisor`: o Pulse **não resolve nada** — grava um
//! relatório Markdown em `%TEMP%` com os serviços ativos, monta um pré-prompt e
//! abre o CLI escolhido (Claude, Antigravity ou Codex). O pré-prompt proíbe a IA
//! de alterar o sistema e de rotular malware sem evidência; o parecer volta num
//! arquivo `.md` que a interface oferece abrir.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::ai_disk_advisor::{
    build_cli_invocation, cleanup_plan_status_at, cli_binary, detect_cli, launch_result,
    path_for_cli, provider_display_name, reconcile_provider_cache, sanitize_prompt,
    spawn_cli_terminal, AiCleanupPlanStatus, AiTerminalLaunchResult, CliInvocation, WslProbe,
};
use crate::services::{
    extract_executable, is_windows_system_dir, snapshot_for_analysis, ServicesSnapshot,
    WindowsService,
};

const CONTEXT_FILE_NAME: &str = "pulse-services-security-context.md";
const REVIEW_FILE_NAME: &str = "pulse-services-security-review.md";
/// Quantos serviços entram no resumo do prompt e na prévia da interface.
const PROMPT_HIGHLIGHTS: usize = 8;

/// Serviço em execução que vai para a revisão da IA.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSummary {
    pub name: String,
    pub display_name: String,
    pub executable_path: Option<String>,
    pub start_name: Option<String>,
    pub start_type: String,
    pub process_id: Option<u32>,
    pub publisher: Option<String>,
    pub category: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiServicesAnalysisPreview {
    pub provider: String,
    pub provider_name: String,
    pub command: String,
    pub prompt: String,
    pub context_file_path: String,
    /// Serviços em execução no snapshot.
    pub running_count: usize,
    /// Serviços em execução fora das pastas do Windows ou sem classificação de sistema.
    pub analyzed_count: usize,
    pub review_count: usize,
    pub without_publisher_count: usize,
    /// Primeiros da fila de revisão (sinais primeiro), para a prévia.
    pub highlighted: Vec<ServiceSummary>,
    pub collected_at_unix_ms: u64,
    pub is_available: bool,
    pub environment: String,
}

fn is_running(service: &WindowsService) -> bool {
    service.state == "Em execução"
}

fn executable_of(service: &WindowsService) -> Option<String> {
    service
        .executable_path
        .as_deref()
        .map(extract_executable)
        .filter(|path| !path.is_empty())
}

fn has_publisher(service: &WindowsService) -> bool {
    service
        .publisher
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

/// Entra na revisão detalhada todo serviço ativo que **não** é do próprio
/// Windows: executável fora de `System32`/`SysWOW64`, sem executável conhecido,
/// ou classificado como usuário/revisão/sem dados. É onde mora software de
/// terceiros — inclusive o que se instala como serviço para persistir.
pub fn needs_detailed_review(service: &WindowsService) -> bool {
    if service.category != "system" {
        return true;
    }
    match executable_of(service) {
        Some(path) => !is_windows_system_dir(&path),
        None => true,
    }
}

fn category_rank(category: &str) -> u8 {
    match category {
        "review" => 0,
        "unknown" => 1,
        "user" => 2,
        _ => 3,
    }
}

fn summarize(service: &WindowsService) -> ServiceSummary {
    ServiceSummary {
        name: service.name.clone(),
        display_name: service.display_name.clone(),
        executable_path: service.executable_path.clone(),
        start_name: service.start_name.clone(),
        start_type: service.start_type.clone(),
        process_id: service.process_id,
        publisher: service.publisher.clone().filter(|p| !p.trim().is_empty()),
        category: service.category.to_string(),
        reasons: service.suspicion_reasons.clone(),
    }
}

/// Separa os serviços ativos em fila de revisão (sinais e sem fabricante
/// primeiro) e demais serviços do Windows.
pub fn partition_running(
    services: &[WindowsService],
) -> (Vec<ServiceSummary>, Vec<ServiceSummary>) {
    let mut detailed: Vec<&WindowsService> = Vec::new();
    let mut windows: Vec<&WindowsService> = Vec::new();
    for service in services.iter().filter(|s| is_running(s)) {
        if needs_detailed_review(service) {
            detailed.push(service);
        } else {
            windows.push(service);
        }
    }
    detailed.sort_by(|a, b| {
        category_rank(a.category)
            .cmp(&category_rank(b.category))
            .then(has_publisher(a).cmp(&has_publisher(b)))
            .then_with(|| {
                a.display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase())
            })
    });
    windows.sort_by_key(|s| s.name.to_lowercase());
    (
        detailed.into_iter().map(summarize).collect(),
        windows.into_iter().map(summarize).collect(),
    )
}

fn category_label(category: &str) -> &'static str {
    match category {
        "system" => "Sistema",
        "user" => "Usuário",
        "review" => "Revisão",
        _ => "Sem dados",
    }
}

/// Célula de tabela Markdown: `|` e quebras de linha não podem deslocar colunas.
fn md_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_owned()
}

fn or_unavailable(value: Option<&str>) -> String {
    value
        .map(md_cell)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Indisponível".into())
}

pub fn generate_services_report(
    snapshot: &ServicesSnapshot,
    detailed: &[ServiceSummary],
    windows: &[ServiceSummary],
) -> String {
    let running = detailed.len() + windows.len();
    let mut md = String::new();
    md.push_str("# Relatório do Pulse — Revisão de Segurança dos Serviços em Execução\n\n");
    md.push_str(&format!(
        "- **Serviços em execução**: {running} (de {} registrados)\n",
        snapshot.total_count
    ));
    md.push_str(&format!(
        "- **Fora das pastas do Windows / a revisar**: {}\n",
        detailed.len()
    ));
    md.push_str(&format!(
        "- **Com sinal forte do Pulse (categoria Revisão)**: {}\n",
        detailed.iter().filter(|s| s.category == "review").count()
    ));
    md.push_str(&format!(
        "- **Sem fabricante declarado**: {}\n\n",
        detailed.iter().filter(|s| s.publisher.is_none()).count()
    ));

    md.push_str("## 🎯 Objetivo da Análise com IA\n");
    md.push_str("Avaliar se algum serviço em execução indica keylogger, spyware, acesso remoto não autorizado, minerador, persistência maliciosa ou outro invasor — e separar isso, com evidência, do software legítimo.\n\n");

    md.push_str("## ⚠️ Limites dos Dados Coletados\n");
    md.push_str("- **Fabricante** é o `CompanyName` auto-declarado nos recursos de versão do executável. **Não é assinatura digital validada.**\n");
    md.push_str("- A categoria **Revisão** só aparece com sinal forte (executável em pasta gravável pelo usuário ou nome imitando componente do Windows). Ausência dela não prova que o serviço é seguro.\n");
    md.push_str("- Serviços hospedados no `svchost.exe` aparecem com o executável do Windows; a DLL real (`ServiceDll`) não foi coletada e pode ser lida em `HKLM\\SYSTEM\\CurrentControlSet\\Services\\<nome>\\Parameters`.\n\n");

    md.push_str("## 🔍 Serviços a Revisar (fora das pastas do Windows)\n");
    if detailed.is_empty() {
        md.push_str("Nenhum serviço em execução fora de `System32`/`SysWOW64`.\n\n");
    } else {
        md.push_str("| Serviço | Nome de exibição | Executável | Conta | Início | PID | Fabricante declarado | Categoria | Sinais do Pulse |\n");
        md.push_str("| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |\n");
        for service in detailed {
            let reasons = if service.reasons.is_empty() {
                "—".to_string()
            } else {
                md_cell(&service.reasons.join("; "))
            };
            md.push_str(&format!(
                "| `{}` | {} | `{}` | {} | {} | {} | {} | {} | {} |\n",
                md_cell(&service.name),
                md_cell(&service.display_name),
                or_unavailable(service.executable_path.as_deref()),
                or_unavailable(service.start_name.as_deref()),
                md_cell(&service.start_type),
                service
                    .process_id
                    .map_or_else(|| "Indisponível".to_string(), |pid| pid.to_string()),
                or_unavailable(service.publisher.as_deref()),
                category_label(&service.category),
                reasons,
            ));
        }
        md.push('\n');
    }

    md.push_str("## 🪟 Demais Serviços em Execução (executável em System32/SysWOW64)\n");
    md.push_str("Verifique nomes que imitam serviços legítimos do Windows ou que não deveriam estar ativos.\n\n");
    md.push_str("| Serviço | Nome de exibição | Executável | Conta |\n");
    md.push_str("| :--- | :--- | :--- | :--- |\n");
    for service in windows {
        md.push_str(&format!(
            "| `{}` | {} | `{}` | {} |\n",
            md_cell(&service.name),
            md_cell(&service.display_name),
            or_unavailable(service.executable_path.as_deref()),
            or_unavailable(service.start_name.as_deref()),
        ));
    }
    md.push('\n');

    md.push_str("## 📋 Diretrizes para a Resposta da IA\n");
    md.push_str("1. **Somente investigação.** Use apenas comandos de leitura (ex.: `Get-AuthenticodeSignature`, `Get-FileHash`, `Get-CimInstance Win32_Service`, `Get-ItemProperty` no registro, `Get-Process`, `Get-NetTCPConnection`). **Não pare, desative, exclua nem altere** serviços, arquivos ou registro.\n");
    md.push_str("2. Classifique cada serviço revisado como **Legítimo**, **Requer investigação** ou **Indício forte**, sempre com a evidência verificável. Nunca rotule como malware sem evidência.\n");
    md.push_str("3. Procure sinais típicos: assinatura ausente ou inválida, executável em pasta gravável (`AppData`, `Temp`, `ProgramData` fora de fabricante conhecido), nome imitando componente do Windows, conexões de rede inesperadas, ganchos de teclado/captura de tela, ferramentas de acesso remoto não reconhecidas.\n");
    md.push_str("4. Para o que exigir ação, descreva o passo seguro e reversível para o usuário decidir (sem executar).\n");
    md.push_str("5. Salve o parecer completo em Markdown no caminho indicado no prompt.\n");

    md
}

fn context_file_path() -> PathBuf {
    env::temp_dir().join(CONTEXT_FILE_NAME)
}

/// Caminho do parecer da IA: sempre derivado aqui, nunca recebido da UI.
fn review_file_path() -> PathBuf {
    env::temp_dir().join(REVIEW_FILE_NAME)
}

fn names_summary(services: &[ServiceSummary]) -> String {
    services
        .iter()
        .take(PROMPT_HIGHLIGHTS)
        .map(|s| {
            let label = if s.display_name.trim().is_empty() {
                s.name.as_str()
            } else {
                s.display_name.as_str()
            };
            match s.publisher.as_deref() {
                Some(publisher) => format!("{label} ({publisher})"),
                None => format!("{label} (sem fabricante)"),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn default_prompt(
    context_path: &str,
    review_path: &str,
    detailed: &[ServiceSummary],
    running: usize,
) -> String {
    let review_count = detailed.iter().filter(|s| s.category == "review").count();
    format!(
        "Faça uma revisão de segurança dos serviços do Windows em execução coletados pelo Pulse. Objetivo: identificar se algum indica keylogger, spyware, acesso remoto não autorizado, minerador, persistência maliciosa ou outro invasor. Relatório completo em: '{context_path}'. Em execução: {running}; fora das pastas do Windows: {analyzed}; com sinal forte do Pulse: {review_count}. Primeiros da fila: [{highlights}]. Regras: apenas investigue, usando somente comandos de leitura (Get-AuthenticodeSignature, Get-FileHash, Get-CimInstance Win32_Service, Get-ItemProperty, Get-NetTCPConnection); NÃO pare, desative, exclua nem altere serviços, arquivos ou registro. Não rotule nada como malware sem evidência; classifique cada serviço revisado como Legítimo, Requer investigação ou Indício forte, com a evidência. Ao final, salve o parecer completo (classificação por serviço, evidências, o que verificar manualmente e passos seguros sugeridos) em Markdown no arquivo '{review_path}', sobrescrevendo se existir, e informe o caminho.",
        analyzed = detailed.len(),
        highlights = names_summary(detailed),
    )
}

fn prepare_analysis(
    provider: String,
    custom_prompt: Option<String>,
    probe: WslProbe,
    snapshot: &ServicesSnapshot,
) -> Result<(AiServicesAnalysisPreview, CliInvocation), String> {
    let cli_cmd = cli_binary(&provider)?;
    if let Some(reason) = snapshot.unavailable_reason.as_deref() {
        return Err(format!(
            "A lista de serviços não está disponível para análise: {reason}"
        ));
    }

    let (detailed, windows) = partition_running(&snapshot.services);
    let running = detailed.len() + windows.len();
    if running == 0 {
        return Err("Nenhum serviço em execução foi coletado para análise.".into());
    }

    let report = generate_services_report(snapshot, &detailed, &windows);
    let context_file = context_file_path();
    fs::write(&context_file, report)
        .map_err(|error| format!("Não foi possível salvar o arquivo de contexto: {error}"))?;

    let detection = detect_cli(cli_cmd, probe);
    if detection.is_available || probe == WslProbe::AllowStart {
        reconcile_provider_cache(&provider, detection.is_available);
    }

    let fallback = default_prompt(
        &path_for_cli(&context_file, &detection),
        &path_for_cli(&review_file_path(), &detection),
        &detailed,
        running,
    );
    // O prompt (e o personalizado vindo da UI) segue como um único argumento do argv.
    let prompt = sanitize_prompt(
        custom_prompt
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or(&fallback),
    );
    let invocation = build_cli_invocation(cli_cmd, &prompt, &detection);

    let preview = AiServicesAnalysisPreview {
        provider_name: provider_display_name(&provider).to_string(),
        provider,
        command: invocation.display(),
        prompt,
        context_file_path: context_file.display().to_string(),
        running_count: running,
        analyzed_count: detailed.len(),
        review_count: detailed.iter().filter(|s| s.category == "review").count(),
        without_publisher_count: detailed.iter().filter(|s| s.publisher.is_none()).count(),
        highlighted: detailed.iter().take(PROMPT_HIGHLIGHTS).cloned().collect(),
        collected_at_unix_ms: snapshot.collected_at_unix_ms,
        is_available: detection.is_available,
        environment: detection.environment,
    };
    Ok((preview, invocation))
}

async fn prepare_off_thread(
    provider: String,
    custom_prompt: Option<String>,
    probe: WslProbe,
) -> Result<(AiServicesAnalysisPreview, CliInvocation), String> {
    let snapshot = snapshot_for_analysis().await;
    // `where.exe`/`wsl.exe` e a gravação do relatório bloqueiam: fora do runtime async.
    tauri::async_runtime::spawn_blocking(move || {
        prepare_analysis(provider, custom_prompt, probe, &snapshot)
    })
    .await
    .map_err(|error| format!("Falha ao preparar a análise dos serviços: {error}"))?
}

#[tauri::command]
pub async fn get_ai_services_analysis_preview(
    provider: String,
    custom_prompt: Option<String>,
) -> Result<AiServicesAnalysisPreview, String> {
    prepare_off_thread(provider, custom_prompt, WslProbe::RunningOnly)
        .await
        .map(|(preview, _)| preview)
}

#[tauri::command]
pub async fn launch_ai_services_terminal(
    provider: String,
    custom_prompt: Option<String>,
) -> Result<AiTerminalLaunchResult, String> {
    let (preview, invocation) =
        prepare_off_thread(provider.clone(), custom_prompt, WslProbe::AllowStart).await?;
    let launched = tauri::async_runtime::spawn_blocking(move || spawn_cli_terminal(&invocation))
        .await
        .map_err(|error| format!("Falha ao abrir o terminal: {error}"))??;
    Ok(launch_result(
        launched,
        provider,
        preview.command,
        preview.context_file_path,
        "Terminal aberto com o CLI e o relatório dos serviços em execução.",
    ))
}

#[tauri::command]
pub fn get_ai_services_review_status() -> AiCleanupPlanStatus {
    cleanup_plan_status_at(&review_file_path())
}

#[tauri::command]
pub fn open_ai_services_review() -> Result<(), String> {
    let path = review_file_path();
    if !path.is_file() {
        return Err("O parecer de segurança ainda não foi gerado pela IA.".into());
    }
    // Abre no app padrão de .md (o nome termina sempre em `.md`, nunca executa)
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|error| format!("Não foi possível abrir o parecer de segurança: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, path: &str, category: &'static str) -> WindowsService {
        WindowsService {
            name: name.into(),
            display_name: format!("{name} display"),
            state: "Em execução".into(),
            state_code: 4,
            start_type: "Automático".into(),
            start_type_code: 2,
            process_id: Some(42),
            executable_path: Some(path.into()),
            icon: None,
            start_name: Some("LocalSystem".into()),
            is_user_service: false,
            category,
            publisher: Some("Fabricante".into()),
            suspicion_reasons: Vec::new(),
        }
    }

    #[test]
    fn windows_services_stay_out_of_detailed_review() {
        let svchost = service(
            "Dhcp",
            r"C:\Windows\System32\svchost.exe -k LocalService",
            "system",
        );
        assert!(!needs_detailed_review(&svchost));
        let third_party = service(
            "GoogleUpdater",
            r#""C:\Program Files\Google\updater.exe" --svc"#,
            "system",
        );
        assert!(needs_detailed_review(&third_party));
    }

    #[test]
    fn stopped_services_are_not_analyzed_and_review_comes_first() {
        let mut stopped = service("Parado", r"C:\Tools\a.exe", "user");
        stopped.state = "Parado".into();
        stopped.process_id = None;
        let user = service("Usuario", r"C:\Tools\b.exe", "user");
        let mut review = service("Revisar", r"C:\Users\x\AppData\Roaming\c.exe", "review");
        review.suspicion_reasons = vec!["Executável em diretório gravável".into()];
        let windows = service("Dhcp", r"C:\Windows\System32\svchost.exe", "system");

        let (detailed, others) = partition_running(&[stopped, user, review, windows]);
        let names: Vec<_> = detailed.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Revisar", "Usuario"]);
        assert_eq!(others.len(), 1);
    }

    #[test]
    fn report_escapes_pipes_and_declares_missing_publisher() {
        let mut odd = service("Odd", r"C:\Tools\a|b.exe", "user");
        odd.publisher = None;
        let snapshot = ServicesSnapshot {
            services: vec![odd.clone()],
            running_count: 1,
            stopped_count: 0,
            paused_count: 0,
            total_count: 1,
            unavailable_reason: None,
            collected_at_unix_ms: 1,
        };
        let (detailed, windows) = partition_running(&snapshot.services);
        let md = generate_services_report(&snapshot, &detailed, &windows);
        assert!(md.contains(r"a\|b.exe"));
        assert!(md.contains("| Indisponível |"));
        assert!(md.contains("Não pare, desative, exclua nem altere"));
    }

    #[test]
    fn default_prompt_forbids_changes_and_points_to_review_file() {
        let detailed = vec![summarize(&service("X", r"C:\Tools\x.exe", "user"))];
        let prompt = default_prompt("ctx.md", "review.md", &detailed, 10);
        assert!(prompt.contains("NÃO pare, desative, exclua nem altere"));
        assert!(prompt.contains("'review.md'"));
        assert!(prompt.contains("Não rotule nada como malware sem evidência"));
    }
}
