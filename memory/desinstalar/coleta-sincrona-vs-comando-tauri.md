---
name: coleta-sincrona-vs-comando-tauri
description: Não chamar comandos Tauri async internamente no backend Rust quando houver função síncrona
module: desinstalar
metadata:
  type: project
---
**Why:**
Comandos Tauri anotados com `#[tauri::command]` são assíncronos (`impl Future`), recebem argumentos como `force_refresh: Option<bool>` e são pensados para IPC do frontend. Chamar `list_installed_apps()` dentro de funções síncronas no Rust gera erro `E0061` (falta de argumento) e `E0609` (campo não existe em `impl Future`).

**How to apply:**
Para consumo interno síncrono no backend (como em `enrich_updates_with_icons`), use diretamente `crate::uninstall::installed::collect_cached()` ou `collect()`, reservando `list_installed_apps` exclusivamente para os bindings de invocação IPC do Tauri.
