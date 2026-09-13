---
name: Compact Storage Header
description: Mantém o cabeçalho do analisador de disco enxuto para priorizar árvore e treemap
module: armazenamento
metadata:
  type: project
---
**Why:**
Na largura útil após a sidebar, o resumo de capacidade era empurrado para uma linha exclusiva e o cabeçalho da página consumia espaço desproporcional.

**How to apply:**
Use uma classe de cabeçalho específica da tela de armazenamento, reduza espaçamentos e mantenha seletor, capacidade e ação na mesma faixa quando o container tiver pelo menos 760 px. Abaixo desse limite, refluía a capacidade para uma segunda linha; não comprima números a ponto de perder legibilidade.
