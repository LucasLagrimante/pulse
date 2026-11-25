---
name: Supressão da tecla Windows no WH_KEYBOARD_LL
description: Por que o Menu Iniciar abre mesmo injetando 0xE8 se o WM_KEYUP físico não for consumido
module: windows
metadata:
  type: project
---

**Why:** Chamar `SendInput([0xE8 DOWN, 0xE8 UP])` dentro de `WH_KEYBOARD_LL` e depois chamar `CallNextHookEx` entrega o `WM_KEYUP` físico da tecla Win ao `StartMenuExperienceHost` antes dos eventos injetados entrarem na fila, fazendo o Menu Iniciar abrir primeiro e conflitar com o Pulse Runner.

**How to apply:** Ao detectar toque isolado na tecla Win em `src-tauri/src/launcher/win_key.rs`, injete 3 eventos (`[0xE8 DOWN, 0xE8 UP, vk_win KEYUP]`) e retorne `LRESULT(1)` consumindo o `WM_KEYUP` físico. Assim o Windows vê `Win+0xE8` seguido de `Win-up`, nunca abre o Menu Iniciar e libera o modificador sem travar atalhos (`Win+E/R/V/L/D`).
