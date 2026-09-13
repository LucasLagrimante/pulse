---
name: subprocessos-sem-janela-windows
description: Todo Command::new no Windows deve ter .creation_flags(CREATE_NO_WINDOW) para não abrir terminais visíveis
module: desktop
metadata:
  type: project
  pin: true
---

**Why:** Sem `creation_flags(0x0800_0000)` cada `reg.exe`, `wevtutil.exe` ou `powershell.exe` abre uma janela de console visível. Com polling de 3–5s o app abria dezenas de terminais por ciclo.

**How to apply:**
- Importar `use std::os::windows::process::CommandExt;` com `#[cfg(windows)]`
- Declarar `const CREATE_NO_WINDOW: u32 = 0x0800_0000;`
- Adicionar `.creation_flags(CREATE_NO_WINDOW)` antes de `.output()` ou `.spawn()` em todo `Command::new`
- Arquivos: `startup.rs`, `startup_inventory.rs`, `system_overview.rs`, `performance.rs`, `disk_analyzer.rs`
