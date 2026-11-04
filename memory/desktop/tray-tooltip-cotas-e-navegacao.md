---
name: tray-tooltip-cotas-e-navegacao
description: Tooltip nativo do ícone mostra contagem regressiva das cotas (limite 127 UTF-16) e card do flyout abre a aba Cotas via show_main_module
module: desktop
metadata:
  type: project
---
**Why:** O hover do ícone da bandeja é a única superfície sem abrir janela; `szTip` do Windows trunca em 128 unidades UTF-16 com terminador.
**How to apply:** `compose_tooltip` põe cotas primeiro (`Codex 5h 46% ↻2h30 · 7d 91% ↻6d21h`) e temperaturas só se couberem; recalcula a cada 60 s a partir de `cached_providers_snapshot()` (nunca dispara scan) e a cada `notify_subscription_updated`. Clique no card do flyout → `invoke("show_main_module", { module })` → evento `pulse://navigate-area` (via `emit_to("main")`) validado contra `ALL_NAVIGATION_ITEMS`.
- Claude via CLI (`claude /usage`) só traz reset textual ("resets Nov 4, 3pm (America/Sao_Paulo)"); `parse_cli_reset_at` converte para timestamp (fuso local, ano inferido). Sem `resetsAtUnixSeconds` o countdown some.
