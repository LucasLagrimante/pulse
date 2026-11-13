---
name: debug-local
description: Investiga sozinho um problema do Pulse instalado lendo a trilha de auditoria gravada em %LOCALAPPDATA%\Pulse\logs, agrupando falhas por operação e etapa, reproduzindo a coleta no Windows e confirmando a correção com teste. Acionar em "telemetria indisponível", "o Pulse não mostra", "erro no app", "investiga os logs", "debug local", "por que falhou no Pulse", qualquer export da auditoria colado na conversa ou /debug-local.
---

# Debug local do Pulse instalado

O Pulse grava cada operação auditada em `%LOCALAPPDATA%\Pulse\logs\debug-AAAA-MM-DD.jsonl`
**somente com o modo Debug ligado** (retenção de 7 dias, teto de 10 MB por dia). Cada linha tem
`operation`, `outcome`, `durationMs`, `message` e, nas falhas, `stage`, `detail` (erro técnico
original) e `context` (trecho do dado ao redor do ponto de falha), além de `appVersion` e
`sessionStartedUnixMs`. Do WSL a pasta é lida direto em `/mnt/c/...`.

## 1. Ler os logs (antes de abrir código)

```bash
node scripts/debug-logs.mjs --since 24h            # resumo: operações, taxa de erro, p50/p95, grupos de erro
node scripts/debug-logs.mjs --since 7d --errors    # só falhas da semana
node scripts/debug-logs.mjs --op system_overview   # uma operação
node scripts/debug-logs.mjs --json                 # para processar
```

- Saída 2 com "pasta inexistente": modo Debug **desligado** ou versão < 1.6.0. Peça ao usuário para
  ligar o Debug no app e reproduzir; não invente diagnóstico sem dado.
- Export colado pelo usuário (JSON da tela Debug) tem o mesmo formato de evento: analise igual.

## 2. Ler o padrão antes da causa

| Sinal | Leitura |
|---|---|
| uma operação com 100% de erro e as outras ok | causa fixa nos dados ou no ambiente daquela coleta |
| erro intermitente | concorrência, timeout ou recurso externo |
| `durationMs` alto e igual em toda falha | o comando rodou até o fim; o problema está na saída (parse) |
| `durationMs` ~0 | falhou ao iniciar (`spawn`) ou caiu antes de qualquer trabalho |
| `message` com "Nª seguida" crescendo | sem nenhuma coleta boa: a tela fica sem dado (cache nunca preenchido) |
| erro só a partir de uma `appVersion` | regressão: `git log v<anterior>..v<atual> -- <arquivo>` |

Etapas (`stage`) do `system_overview`: `spawn`, `exit_status` (context = stderr do PowerShell), `utf8`,
`clock`, `parse` (detail = erro do serde com linha/coluna, context = trecho do JSON com caracteres de
controle escapados como `\u{0007}`). Comandos via `record_result` usam `stage=command` com o erro real em `detail`.

## 3. Reproduzir no Windows a partir do WSL

Coleta PowerShell embutida no Rust (ex.: `OVERVIEW_SCRIPT` em `src-tauri/src/system_overview.rs`):

```bash
awk '/^const OVERVIEW_SCRIPT: &str = r#"/{f=1;next} /^"#;/{f=0} f' src-tauri/src/system_overview.rs > /mnt/c/Users/Public/pulse-repro.ps1
powershell.exe -NoProfile -NonInteractive -File 'C:\Users\Public\pulse-repro.ps1' > "$SCRATCH/out.json"; echo "exit=$?"
node -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$SCRATCH/out.json"
rm /mnt/c/Users/Public/pulse-repro.ps1
```

`$SCRATCH` = diretório temporário da sessão. Apague o `.ps1` público no fim.
Atenção: o PowerShell do app roda com os privilégios do Pulse (pode estar elevado); diferenças de
resultado entre sua execução e a do app costumam vir daí.

## 4. Corrigir com prova

1. Teste Rust que reproduz o dado real que quebrou (ex.: `overview_tolerates_raw_control_characters_in_metadata`).
2. `bash scripts/rust-check.sh` (fmt dos `.rs` alterados + clippy + cargo test no Windows).
3. Frontend tocado: `npm run verify`.
4. Se a falha não deixou rastro útil no log, **melhore o log na mesma tarefa**: use
   `crate::debug::record_failure(op, ms, Failure::new(msg).stage(..).detail(..).context(..))` ou
   `record_result(op, ms, &result, msg)`. Nunca descarte o erro real com `map_err(|_| "Falha")`.
5. Registre a lição em `memory/<modulo>/<slug>.md` e reindexe.

## 5. Entregar

Causa (com o evento do log que a comprova), correção, testes rodados e se precisa de release
(skill `lancar-versao`). Sem release, o app instalado continua com o bug.
