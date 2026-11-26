---
name: audio-nativo-duplicado-suprimido
description: Ícone de volume do Explorer é suprimido na Barra Superior; painel do Pulse abre o legado mmsys.cpl
module: desktop
metadata:
  type: project
---

**Why:** O SndVolSSO do Explorer aparecia na Barra Superior ao lado do widget de volume do Pulse, e ambos abriam o mesmo painel. O proxy captura ícones do Explorer sem distinção, então era preciso um predicado estreito para não ocultar rede, Segurança ou apps de terceiros.
**How to apply:** Em `tray_proxy.rs`, `TrayEntry::is_suppressed()` exige `is_explorer && is_shell_audio_tip(tip)` e é aplicado em `visible_icons`, `reorder_icon`, `click_icon_by_id` e `handle_get_rect`; o proxy continua encaminhando o `WM_COPYDATA` ao Explorer e chamando `ReplyMessage`, intactos. O comando `tray::open_legacy_audio_panel` abre `control.exe mmsys.cpl` por argv (sem `powershell -Command`) e é exposto como botão no painel de áudio do flyout.
