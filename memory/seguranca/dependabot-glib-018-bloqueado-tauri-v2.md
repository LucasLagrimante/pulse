---
name: dependabot-glib-018-bloqueado-tauri-v2
description: Alerta GHSA-wrw7-89jp-8q8g (glib 0.18.5) não pode ser atualizado via Cargo enquanto o Tauri v2 fixar gtk ^0.18 e webkit2gtk 2.0.2
module: seguranca
metadata:
  type: project
---

**Why:** `glib 0.18.5` entra em `src-tauri/Cargo.lock` apenas como dependência transitiva de `tauri`, `wry`, `tao`, `tray-icon` e `muda` (targets Linux/BSD via `gtk ^0.18` e `webkit2gtk =2.0.2`). Mesmo `tauri 2.12.7` / `wry 0.57.3` ainda prendem `gtk ^0.18`, e `glib 0.18.x` não tem backport além de `0.18.5`; no Windows/macOS `glib` não compila e o Pulse não chama `VariantStrIter`.

**How to apply:** Não tente forçar `glib >= 0.20` em `Cargo.toml` nem usar `[patch.crates-io]` esperando fechar o alerta do Dependabot (patches mantêm versão `0.18.x` no `Cargo.lock`). Só reavalie atualizar o lockfile quando `tauri`/`wry`/`tao`/`tray-icon`/`muda` migrarem para `gtk >= 0.19` (que usa `glib ^0.22`) ou GTK4; até lá, tratar o alerta #8 como dispensado (`not_used` / risco tolerável).
