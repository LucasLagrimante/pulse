---
name: ordem-e-drag-provedores-ia
description: Reordenar cards de cotas arrastando persiste a ordem no backend e reflete no tray
module: telemetria
metadata:
  type: project
---

**Why:** O tray é outra janela e só recebe o array `providers` do relatório — não conhece estado local da tela de cotas. Por isso a ordem precisa ser persistida e aplicada no backend, não só no React.

**How to apply:**
1. Guardar `providerOrder` em `subscription-preferences.json` e expor em `SubscriptionSettings.providerOrder`.
2. `apply_provider_order` roda no scan e antes de emitir o relatório: dois `sort_by_key` estáveis (índice salvo, depois `disabled`) — assim desativados vão sempre ao final e o tray herda a ordem.
3. Comando `set_provider_order(order)` sanitiza contra `PROVIDER_IDS`, salva, reordena o cache e emite o evento na hora (sem rescan).
4. Frontend: DnD HTML5 nativo (`draggable` + `onDragStart/onDragOver/onDrop`), sem bibliotecas; `reorderIds` move o id e `displayProviders` = ativos ordenados + desativados ao final.
5. Cuidado com CSS: `.quota-provider-heading > div` (genérico) aplicava `flex-direction: column` no `.quota-header-end` e empilhava os botões — usar seletor específico (`> .quota-provider-identity`).
