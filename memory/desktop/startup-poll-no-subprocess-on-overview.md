---
name: startup-poll-no-subprocess-on-overview
description: Nunca chamar list_startup_items() dentro do ciclo de polling de 3s do system_overview
module: desktop
metadata:
  type: project
  pin: true
---

**Why:** `list_startup_items()` dispara 4+ subprocessos (`reg.exe`, `wevtutil.exe`). Chamado a cada 3s dentro de `get_system_overview_sync()` travava a janela e causava jank ao arrastar.

**How to apply:**
- Manter `startup_item_count` fora do ciclo de overview (retornar 0 ou valor do cache)
- `list_startup_items` tem cache TTL de 10s próprio — é invalidado pelo `set_startup_item_enabled`
- Nunca colocar operações de I/O ou subprocessos pesados dentro de `get_system_overview_sync`
