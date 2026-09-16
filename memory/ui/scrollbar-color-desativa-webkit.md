---
name: scrollbar-color-desativa-webkit
description: Definir scrollbar-color ou scrollbar-width não-auto num elemento desliga o ::-webkit-scrollbar e a barra perde o tema
module: ui
metadata:
  type: feedback
  pin: true
---

**Why:** No Chromium/WebView2, `scrollbar-color`/`scrollbar-width` com valor diferente de `auto` fazem o navegador ignorar todo `::-webkit-scrollbar`. A regra hardcoded em `App.css` (`.startup-list { scrollbar-color: #4a544c #141619 }`) deixava a barra de Inicialização escura fixa no tema claro — enquanto Cotas de IA, sem essas props, usava o skin neumórfico temático global de `identity.css`.

**How to apply:**
- Nunca fixar `scrollbar-color`/`scrollbar-width` em listas; deixar herdar o global `::-webkit-scrollbar` de `identity.css` (`--neu-scroll-thumb` / `--neu-scroll-track`).
- Para esconder barra use `scrollbar-width: none` apenas onde é intencional (tray/flyout).
- Overview (`detail-grid`) mantém trilho silencioso via `::-webkit-scrollbar-thumb` transparente e `var(--neu-accent)` no hover, sem `scrollbar-color`.
