---
name: landing-sem-grade-e-header-responsivo-860
description: Proibição de fundo quadriculado na Hero da Landing Page e alinhamento simétrico do header/hero em viewports < 860px
module: ui
metadata:
  type: feedback
---

**Why:** Na Landing Page antiga, o fundo quadriculado (`.hero::after` com `--grid-line`) tinha aparência genérica de IA. Além disso, em larguras entre `621px` e `860px`, ocultar `.site-nav` (`display: none`) fazia `.header-actions` colar na logo à esquerda (pois só `.site-nav` tinha `margin-left: auto`), e a Hero alinhada à esquerda ficava desbalanceada em viewports intermediárias.

**How to apply:**
- Nunca usar padrões de grade/quadriculado (`linear-gradient` em malha) no fundo da Landing Page; preservar apenas a superfície mineral neumórfica limpa.
- Manter `.site-header .container` com `justify-content: space-between` e aplicar `margin-left: auto` em `.header-actions` quando `.site-nav` estiver oculto (`<= 920px`).
- Manter a Hero centralizada (`text-align: center; align-items: center`) em eixo simétrico sobre a janela do simulador em todas as larguras de tela.
