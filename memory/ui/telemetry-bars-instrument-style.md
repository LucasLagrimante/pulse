---
name: telemetry-bars-instrument-style
description: Gráficos de telemetria usam curva Bézier suave + área translúcida em poço recuado neumórfico com crosshair interativo, cards inteiros clicáveis e reordenáveis
module: ui
metadata:
  type: feedback
---

**Why:** O usuário reprovou tanto as barras com fade quanto as barras verticais sólidas ("não estão bonitas nem com a cara do projeto") e pediu cards de recursos clicáveis (sem botão "Expandir" redundante) e reordenáveis.
**How to apply:** Em `src/Charts.tsx`, `SparklineChart` e `DetailedResourceChart` usam curva monotônica Fritsch-Carlson + gradiente vertical sutil via tokens `--neu-*`, poço recuado (`inset well`), ponto luminoso na última leitura e crosshair interativo no hover (sem cores hex/rgb). Em `PerformanceModule.tsx`, o card inteiro abre o detalhe e `TelemetryPreferences.cardOrder` controla a ordem com drag-and-drop e botões `←`/`→`.
