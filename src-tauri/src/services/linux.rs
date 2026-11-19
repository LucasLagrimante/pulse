//! Serviços no Linux: units `.service` do systemd, de sistema e do usuário.
//!
//! ## Coleta
//!
//! 1. Nomes: `systemctl list-units --all` (units carregadas, inclusive
//!    instâncias como `getty@tty1.service`) ∪ `systemctl list-unit-files`
//!    (instaladas mas nunca carregadas). Templates (`foo@.service`) não são
//!    serviços executáveis e ficam de fora.
//! 2. Detalhes: **um único** `systemctl show -p ...` com todos os nomes — uma
//!    chamada por unit custaria centenas de processos.
//! 3. O mesmo para o gerenciador do usuário (`systemctl --user`). Units de
//!    usuário recebem o prefixo `user/` no `name`: `/` não é caractere válido em
//!    nome de unit, então o prefixo nunca colide com uma unit de sistema de mesmo
//!    nome (`dbus.service` existe nos dois escopos).
//!
//! ## Mapeamento para o modelo do Windows (sem inventar valores)
//!
//! - `ActiveState`/`SubState` → `state`/`state_code` (códigos do SCM: 1 parado,
//!   2 iniciando, 3 parando, 4 em execução). `active (exited)` é ativo **sem
//!   processo** e o texto diz isso; `failed` conta como parado com texto próprio.
//! - `UnitFileState` → `start_type` em pt-BR. `disabled` no systemd ainda
//!   permite iniciar à mão, então vira "Manual" (código 3), não "Desativado";
//!   o equivalente real de "Desativado" (código 4) é `masked`.
//! - `start_name`: `User=` da unit, `root` quando vazio (é o padrão do
//!   systemd), e o usuário logado para units de usuário.
//! - `publisher`: pacote dpkg dono do executável (`dpkg-query -S`), um fato
//!   verificável — diferente do `CompanyName` auto-declarado do Windows.
//!
//! ## Controle
//!
//! Units de usuário: `systemctl --user` direto (não exige senha). Units de
//! sistema: `pkexec systemctl ...` — o polkit pede a senha do administrador
//! na hora, e cancelar o pedido devolve mensagem clara. Units essenciais
//! ([`is_critical_unit`]) não podem ser paradas, reiniciadas nem desativadas
//! pelo Pulse.

use super::command::{self, CommandOutput};
use super::WindowsService;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Prefixo das units do gerenciador do usuário no campo `name`.
pub const USER_PREFIX: &str = "user/";

const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// Inclui o tempo de o usuário digitar a senha no diálogo do polkit.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(180);

const SHOW_PROPERTIES: &str =
    "Id,Description,LoadState,ActiveState,SubState,UnitFileState,MainPID,User,DynamicUser,ExecStart";

/// Escopo do gerenciador do systemd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    System,
    User,
}

fn systemctl_path() -> String {
    command::resolve_program("systemctl", &["/usr/bin/systemctl", "/bin/systemctl"])
}

fn systemctl(scope: Scope) -> std::process::Command {
    let mut command = command::c_locale_command(&systemctl_path());
    if scope == Scope::User {
        command.arg("--user");
    }
    command.arg("--no-pager");
    command
}

/* ------------------------------------------------------------------ */
/* Parsers (puros, testados com fixtures)                              */
/* ------------------------------------------------------------------ */

/// Primeira coluna de `list-units --plain --no-legend` / `list-unit-files
/// --no-legend`, só `.service` e sem templates.
pub fn parse_unit_names(output: &str) -> Vec<String> {
    output
        .lines()
        // `list-units` sem `--plain` marca units com falha com "●".
        .filter_map(|line| {
            line.trim_start()
                .trim_start_matches('●')
                .split_whitespace()
                .next()
        })
        .filter(|name| name.ends_with(".service") && !name.ends_with("@.service"))
        .map(str::to_string)
        .collect()
}

/// Blocos `Chave=Valor` do `systemctl show`, separados por linha vazia.
pub fn parse_show_blocks(output: &str) -> Vec<HashMap<String, String>> {
    let mut blocks = Vec::new();
    let mut current: HashMap<String, String> = HashMap::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            current.insert(key.to_string(), value.to_string());
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// `ActiveState` + `SubState` → texto pt-BR e código do SCM.
pub fn describe_systemd_state(active: &str, sub: &str) -> (String, u32) {
    match (active.trim(), sub.trim()) {
        ("active", "exited") => ("Ativo (concluído, sem processo)".to_string(), 4),
        ("active", "reload") | ("reloading", _) => ("Recarregando".to_string(), 4),
        ("active", _) => ("Em execução".to_string(), 4),
        ("activating", _) => ("Iniciando".to_string(), 2),
        ("deactivating", _) => ("Parando".to_string(), 3),
        ("failed", _) => ("Falhou".to_string(), 1),
        ("inactive", _) => ("Parado".to_string(), 1),
        ("maintenance", _) | ("refreshing", _) => ("Em manutenção".to_string(), 4),
        _ => ("Estado desconhecido".to_string(), 0),
    }
}

/// `UnitFileState` → tipo de inicialização pt-BR e código do SCM.
///
/// Códigos seguem o Windows: 2 automático, 3 manual, 4 desativado.
pub fn describe_unit_file_state(state: &str) -> (String, u32) {
    match state.trim() {
        "enabled" => ("Automático".to_string(), 2),
        "enabled-runtime" => ("Automático (até reiniciar)".to_string(), 2),
        "linked" | "linked-runtime" => ("Vinculado".to_string(), 3),
        "alias" => ("Apelido de outra unit".to_string(), 3),
        "static" => ("Estático (iniciado por dependência)".to_string(), 3),
        "indirect" => ("Indireto (por outra unit)".to_string(), 3),
        "generated" => ("Gerado automaticamente".to_string(), 3),
        "transient" => ("Transitório".to_string(), 3),
        "disabled" => ("Manual".to_string(), 3),
        "masked" | "masked-runtime" => ("Mascarado (desativado)".to_string(), 4),
        "bad" => ("Arquivo da unit inválido".to_string(), u32::MAX),
        _ => ("Desconhecido".to_string(), u32::MAX),
    }
}

/// Interpretadores cujo executável "real" é o script do primeiro argumento.
const INTERPRETERS: [&str; 12] = [
    "sh", "bash", "dash", "zsh", "env", "python", "python3", "perl", "node", "ruby", "php", "java",
];

/// Executável de `ExecStart={ path=/usr/bin/x ; argv[]=/usr/bin/x -a ; ... }`.
///
/// Para `bash script.sh` / `python3 /opt/x.py` o executável informado é o
/// script: é ele que diz de onde o serviço vem (um script em `/home` rodado
/// pelo `/usr/bin/bash` seria classificado como "pasta do sistema").
pub fn parse_exec_start(value: &str) -> Option<String> {
    let path = value
        .split(';')
        .map(str::trim)
        .find_map(|part| part.trim_start_matches('{').trim().strip_prefix("path="))
        .map(str::trim)
        .filter(|path| !path.is_empty())?
        .to_string();
    let file_name = path.rsplit('/').next().unwrap_or(&path);
    if INTERPRETERS.contains(&file_name) {
        let argv = value
            .split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix("argv[]="))
            .unwrap_or("");
        if let Some(script) = argv
            .split_whitespace()
            .skip(1)
            .find(|token| token.starts_with('/'))
        {
            return Some(script.to_string());
        }
    }
    Some(path)
}

/// Converte um bloco do `systemctl show` em serviço. `None` para units que não
/// existem (`LoadState=not-found`): estão referenciadas por outra unit, mas não
/// há serviço instalado para listar.
pub fn service_from_block(
    block: &HashMap<String, String>,
    scope: Scope,
    login_user: Option<&str>,
) -> Option<WindowsService> {
    let get = |key: &str| block.get(key).map(String::as_str).unwrap_or("").trim();
    let id = get("Id");
    if id.is_empty() || !id.ends_with(".service") {
        return None;
    }
    if matches!(get("LoadState"), "not-found" | "") {
        return None;
    }
    let (state, state_code) = describe_systemd_state(get("ActiveState"), get("SubState"));
    let (start_type, start_type_code) = describe_unit_file_state(get("UnitFileState"));
    let process_id = get("MainPID")
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0 && state_code == 4);
    let description = get("Description");
    let start_name = match scope {
        Scope::User => login_user.map(str::to_string),
        Scope::System => {
            let user = get("User");
            let dynamic = get("DynamicUser") == "yes";
            Some(match (user.is_empty(), dynamic) {
                (_, true) => {
                    let name = if user.is_empty() {
                        id.trim_end_matches(".service")
                    } else {
                        user
                    };
                    format!("{name}{DYNAMIC_USER_SUFFIX}")
                }
                (true, false) => "root".to_string(),
                (false, false) => user.to_string(),
            })
        }
    };
    let name = match scope {
        Scope::System => id.to_string(),
        Scope::User => format!("{USER_PREFIX}{id}"),
    };
    Some(WindowsService {
        name,
        display_name: if description.is_empty() {
            id.to_string()
        } else {
            description.to_string()
        },
        state,
        state_code,
        start_type,
        start_type_code,
        process_id,
        executable_path: parse_exec_start(get("ExecStart")),
        icon: None,
        is_user_service: false,
        start_name,
        category: "unknown",
        publisher: None,
        suspicion_reasons: Vec::new(),
    })
}

/// Sufixo de `start_name` para usuário alocado em tempo de execução
/// (`DynamicUser=yes`): não existe em `/etc/passwd`, mas é conta de sistema.
pub const DYNAMIC_USER_SUFFIX: &str = " (DynamicUser)";

/// Saída de `dpkg-query -S` → caminho → pacote.
///
/// Linhas: `pacote: /caminho` ou `pkg1, pkg2: /caminho`. Linhas de
/// `diversion` são ignoradas.
pub fn parse_dpkg_search(output: &str) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    for line in output.lines() {
        if line.starts_with("diversion") {
            continue;
        }
        let Some((packages, path)) = line.split_once(": ") else {
            continue;
        };
        let path = path.trim();
        let package = packages.split(',').next().unwrap_or("").trim();
        if path.starts_with('/') && !package.is_empty() {
            owners.insert(path.to_string(), package.to_string());
        }
    }
    owners
}

/// Caminho equivalente pelo usrmerge (`/usr/bin/x` ↔ `/bin/x`): pacotes antigos
/// registram o caminho sem `/usr`.
fn usrmerge_alias(path: &str) -> Option<String> {
    for dir in ["bin/", "sbin/", "lib/", "lib64/"] {
        if let Some(rest) = path.strip_prefix(&format!("/usr/{dir}")) {
            return Some(format!("/{dir}{rest}"));
        }
        if let Some(rest) = path.strip_prefix(&format!("/{dir}")) {
            return Some(format!("/usr/{dir}{rest}"));
        }
    }
    None
}

/* ------------------------------------------------------------------ */
/* Units essenciais                                                     */
/* ------------------------------------------------------------------ */

/// Prefixos de units que sustentam boot, sessão gráfica, rede, áudio, login e
/// segurança. Parar qualquer uma derruba a sessão do usuário ou o acesso à
/// máquina — o Pulse recusa parar, reiniciar ou desativar (iniciar é livre).
const CRITICAL_UNIT_PREFIXES: [&str; 48] = [
    "systemd-",
    "dbus",
    "polkit",
    "udev",
    "networkmanager",
    "network-manager",
    "systemd-networkd",
    "systemd-resolved",
    "wpa_supplicant",
    "iwd",
    "display-manager",
    "sddm",
    "gdm",
    "lightdm",
    "lxdm",
    "xdm",
    "getty@",
    "serial-getty@",
    "user@",
    "user-runtime-dir@",
    "accounts-daemon",
    "upower",
    "udisks2",
    "rtkit-daemon",
    "pipewire",
    "wireplumber",
    "pulseaudio",
    "plasma-",
    "kwin",
    "gnome-session",
    "gnome-shell",
    "cinnamon-session",
    "xdg-desktop-portal",
    "xdg-document-portal",
    "xdg-permission-store",
    "at-spi",
    "ssh",
    "apparmor",
    "ufw",
    "firewalld",
    "auditd",
    "lvm2",
    "dm-event",
    "cryptsetup",
    "emergency",
    "rescue",
    "keyboard-setup",
    "console-setup",
];

/// A unit é essencial para o sistema ou para a sessão?
pub fn is_critical_unit(unit: &str) -> bool {
    let lower = unit.trim().to_lowercase();
    CRITICAL_UNIT_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// Separa `user/foo.service` em escopo + unit e valida o nome.
///
/// Só caracteres válidos de nome de unit do systemd (`[A-Za-z0-9:_.@\\-]`),
/// terminando em `.service` e sem começar por `-` (não pode virar opção do
/// `systemctl`, embora o `--` já impeça).
pub fn parse_target(name: &str) -> Result<(Scope, String), String> {
    let (scope, unit) = match name.strip_prefix(USER_PREFIX) {
        Some(rest) => (Scope::User, rest),
        None => (Scope::System, name),
    };
    let valid = !unit.is_empty()
        && unit.len() <= 256
        && unit.ends_with(".service")
        && !unit.ends_with("@.service")
        && !unit.starts_with('-')
        && unit.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '.' | '@' | '\\' | '-')
        });
    if !valid {
        return Err("Nome de serviço inválido".to_string());
    }
    Ok((scope, unit.to_string()))
}

/* ------------------------------------------------------------------ */
/* Coleta real                                                         */
/* ------------------------------------------------------------------ */

fn checked(output: CommandOutput, what: &str) -> Result<String, String> {
    if output.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "{what}: {}",
            output
                .first_error_line()
                .unwrap_or_else(|| format!("código de saída {:?}", output.code))
        ))
    }
}

fn list_names(scope: Scope) -> Result<Vec<String>, String> {
    let mut units = systemctl(scope);
    units.args([
        "list-units",
        "--type=service",
        "--all",
        "--plain",
        "--no-legend",
    ]);
    let loaded = checked(
        command::run(units, LIST_TIMEOUT)?,
        "Falha ao listar as units do systemd",
    )?;
    let mut files = systemctl(scope);
    files.args(["list-unit-files", "--type=service", "--no-legend"]);
    let installed = checked(
        command::run(files, LIST_TIMEOUT)?,
        "Falha ao listar os arquivos de unit do systemd",
    )?;

    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for name in parse_unit_names(&loaded)
        .into_iter()
        .chain(parse_unit_names(&installed))
    {
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    Ok(names)
}

fn collect_scope(scope: Scope, login_user: Option<&str>) -> Result<Vec<WindowsService>, String> {
    let names = list_names(scope)?;
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut show = systemctl(scope);
    show.args(["show", "-p", SHOW_PROPERTIES, "--"])
        .args(&names);
    let output = command::run(show, LIST_TIMEOUT)?;
    // `show` devolve código 0 mesmo com units inexistentes (LoadState=not-found);
    // código diferente de zero sem saída é falha real.
    if !output.success() && output.stdout.trim().is_empty() {
        return checked(output, "Falha ao ler os detalhes das units").map(|_| Vec::new());
    }
    let mut seen = HashSet::new();
    Ok(parse_show_blocks(&output.stdout)
        .iter()
        .filter_map(|block| service_from_block(block, scope, login_user))
        // Apelidos (`sshd.service` → `ssh.service`) resolvem para o mesmo Id.
        .filter(|service| seen.insert(service.name.clone()))
        .collect())
}

fn login_user() -> Option<String> {
    std::env::var("USER")
        .ok()
        .filter(|user| !user.trim().is_empty())
        .or_else(|| {
            let output = command::run(command::c_locale_command("id"), LIST_TIMEOUT).ok()?;
            output
                .success()
                .then(|| output.stdout.trim().to_string())
                .filter(|user| !user.is_empty())
        })
}

/// Preenche `publisher` com o pacote dpkg dono do executável. Sem `dpkg-query`
/// (distro sem dpkg) o campo fica `None` — "não sei", não "sem fabricante".
fn fill_package_owners(services: &mut [WindowsService]) {
    if !command::program_exists("dpkg-query") {
        return;
    }
    let mut paths: Vec<String> = Vec::new();
    for service in services.iter() {
        if let Some(path) = service.executable_path.as_deref() {
            if path.starts_with('/') && !paths.iter().any(|known| known == path) {
                paths.push(path.to_string());
                if let Some(alias) = usrmerge_alias(path) {
                    paths.push(alias);
                }
            }
        }
    }
    if paths.is_empty() {
        return;
    }
    let mut query = command::c_locale_command("dpkg-query");
    query.arg("-S").args(&paths);
    // Código 1 = algum caminho sem dono, o que é esperado (binários em /opt).
    let Ok(output) = command::run(query, LIST_TIMEOUT) else {
        return;
    };
    let owners = parse_dpkg_search(&output.stdout);
    for service in services.iter_mut() {
        let Some(path) = service.executable_path.as_deref() else {
            continue;
        };
        let owner = owners
            .get(path)
            .or_else(|| usrmerge_alias(path).and_then(|alias| owners.get(&alias)));
        if let Some(package) = owner {
            service.publisher = Some(format!("Pacote {package} (dpkg)"));
        }
    }
}

pub fn enumerate() -> Result<Vec<WindowsService>, String> {
    let started = std::time::Instant::now();
    let user = login_user();
    let mut services = collect_scope(Scope::System, user.as_deref())?;
    match collect_scope(Scope::User, user.as_deref()) {
        Ok(user_services) => services.extend(user_services),
        // Sem sessão D-Bus do usuário (ex.: rodando fora da sessão gráfica) as
        // units de sistema continuam válidas; a falha fica registrada.
        Err(error) => crate::debug::record_failure(
            "services_list_user_units",
            started.elapsed().as_millis(),
            crate::debug::Failure::new("Units de usuário do systemd indisponíveis")
                .stage("systemctl --user")
                .detail(error),
        ),
    }
    if services.is_empty() {
        return Err("O systemd não retornou nenhum serviço".to_string());
    }
    fill_package_owners(&mut services);
    Ok(services)
}

/// Inicia, para, reinicia, ativa ou desativa uma unit.
pub fn control(name: &str, action: &str) -> Result<(), String> {
    let verb = match action {
        "start" | "stop" | "restart" | "enable" | "disable" => action,
        other => return Err(format!("Ação de serviço desconhecida: {other}")),
    };
    let (scope, unit) = parse_target(name)?;
    if matches!(verb, "stop" | "restart" | "disable") && is_critical_unit(&unit) {
        return Err(format!(
            "{unit} é essencial para o sistema ou para a sessão; o Pulse não o para, reinicia nem desativa"
        ));
    }

    let output = match scope {
        Scope::User => {
            let mut command = systemctl(Scope::User);
            command.args([verb, "--", unit.as_str()]);
            command::run(command, CONTROL_TIMEOUT)?
        }
        Scope::System => {
            let pkexec = command::resolve_program("pkexec", &["/usr/bin/pkexec", "/bin/pkexec"]);
            if !std::path::Path::new(&pkexec).is_file() && !command::program_exists("pkexec") {
                return Err(
                    "Controlar serviços do sistema exige o pkexec (polkit), que não foi encontrado"
                        .to_string(),
                );
            }
            let mut command = command::c_locale_command(&pkexec);
            command.args([systemctl_path().as_str(), verb, "--", unit.as_str()]);
            command::run(command, CONTROL_TIMEOUT)?
        }
    };

    if output.success() {
        return Ok(());
    }
    if scope == Scope::System {
        if let Some(message) = command::describe_pkexec_exit(output.code) {
            return Err(message.to_string());
        }
    }
    let detail = output
        .first_error_line()
        .unwrap_or_else(|| format!("código de saída {:?}", output.code));
    Err(format!("O systemd recusou a operação: {detail}"))
}

/* ------------------------------------------------------------------ */
/* Contas de sistema                                                    */
/* ------------------------------------------------------------------ */

/// Contas com UID < 1000 (faixa de sistema do Debian/Ubuntu/Fedora) e `nobody`,
/// lidas uma vez de `/etc/passwd`.
fn system_users() -> &'static HashSet<String> {
    static USERS: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();
    USERS.get_or_init(|| {
        std::fs::read_to_string("/etc/passwd")
            .map(|text| parse_system_users(&text))
            .unwrap_or_default()
    })
}

pub fn parse_system_users(passwd: &str) -> HashSet<String> {
    passwd
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?.trim();
            let uid = fields.nth(1)?.trim().parse::<u32>().ok()?;
            (uid < 1000 || uid == 65534).then(|| name.to_lowercase())
        })
        .collect()
}

/// `root`, usuário dinâmico do systemd ou conta de sistema do `/etc/passwd`.
pub fn is_system_user(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    lower == "root"
        || lower.ends_with(&DYNAMIC_USER_SUFFIX.to_lowercase())
        || system_users().contains(&lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW_FIXTURE: &str = "MainPID=940
ExecStart={ path=/usr/sbin/cron ; argv[]=/usr/sbin/cron -f -P $EXTRA_OPTS ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }
User=
DynamicUser=no
Id=cron.service
Description=Regular background program processing daemon
LoadState=loaded
ActiveState=active
SubState=running
UnitFileState=enabled

MainPID=0
ExecStart=
User=
DynamicUser=no
Id=nonexist.service
Description=nonexist.service
LoadState=not-found
ActiveState=inactive
SubState=dead
UnitFileState=

MainPID=0
ExecStart={ path=/usr/bin/bash ; argv[]=/usr/bin/bash /home/dev/bin/vigia.sh --loop ; ignore_errors=no }
User=
DynamicUser=no
Id=vigia.service
Description=Vigia
LoadState=loaded
ActiveState=failed
SubState=failed
UnitFileState=disabled

MainPID=0
ExecStart={ path=/usr/lib/systemd/systemd-timesyncd ; argv[]=/usr/lib/systemd/systemd-timesyncd ; ignore_errors=no }
User=
DynamicUser=yes
Id=systemd-timesyncd.service
Description=Network Time Synchronization
LoadState=loaded
ActiveState=active
SubState=exited
UnitFileState=masked
";

    #[test]
    fn parses_show_blocks_and_skips_missing_units() {
        let blocks = parse_show_blocks(SHOW_FIXTURE);
        assert_eq!(blocks.len(), 4);
        let services: Vec<_> = blocks
            .iter()
            .filter_map(|block| service_from_block(block, Scope::System, Some("dev")))
            .collect();
        assert_eq!(services.len(), 3, "not-found não é serviço instalado");

        let cron = &services[0];
        assert_eq!(cron.name, "cron.service");
        assert_eq!(cron.state, "Em execução");
        assert_eq!(cron.state_code, 4);
        assert_eq!(cron.start_type, "Automático");
        assert_eq!(cron.process_id, Some(940));
        assert_eq!(cron.executable_path.as_deref(), Some("/usr/sbin/cron"));
        assert_eq!(cron.start_name.as_deref(), Some("root"));

        let vigia = &services[1];
        assert_eq!(vigia.state, "Falhou");
        assert_eq!(vigia.state_code, 1);
        assert_eq!(vigia.start_type, "Manual");
        assert_eq!(vigia.process_id, None);
        // O script, não o bash, é o executável que importa.
        assert_eq!(
            vigia.executable_path.as_deref(),
            Some("/home/dev/bin/vigia.sh")
        );

        let timesync = &services[2];
        assert_eq!(timesync.state, "Ativo (concluído, sem processo)");
        assert_eq!(timesync.start_type_code, 4);
        assert_eq!(
            timesync.start_name.as_deref(),
            Some("systemd-timesyncd (DynamicUser)")
        );
        assert!(is_system_user(timesync.start_name.as_deref().unwrap()));
    }

    #[test]
    fn user_units_get_prefix_and_login_user() {
        let blocks = parse_show_blocks(SHOW_FIXTURE);
        let service = service_from_block(&blocks[0], Scope::User, Some("dev")).unwrap();
        assert_eq!(service.name, "user/cron.service");
        assert_eq!(service.start_name.as_deref(), Some("dev"));
    }

    #[test]
    fn parses_unit_names_without_templates() {
        let text = "accounts-daemon.service   loaded active running Accounts Service\n\
                    getty@.service   enabled  enabled\n\
                    getty@tty1.service loaded active running Getty\n\
                    ● bad.service loaded failed failed Bad\n\
                    foo.timer enabled enabled\n";
        assert_eq!(
            parse_unit_names(text),
            vec![
                "accounts-daemon.service",
                "getty@tty1.service",
                "bad.service"
            ]
        );
    }

    #[test]
    fn maps_states_without_inventing() {
        assert_eq!(describe_systemd_state("active", "running").1, 4);
        assert_eq!(describe_systemd_state("activating", "start").1, 2);
        assert_eq!(describe_systemd_state("deactivating", "stop").1, 3);
        assert_eq!(describe_systemd_state("inactive", "dead").1, 1);
        assert_eq!(describe_systemd_state("weird", "x").1, 0);
        assert_eq!(describe_unit_file_state("disabled").0, "Manual");
        assert_eq!(describe_unit_file_state("masked").1, 4);
        assert_eq!(describe_unit_file_state("").1, u32::MAX);
    }

    #[test]
    fn parses_dpkg_search_output() {
        let text = "cron: /usr/sbin/cron\n\
                    libc-bin, libc6: /usr/bin/ldd\n\
                    diversion by foo from: /usr/bin/bar\n\
                    dpkg-query: no path found matching pattern /opt/x\n";
        let owners = parse_dpkg_search(text);
        assert_eq!(
            owners.get("/usr/sbin/cron").map(String::as_str),
            Some("cron")
        );
        assert_eq!(
            owners.get("/usr/bin/ldd").map(String::as_str),
            Some("libc-bin")
        );
        assert_eq!(owners.len(), 2);
        assert_eq!(
            usrmerge_alias("/usr/sbin/cron").as_deref(),
            Some("/sbin/cron")
        );
        assert_eq!(usrmerge_alias("/lib/x").as_deref(), Some("/usr/lib/x"));
        assert_eq!(usrmerge_alias("/opt/x"), None);
    }

    #[test]
    fn validates_targets_and_protects_critical_units() {
        assert_eq!(
            parse_target("user/pipewire.service").unwrap(),
            (Scope::User, "pipewire.service".to_string())
        );
        assert_eq!(parse_target("getty@tty1.service").unwrap().0, Scope::System);
        assert!(parse_target("foo.service; rm -rf /").is_err());
        assert!(parse_target("-H.service").is_err());
        assert!(parse_target("foo.timer").is_err());
        assert!(parse_target("getty@.service").is_err());
        assert!(is_critical_unit("NetworkManager.service"));
        assert!(is_critical_unit("systemd-journald.service"));
        assert!(is_critical_unit("sddm.service"));
        assert!(!is_critical_unit("cron.service"));
        assert!(!is_critical_unit("anydesk.service"));
        let error = control("dbus.service", "stop").unwrap_err();
        assert!(error.contains("essencial"));
        assert!(control("cron.service", "rm").is_err());
    }

    #[test]
    fn system_users_come_from_passwd() {
        let users = parse_system_users(
            "root:x:0:0:root:/root:/bin/bash\nmessagebus:x:101:101::/:/usr/sbin/nologin\n\
             dev:x:1000:1000::/home/dev:/bin/bash\nnobody:x:65534:65534::/:/usr/sbin/nologin\n",
        );
        assert!(users.contains("messagebus"));
        assert!(users.contains("nobody"));
        assert!(!users.contains("dev"));
    }

    /// Coleta real nesta máquina: forma e sanidade, não valores exatos.
    #[test]
    fn enumerates_real_units() {
        if !std::path::Path::new("/run/systemd/system").exists() {
            return; // Sem systemd (contêiner): nada a verificar.
        }
        let services = enumerate().expect("systemd deve listar serviços");
        assert!(services.len() > 10);
        assert!(services.iter().any(|s| s.state_code == 4));
        assert!(services.iter().all(|s| !s.name.ends_with("@.service")));
        let names: HashSet<_> = services.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names.len(), services.len(), "nomes devem ser únicos");
        assert!(services
            .iter()
            .filter(|s| s.state_code == 4 && s.state == "Em execução")
            .any(|s| s.process_id.is_some()));

        // Classificação completa: units de sistema do systemd devem cair em
        // "system" (root/contas de sistema + /usr), nunca em massa na revisão.
        let snapshot = crate::services::build_snapshot(services);
        assert!(snapshot.unavailable_reason.is_none());
        let count = |category: &str| {
            snapshot
                .services
                .iter()
                .filter(|s| s.category == category)
                .count()
        };
        eprintln!(
            "total={} system={} user={} review={} unknown={} com pacote={}",
            snapshot.total_count,
            count("system"),
            count("user"),
            count("review"),
            count("unknown"),
            snapshot
                .services
                .iter()
                .filter(|s| s.publisher.is_some())
                .count()
        );
        for service in snapshot.services.iter().filter(|s| s.category == "review") {
            eprintln!(
                "revisão: {} {:?} {:?}",
                service.name, service.executable_path, service.suspicion_reasons
            );
        }
        let journald = snapshot
            .services
            .iter()
            .find(|s| s.name == "systemd-journald.service")
            .expect("journald existe em todo systemd");
        assert_eq!(journald.category, "system");
        assert!(count("system") > count("review") * 5);
    }
}
