---
name: panel-flex-grow-espaco-vazio
description: Painel .panel com conteúdo curto dentro de coluna flex estica e cria espaço vazio; usar flex 0 0 auto + height auto
module: ui
metadata:
  type: project
---
**Why:** `App.css` define `.panel { flex: 1; height: 100% }` global. Um `.panel` de conteúdo curto (ex.: `ScanFoldersBar`) como filho de um wrapper `flex-direction: column` cresce junto com o painel de lista ao lado e vira uma faixa enorme de espaço vazio.

**How to apply:**
- Em qualquer `.panel` que deva medir só o próprio conteúdo, sobrescrever com `flex: 0 0 auto; height: auto; min-height: 0;` (só `flex-shrink: 0` não basta — `flex-grow` continua 1).
- Padrão já usado em `.disk-toolbar.panel` e `.scan-folders-panel`.
