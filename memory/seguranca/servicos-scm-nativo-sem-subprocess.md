---
name: servicos-scm-nativo-sem-subprocess
description: Inventário de serviços do Windows deve usar a API SCM (advapi32), nunca Get-Service/PowerShell
module: seguranca
metadata:
  type: project
  pin: true
---

**Why:** A lista de serviços no Windows passa de 300 entradas. `Get-Service | ConvertTo-Json` via PowerShell custa 1–3 s de cold start do `powershell.exe` e uma janela de console se o `CREATE_NO_WINDOW` for esquecido — o mesmo problema que motivou `memory/desktop/powershell-stdin-freeze.md` e `no-window-subprocesses.md`. A API SCM entrega tudo em uma chamada, sem subprocess.

**How to apply:**
- `OpenSCManagerW(SC_MANAGER_CONNECT)` → `EnumServicesStatusExW(SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL)` devolve nome, nome de exibição, estado e PID de todos os serviços de uma vez. Filtrar por `SERVICE_WIN32` já exclui drivers de kernel e de filesystem (não são serviços de usuário).
- `QueryServiceConfigW` por serviço dá `dwStartType` (Automático/Manual/Desativado/Boot/Sistema) e `lpBinaryPathName`. Abrir/fechar com `OpenServiceW` + `CloseServiceHandle`; o `PWSTR` aponta para dentro do buffer do Windows — converter com `String::from_utf16_lossy` sobre o slice, nunca `LocalFree`.
- `EnumServicesStatusExW` retorna `ERROR_MORE_DATA` (234) quando o buffer não cabe: crescer o buffer e repetir o mesmo índice (o handle de resume continua válido).
- Estado `SERVICE_START_PENDING`/`STOP_PENDING` ≠ "Em execução": manter códigos distintos na UI, senão serviço ainda subindo aparece como ativo.
- Cache com TTL próprio (~10 s) e `spawn_blocking` no comando; nunca chamar de dentro do polling de `system_overview` (ver `memory/desktop/startup-poll-no-subprocess-on-overview.md`).
- Iniciar/parar exige privilégio: erro 5 do SCM deve virar "Execute o Pulse como administrador", nunca erro genérico engolido.
- Precisa da feature `"Win32_System_Services"` no crate `windows` do `src-tauri/Cargo.toml`.