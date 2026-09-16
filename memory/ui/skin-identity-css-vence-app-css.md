---
name: skin-identity-css-vence-app-css
description: Para mudar estilo dos módulos é preciso editar o bloco neumórfico em identity.css, não o App.css
module: ui
metadata:
  type: project
  pin: true
---
**Why:** `identity.css` (importado depois de `App.css`) redefine `.focused-card`, `.panel`, `.detailed-chart-panel`, `.sparkline-container`, `.mini-capacity-bar-track` com `!important`. Editar só o `App.css` não muda nada em runtime — gastei uma rodada inteira de ajuste sem efeito.

**How to apply:**
- Ajustes de padding/fonte/raio dos cards do Desempenho vão no bloco "Workspace e Painéis Neumórficos" (~linha 554+) e "Gráficos e Indicadores" (~linha 663+) de `src/identity.css`.
- Antes de mexer, confirmar a origem da regra: `[...document.styleSheets]` filtrando `selectorText`/`conditionText`, ou ler `getComputedStyle` e procurar quem casa.
- O `App.css` continua válido para regras sem `!important` do skin (ex.: `.detailed-chart-body`, `.detailed-chart-timeline`).
