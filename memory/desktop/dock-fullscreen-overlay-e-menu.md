---
name: dock-fullscreen-overlay-e-menu
description: Dock em jogo de tela cheia — sempre oculta, borda inferior NÃO revela; só tecla Windows (runner/Menu Iniciar nativo) mostra
module: desktop
metadata:
  type: project
---

**Why:** Em 2026-11-26 o usuário pediu: no jogo, passar o mouse no fundo da tela não pode revelar a Dock (atrapalha jogar); ela só aparece com a tecla Windows. Com o runner pausado por anti-cheat, o Iniciar nativo abria sem a Dock, a lupa não reagia (sem `WM_MOUSEMOVE`) e focar janela já aberta falhava (foreground lock), só abrir app novo funcionava.
**How to apply:** Em `custom_dock/dock_window.rs::update_visibility_state`: fullscreen ⇒ oculta mesmo sem auto-hide, `near_bottom` ignorado. Visível só por `reveal_until` (toque Win), `hold_from_launcher` (runner) ou `start_menu_open` (foreground é `StartMenuExperienceHost`/`SearchHost`, detectado lendo o foreground com cache por hwnd — sem hook). Com o Iniciar aberto a lupa vem de `GetCursorPos` por tick. `focus_window_reliable` usa `AttachThreadInput` + `SwitchToThisWindow` quando `SetForegroundWindow` é recusado — nunca injetar teclas. `menu_open` congela o auto-hide no popup.
