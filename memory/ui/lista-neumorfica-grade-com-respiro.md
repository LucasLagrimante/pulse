---
name: lista-neumorfica-grade-com-respiro
description: Grade com cards neumórficos precisa de padding de respiro e sombra suave para não clipar nas bordas e no fim
module: ui
metadata:
  type: project
---
**Why:** Sombra `raised` (18px blur) em grade densa com `gap` pequeno clipava nas laterais/scroll e ficava feia; o último card (Lixeira) perdia a sombra no fim da rolagem.
**How to apply:** Lista com `padding: 6px 12px 26px`, `overflow-x: clip`, `scrollbar-gutter: stable`; cards com `border-radius: 18px` + `raised-sm` (hover `raised`); `.selected` com `border-color: accent` sobre `accent-soft`. Vale para qualquer grade de cards `.panel` rolável.
