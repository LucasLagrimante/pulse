---
name: Performance Charts and SVG Optimization
description: Melhores práticas para gráficos de telemetria em tempo real com SVG leve, memoização e isolamento de layout
module: telemetria
metadata:
  type: project
---
**Why:**
Telemetria em tempo real atualiza a cada 1s. Gráficos pesados ou re-renderizações globais causam layout thrashing e travamentos no Tray Flyout compacto e no painel.

**How to apply:**
Use SVG vetorial nativo com `shape-rendering: geometricPrecision` e CSS `contain: content; will-change: transform;`. Envolva gráficos com `React.memo` checando comprimentos e último valor. Mantenha buffers circulares em `useRef` para não invalidar callbacks e monte curvas Bézier monotônicas limitadas aos extremos.
