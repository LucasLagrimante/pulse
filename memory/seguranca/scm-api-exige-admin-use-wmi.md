---
name: scm-api-exige-admin-use-wmi
description: EnumServicesStatusExW/OpenSCManagerW devolvem ACCESS_DENIED sem elevação — usar Get-CimInstance Win32_Service
module: seguranca
metadata:
  type: project
  pin: true
---

**Why:** A API SCM do Windows (`OpenSCManagerW` + `EnumServicesStatusExW`, advapi32) **exige privilégio administrativo para ENUMERAR**, não só para iniciar/parar. Medido nesta máquina com usuário comum (não administrador) (`IsAdmin: False`):

```
DIAG FAIL SERVICE_WIN32:         HRESULT(0x80070005)
DIAG FAIL SERVICE_DRIVER:        HRESULT(0x80070005)
DIAG FAIL KERNEL+FS+WIN32:       HRESULT(0x80070005)
```

`OpenSCManagerW(SC_MANAGER_CONNECT)` **passa**, e `EnumServicesStatusExW` falha em seguida com `0x80070005` (ERROR_ACCESS_DENIED) — em todas as combinações de `dwservicetype`. Não é parâmetro errado nem falta de feature; é privilégio. O `services.rs` original mostrava "Falha ao enumerar serviços do Windows (código 2147942405)" na tela para todo usuário não-elevado.

**Como aplicar:**
- O Pulse é Windows-first mas **não exige elevação**; qualquer leitura que dependa de admin transforma o módulo em erro para a maioria dos usuários.
- `Get-CimInstance Win32_Service` lê os **mesmos 313 serviços sem privilégio em ~0,8 s** (medido). Alternativa canônica para inventário.
- Para emitir dados, preferir TSV/CSV por `ForEach-Object` + `'{0}|{1}' -f` a `ConvertTo-Json`: serializar centenas de objetos para re-parsear é custo puro. `splitn(6, '|')` preserva o `PathName` (que tem aspas e espaços).
- WMI devolve `State`/`StartMode` em **inglês** (`Running`/`Stopped`/`StartPending`, `Auto`/`Manual`/`Disabled`) — traduzir em Rust, e nunca converter estado desconhecido em "Parado".
- PID `0` do WMI significa **sem processo** → `None`, nunca `Some(0)`.
- Iniciar/parar: `sc.exe start|stop <nome>` funciona sem admin para serviços de usuário, ao contrário de `StartServiceW`/`ControlService`.
- Assinatura que devolve `windows::core::Error` traz **HRESULT** (`0x80070005`), não o Win32 cru (`5`). Comparar `error.code().0 as u32` com `ERROR_MORE_DATA` (234) nunca casa — mais um bug silencioso. Decodificar com `HRESULT_FROM_WIN32` ou comparar o `HRESULT` correto.
- Cobrir com teste de integração real (`real_enumeration_reads_services_without_elevation`) — só assim uma mudança de permissão quebraria no CI, não na tela do usuário.