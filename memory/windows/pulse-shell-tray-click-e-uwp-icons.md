---
name: pulse-shell-tray-click-e-uwp-icons
description: Correção de cliques em ícones do System Tray (callback == 0 / v3) e ícones de apps UWP/MSIX na Dock
module: windows
metadata:
  type: project
---
**Why:** Ícones do tray que chegam via `NIM_MODIFY` sem `NIF_MESSAGE` ficam com `callback == 0` e não respondem a cliques; apps v3 (ex.: Flameshot/Qt) exigem `NIN_SELECT`/`WM_CONTEXTMENU`. Na Dock, `SHDefExtractIconW` em `.exe` dentro de `WindowsApps` (`WhatsApp.Root.exe`) retorna o ícone genérico de janela branca.
**How to apply:** Em `tray_proxy.rs`, disparar `ask_missing_callbacks()` (`TaskbarCreated` direcionado ao HWND não-Explorer com `callback == 0`), despachar `NIN_SELECT`/`WM_CONTEXTMENU` para `version >= 3` e identificar ícones por `stable_id` persistido por `GUID`/`exe+uid`. Em `icons.rs`, resolver `store_app_id(src)` via `IShellItemImageFactory` antes de `SHDefExtractIconW` para caminhos em `\WindowsApps\`.
