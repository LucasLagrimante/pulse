---
name: no-powershell-string-interpolation
description: Nunca interpolar nomes de arquivos/pastas, prompts ou IDs vindos do IPC em string de `powershell -Command`; usar argv separado
module: seguranca
metadata:
  type: project
  pin: true
---

**Why:** Na auditoria open source (2026-11-04), `ai_disk_advisor.rs` montava `powershell.exe -Command "<cli> \"<prompt>\""` com nomes de pastas/arquivos do scan. `\"` não escapa nada no PowerShell, e `$(...)` dentro de aspas duplas é executado: um arquivo chamado `$(Start-Process calc).zip` rodaria código ao abrir o terminal. Além disso, `provider` desconhecido virava o nome do executável.

**How to apply:**
- Passar o executável e cada argumento como argv (`Command::new(exe).args([...])`), sem shell intermediário; para abrir terminal, `wt.exe new-tab <exe> <args...>`.
- Se precisar de PowerShell, usar `-EncodedCommand` com script fixo e dados via variável de ambiente/arquivo, nunca concatenados.
- Validar enums do IPC (`provider`, `action`) com allowlist e rejeitar o resto (`other => Err`).
