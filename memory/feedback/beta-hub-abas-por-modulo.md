---
name: beta-hub-abas-por-modulo
description: Recursos Beta devem aparecer como abas com estado visível, não como lista contínua de três cards gigantes
metadata:
  type: feedback
---
**Why:** O usuário rejeitou a lista vertical contínua dos três módulos beta: não dava para enxergar onde um módulo começa e termina, especialmente o Pulse Shell que tem dezenas de controles encapsulados.
**How to apply:** Em `BetaHub.tsx`, usar `role="tablist"` com `.storage-tabs-bar`/`.storage-tab-btn` de `identity.css`, um `role="tabpanel"` por módulo e chip `ATIVO`/`INATIVO` na aba (tokens `--neu-success`/`--neu-accent`). Manter trilho `position: sticky` para as abas não sumirem no scroll do painel. Ao testar, selecionar a aba com `getByRole("tab", { name })` antes de buscar controles do módulo; verificar `aria-selected` em vez de depender de conteúdo alheio.
