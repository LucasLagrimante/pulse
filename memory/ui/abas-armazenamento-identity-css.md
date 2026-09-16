---
name: abas-armazenamento-identity-css
description: Abas (storage-tabs-bar/btn) são estilizadas em identity.css, separadas do segmented control; App.css tem regras legadas perdedoras
module: ui
metadata:
  type: project
---
**Why:** `identity.css` é importado depois de `App.css` (App.tsx linhas 6-7) e usa `!important`, então as regras `.storage-tab-btn*` legadas do `App.css` (seção "MÓDULO 4") nunca vencem. Editar só o App.css não muda a UI renderizada.

**How to apply:**
- Estilizar as abas de Armazenamento em `identity.css`, no bloco "Abas de Armazenamento", mantendo-as fora do grupo do segmented control (`.detailed-chart-timeframes`, `.disk-view-modes`) para não afetar os gráficos.
- Ativo = accent (texto + `::after` sublinhado + `--neu-accent-soft` de fundo); badge usa pílula `--neu-accent-soft`.
