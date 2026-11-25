---
name: Dimensões explícitas em SVGs inline de ícones (.file-symbol)
description: SVGs inline sem width/height crescem com o flex container quando falta regra base CSS
module: ui
metadata:
  type: project
---

**Why:** Na migração para `identity.css`, a regra global `.file-symbol` sumiu; `<FileIcon />` sem `width`/`height` virou filho direto de `.tree-cell-name` (`display: flex`) na aba "Maiores arquivos" e passou a escalar junto com a largura da janela.
**How to apply:** Todo componente SVG de ícone em `src/Identity.tsx` deve declarar `width`/`height` intrínsecos de fallback e ter regra base (`.file-symbol` / `.module-icon`) com `width`, `height` e `flex: none` em `src/identity.css`; overrides locais por contexto continuam vencendo por especificidade.
