---
name: theme-toggle-icons-from-lucide
description: Ícones do botão de troca de tema vêm da lucide-react (Sun/Moon), nunca de glifos Unicode ou SVG desenhado à mão
module: ui
metadata:
  type: feedback
---

**Why:** Os glifos `☼`/`☾` no botão de tema (flyout da bandeja e app principal) renderizavam minúsculos, tortos e inconsistentes. O usuário reprovou e pediu pacote de ícones real.

**How to apply:**
- Usar `<Sun />`/`<Moon />` de `lucide-react` em `src/App.tsx`, com a classe `.theme-toggle-icon`.
- Flyout: 20px dentro do círculo de 30px (`.flyout-theme-toggle`). App principal: 16px + rótulo textual `CLARO`/`ESCURO`.
- Regra compartilhada em `identity.css`: `display: block; flex-shrink: 0; stroke-width: 2.1`.
- Cobertura em `App.test.tsx`: o botão deve conter `.lucide-sun`/`.lucide-moon` e não conter os glifos.
