---
name: indicador-permissao-administrador
description: Pílula PERMISSÃO na command-bar mostra se o Pulse está elevado, lista o que exige admin e reinicia via UAC; nova funcionalidade que dependa de admin deve entrar na lista
module: plataforma
metadata:
  type: project
---
**Why:** O usuário quis saber, dentro do app, se está como administrador e quais funcionalidades deixam de funcionar sem isso.
**How to apply:**
- Backend `privileges.rs`: `get_privilege_status` (token efetivo via `uninstall::platform::is_elevated`, cacheado em OnceLock) e `relaunch_as_admin` (ShellExecuteExW `runas` + `--elevated-relaunch`, depois `app.exit(0)`).
- O processo elevado usa `InstanceGuard::acquire_after_relaunch` (espera até 15 s pelo mutex) para não morrer ao achar a instância antiga ainda fechando.
- Funcionalidade nova que exija admin: acrescentar em `admin_only_features()`; categoria de limpeza em pasta do sistema: `requires_admin()` em `disk_cleanup.rs` (selo EXIGE ADMINISTRADOR via `useIsStandardUser`).
- Inicialização NÃO entra na lista: entradas HKLM são desativadas por override em HKCU.
