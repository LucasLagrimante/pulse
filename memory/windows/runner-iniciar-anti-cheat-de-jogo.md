---
name: runner-iniciar-anti-cheat-de-jogo
description: Com jogo protegido por anti-cheat aberto, o runner na tecla Windows é pausado (hook desinstalado) via launcher/game_guard.rs — não competir com o anti-cheat
module: windows
metadata:
  type: project
---

**Why:** Em 2026-11-25, com o hook já engolindo todo toque, o anti-cheat do Marvel Rivals (`Marvel-Win64-Shipping`) roubava o foco ~0,4s após o runner abrir, a Pesquisa (`SearchHost`) surgia sem tecla nenhuma, e às vezes só a Dock aparecia. O usuário decidiu: não burlar o anti-cheat; desativar o runner e deixar o teclado sem nenhum hook para o jogo enxergar.
**How to apply:** `launcher::apply_win_key_hook` só guarda a preferência; `reconcile_win_key_hook` instala o hook se preferência ligada e `game_guard::active()` é `None`. `game_guard` faz snapshot Toolhelp a cada 3s contra `GUARDED_PROCESSES` (jogo e anti-cheats). Jogo novo com anti-cheat só de driver → adicionar o `.exe` do jogo na lista. Para diagnosticar, cruzar `win-key-trace.log` com um logger LL somente leitura + `EVENT_SYSTEM_FOREGROUND`. Ver [[win-key-suppress-start-menu]].
