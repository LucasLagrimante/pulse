---
name: telemetry-bars-instrument-style
description: Telemetria de CPU/RAM (gráficos, flyout e ícone da bandeja) usa colunas verticais densas em pixels reais com fade temporal, nunca frações do viewBox
module: ui
metadata:
  type: feedback
---

**Why:** O usuário reprovou os sparklines Bézier e, na 2ª iteração, as colunas espaçadas ("muito espaçados, não ficou 100%"): o slot era uma fração do viewBox (100/N), então histórico curto inflava as barras e os gaps.
**How to apply:** `SparklineChart` mede a largura real com `ResizeObserver` + `clientWidth` (fallback 320 px em testes/SSR) e desenha em px reais no viewBox: `slot = min(barWidth + barGap, chartWidth / len)`, `thickness = min(barWidth, slot * 0.78)`, ancorado na borda direita (`startX = chartWidth - slot * len`) — o histórico cresce para a esquerda e o slot encolhe só quando excede a largura. Fade temporal com gradiente `userSpaceOnUse` de `occupiedStart` → `chartWidth` (opacidade 0.2 → 1.0); nunca `objectBoundingBox` (degenera com 1 amostra). Card e flyout usam `barWidth 4 / barGap 2`; o painel Desempenho usa `6 / 3`. O ícone da bandeja (`tray.rs::tray_icon_for_theme`) replica colunas âmbar com alpha 0.34 → 1.0.
