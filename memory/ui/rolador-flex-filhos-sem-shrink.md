---
name: rolador-flex-filhos-sem-shrink
description: Em rolador flex-column (overflow auto), filhos precisam de flex 0 0 auto; e faixas .panel extras precisam height auto, senão herdam 100%
module: ui
metadata:
  type: project
---
**Why:** Na Limpeza, os grupos dentro de `.cleanup-category-list` (flex column + overflow) encolheram e os cards se sobrepuseram; e o novo `.auto-cleanup.panel` herdou `height: 100%` do `.panel`, escondendo a lista inteira. Testes jsdom não pegam nenhum dos dois.
**How to apply:** Filhos de rolador flex: `flex: 0 0 auto`. Qualquer faixa nova com classe `.panel` acima do rolador: `flex: 0 0 auto; height: auto !important`. Validar visualmente no Vite dev com `__TAURI_INTERNALS__` mockado via `page.addInitScript` (Playwright) — não há mock de IPC no repo.
