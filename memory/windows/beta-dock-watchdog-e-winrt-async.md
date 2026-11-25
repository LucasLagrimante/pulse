---
name: Pulse Shell AppBar, Proxy Tray, Watchdog e WinRT async
description: Regras do Pulse Shell (ocultação automática da taskbar, SHAppBarMessage direto ao explorer.exe com proxy Shell_TrayWnd, alpha per-pixel, watchdog e WinRT async)
module: windows
metadata:
  type: project
---

**Why:** Ativar o Pulse Shell deve ocultar automaticamente a taskbar nativa (sem toggle separado), reservar WorkArea no topo e na base (`SHAppBarMessage`) para janelas/overlays não ficarem por baixo, renderizar alpha per-pixel sem retângulo opaco sobre a dock e restaurar tudo em crash (`panic = "abort"`).
**How to apply:** Encaminhe `SHAppBarMessage` direto ao `Shell_TrayWnd` real do `explorer.exe` via `SHAllocShared` (nunca deixe o proxy `Shell_TrayWnd` interceptar `ABM_SETPOS`), use `UpdateLayeredWindow` + `ID2D1DCRenderTarget` DIB32 pré-multiplicada com DPI real, e lance `--dock-watchdog <pid>` gravando `shell-recovery.json` para restaurar taskbar + `SPI_SETWORKAREA`. Em WinRT BLE (`windows 0.61.3`), aguarde `IAsyncOperation` com `Status()` + deadline + `Cancel()`.
