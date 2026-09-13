---
name: powershell-stdin-freeze
description: powershell.exe chamado pelo WSL sem < /dev/null trava indefinidamente esperando stdin
module: desktop
metadata:
  type: project
---

**Why:** No WSL, `powershell.exe` (Windows) lê stdin herdado do bash, que fica bloqueado esperando EOF. O processo nunca finaliza.

**How to apply:** Sempre chamar com stdin fechado: `powershell.exe ... < /dev/null` ao invocar do WSL/bash.
