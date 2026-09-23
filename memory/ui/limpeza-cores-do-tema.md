---
name: limpeza-cores-do-tema
description: Limpeza de disco usa só variáveis neu; cor chapada escura some no tema claro e seletor errado mata a grade
module: ui
metadata:
  type: project
---
**Why:** Títulos `#e7e5df` chapados ficavam invisíveis no Porcelain claro; e o `identity.css` agrupava `.cleanup-category-list` no `display: flex` (matando a grade 2-col do `App.css`) e estilizava `.cleanup-category-card`, classe que não existe (a real é `.cleanup-category`).
**How to apply:** Textos/fundos/bordas/checkmark/mensagens só com `var(--neu-*)` em bloco próprio após as tabs; conferir no DOM que o seletor existe e que nenhuma regra anterior anula `display`. `toggle()` ignora categoria com `availableBytes <= 0`.
