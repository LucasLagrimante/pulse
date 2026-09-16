---
name: tray-quota-dual-bars-contrast
description: Layout de duas barras, cabeçalho em linha e contraste estrito claro/escuro para cotas no Tray Flyout
module: desktop
metadata:
  type: feedback
---
**Why:** Janelas de cota de 5h e 7d geravam ambiguidade com barra única, enquanto porcentagens ficavam camufladas no tema claro (texto claro sobre fundo claro) e escuro (mesma cor do fundo).
**How to apply:** Mover o plano para a 1ª linha (`.flyout-quota-head`) para acomodar 2 barras (`5h` e `7d`) sem expandir altura nem criar scroll. Usar vetores SVG oficiais em `ProviderIcon`. Em `identity.css`, forçar contraste explícito: no tema claro, `.flyout-quota-value` com `#0f172a` e barra com trilho `#e2e8f0`; no tema escuro, `.flyout-quota-value` com `#ffffff`, text-shadow e barra destacada (`#f59e0b`/`#38bdf8`), garantindo legibilidade perfeita mesmo em 90% e 100%.
