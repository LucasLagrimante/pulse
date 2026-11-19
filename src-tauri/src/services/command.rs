//! Execução de comandos do sistema (Linux/macOS) com tempo limite.
//!
//! As saídas de `systemctl show`, `apt list` e `flatpak` passam facilmente de
//! 64 KiB — o tamanho do buffer de pipe do kernel. Esperar o processo terminar
//! **antes** de ler a saída trava os dois lados (o filho bloqueia escrevendo, o
//! pai bloqueia esperando). Por isso stdout e stderr são drenados em threads
//! próprias enquanto o processo é acompanhado com `try_wait`.
//!
//! Os argumentos vão sempre separados (`Command::args`), nunca como string de
//! shell, e `LC_ALL=C` garante saída em inglês estável para os parsers.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Saída de um comando concluído dentro do tempo limite.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Código de saída; `None` quando o processo foi encerrado por sinal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Primeira linha não vazia do stderr (ou do stdout), para mensagens de erro.
    pub fn first_error_line(&self) -> Option<String> {
        self.stderr
            .lines()
            .chain(self.stdout.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    }
}

/// Monta um `Command` com `LC_ALL=C` (saída parseável) e sem stdin.
pub fn c_locale_command(program: &str) -> Command {
    let mut command = crate::process::hidden_command(program);
    command
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null());
    command
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    })
}

/// Executa o comando, drenando a saída, e o encerra se passar de `timeout`.
///
/// `Err` só para falha ao iniciar ou estouro do tempo: código de saída diferente
/// de zero é resultado legítimo e quem chama decide o que significa.
pub fn run(mut command: Command, timeout: Duration) -> Result<CommandOutput, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Não foi possível executar o {program}: {error}"))?;

    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "O {program} não respondeu em {} s e foi encerrado",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(error) => return Err(format!("Falha ao acompanhar o {program}: {error}")),
        }
    };

    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    Ok(CommandOutput {
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Primeiro caminho existente entre os candidatos absolutos; senão o nome puro
/// (resolvido pelo `PATH`). `pkexec` exige caminho que exista, então preferimos
/// o absoluto.
pub fn resolve_program(name: &str, candidates: &[&str]) -> String {
    candidates
        .iter()
        .find(|path| std::path::Path::new(path).is_file())
        .map(|path| (*path).to_string())
        .unwrap_or_else(|| name.to_string())
}

/// O programa existe em algum diretório do `PATH`?
pub fn program_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Mensagem de `pkexec` para os códigos de saída que ele próprio produz.
///
/// 126: o usuário fechou o diálogo de senha ou não tem autorização;
/// 127: falha de autenticação (ou o programa não existe).
pub fn describe_pkexec_exit(code: Option<i32>) -> Option<&'static str> {
    match code {
        Some(126) => Some("A autorização foi cancelada ou negada no pedido de senha do sistema"),
        Some(127) => {
            Some("A autenticação de administrador falhou (senha incorreta ou polkit indisponível)")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_large_output_without_deadlock() {
        // 200 KB > buffer de pipe: sem drenagem paralela isto travaria.
        let mut command = c_locale_command("sh");
        command.args(["-c", "head -c 200000 /dev/zero | tr '\\0' 'a'"]);
        let output = run(command, Duration::from_secs(10)).expect("sh deve rodar");
        assert!(output.success());
        assert_eq!(output.stdout.len(), 200_000);
    }

    #[test]
    fn kills_on_timeout() {
        let mut command = c_locale_command("sleep");
        command.arg("5");
        let started = Instant::now();
        let error = run(command, Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("não respondeu"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn pkexec_codes_are_translated() {
        assert!(describe_pkexec_exit(Some(126)).is_some());
        assert!(describe_pkexec_exit(Some(127)).is_some());
        assert!(describe_pkexec_exit(Some(1)).is_none());
    }
}
