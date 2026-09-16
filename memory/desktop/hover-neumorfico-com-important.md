---
name: hover-neumorfico-com-important
description: Botões neumórficos precisam de :hover com !important, senão o estado base vence
module: desktop
metadata:
  type: project
---

**Why:** Os overrides de tema no `identity.css` usam `[data-theme] ... { background: var(--neu-surface) !important }` no estado base. O `:hover` sem `!important` (no `App.css`) perde para o `!important` do base, então o botão parece "morto" mesmo tendo hover declarado.

**How to apply:**
- Todo botão com override `[data-theme]` precisa também de um bloco `[data-theme] ...:hover` com `!important` seguindo o padrão do projeto:
  `background: var(--neu-surface-hover) !important; border-color: var(--neu-accent) !important; color: var(--neu-text-title) !important;`
- Variante destrutiva usa `var(--neu-danger)` no lugar do accent.
- Conferir o trio completo: base `[data-theme]`, `:hover` e (quando existir) `.active`/`:disabled`.
