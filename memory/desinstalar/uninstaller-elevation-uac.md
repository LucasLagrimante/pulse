---
name: uninstaller-elevation-uac
description: Execução de desinstaladores externos que exigem elevação administrativa sem rodar o Pulse como admin
module: desinstalar
metadata:
  type: project
---
**Why:** No Windows, `std::process::Command::spawn` usa `CreateProcessW`, que falha com código 740 (`ERROR_ELEVATION_REQUIRED`) quando o desinstalador possui manifesto com `requireAdministrator` e o Pulse está rodando como usuário comum sem token elevado.
**How to apply:** Ao capturar erro 740 ou `PermissionDenied` no `spawn`, execute fallback via Win32 `ShellExecuteExW` com o verbo `"runas"` e máscara `SEE_MASK_NOCLOSEPROCESS`. Monitore o `hProcess` com `WaitForSingleObject` para obter o exit code sem obrigar o usuário a iniciar o Pulse inteiro elevado, e trate recusa de UAC (`ERROR_CANCELLED` / 1223) com mensagem explicativa.
