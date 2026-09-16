---
name: troca-de-tema-sem-cross-fade
description: A troca de tema deve ser instantânea; desligar transições com a classe .theme-switching durante a aplicação do data-theme
module: ui
metadata:
  type: project
---

**Why:** `html, body, #root` e dezenas de superfícies têm `transition` de cor/sombra. Ao trocar `data-theme`, o novo tema era pintado com cross-fade de ~250ms e só depois "pegava" — o usuário reprovou o efeito lento no botão de tema do cabeçalho.

**How to apply:**
- Em `useNeumorphicTheme` (`src/App.tsx`), aplicar `data-theme` em `useLayoutEffect` com a classe `theme-switching` no `documentElement`.
- `identity.css`: `html.theme-switching, html.theme-switching *, ...::before, ...::after { transition: none !important }`.
- Remover a classe só depois de dois `requestAnimationFrame` (um frame ainda pinta com transição ligada e anima); usar token em `useRef` para não religar cedo em toggles rápidos.
- Vale para o app principal e o `tray-flyout` (hook compartilhado).
