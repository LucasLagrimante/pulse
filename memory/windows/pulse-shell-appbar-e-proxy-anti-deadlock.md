---
name: Pulse Shell — AppBar, TrayProxy e comandos Tauri anti-deadlock
description: Regras obrigatórias no Windows para nunca travar o Explorer nem a UI do Tauri ao ativar o Pulse Shell
module: windows
metadata:
  type: project
  pin: true
---
**Why:** Chamar `ABM_SETPOS`/`ABM_SETSTATE` na thread principal do Tauri ou antes de sinalizar `ready`/iniciar `GetMessageW` causa deadlock circular com o `explorer.exe` (`WM_SETTINGCHANGE` síncrono -> `AppHangB1`).
**How to apply:**
- Comandos Tauri que mexem no Pulse Shell (`set_beta_feature_enabled`, `update_dock_settings`, `restore_windows_taskbar`) devem ser `async fn` com `spawn_blocking`; `custom_dock::init` roda em thread própria com guarda `shell-starting.lock`.
- `AppBarSpace` (`appbar.rs`) roda em thread dedicada por borda, chama `ready.send()` **antes** de `ABM_NEW`/`ABM_SETPOS` e usa `placed_for` para evitar ping-pong entre TopBar e Dock.
- `tray_proxy.rs` usa `PostMessageW(HWND_BROADCAST)` (nunca `SendNotifyMessageW`), copia bytes de `COPYDATASTRUCT` antes de encaminhar, não encaminha `WM_ACTIVATEAPP` e chama `ReplyMessage` quando o remetente está na thread do Explorer.
- Todo erro Win32/D2D/AppBar/UpdateLayeredWindow grava com `flush()` imediato em `%LOCALAPPDATA%\Pulse\logs\shell-trace.log` (`shell_log.rs`).
