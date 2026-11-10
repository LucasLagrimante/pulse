---
name: identidade-processos-protegidos
description: Caminho/nome/ícone de processos elevados ou protegidos vêm de QueryFullProcessImageNameW no Rust, não de Get-Process .Path
module: atividade
metadata:
  type: project
---
`Get-Process .Path` exige acesso total (MainModule) e devolve vazio para processos elevados, de outra sessão ou PPL (MsMpEng, PowerToys elevado) — a linha ficava sem ícone e com o ProcessName cru.
**Why:** o Gerenciador de Tarefas resolve tudo com `PROCESS_QUERY_LIMITED_INFORMATION`; pseudo-processos (Memory Compression, vmmemWSL, System) nem têm executável.
**How to apply:** `process_identity::enrich` completa path + FileDescription/CompanyName (GetFileVersionInfoW) após o PowerShell; pseudo-processos usam o catálogo `known_process` → `systemKind` e glifo lucide em `ProcessFallbackIcon` (Identity.tsx). Nunca usar ícone de outro app como se fosse do processo (exceção honesta: `wsl.exe` para vmmemWSL).
