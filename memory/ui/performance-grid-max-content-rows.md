---
name: performance-grid-max-content-rows
description: Linhas da grade de Desempenho devem usar minmax(max-content, 1fr) para não comprimir os cards
module: ui
metadata:
  type: project
---
**Why:** `.focused-card` tem `height: 100%` e o painel de gráfico `flex: 1` (basis 0), então tracks `auto`/`minmax(0, 1fr)` medem só o conteúdo não-flexível (~74px) e o gráfico transborda sobre o card vizinho em janelas mais baixas.

**How to apply:**
- Manter `grid-auto-rows: minmax(max-content, 1fr)` em `.focused-grid` (identity.css e App.css).
- Em ≤1050px/≤760px usar `grid-auto-rows: max-content` e `height: auto`.
- Ao inspecionar via DevTools/CDP, conferir `card.scrollHeight == card.clientHeight`; se scrollHeight for maior, a track comprimiu o card.
