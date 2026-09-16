---
name: compactacao-por-altura-de-janela
description: Densidade dos cards do Desempenho muda por @media (max-height) e listas rolam dentro do card
module: ui
metadata:
  type: project
---
**Why:** Reduzir só os paddings base deixava a grade ainda alta para 1280x720/1366x768; limitar o conteúdo por altura é o que zera a rolagem da página.

**How to apply:**
- Em `@media (max-height: 820px)` (identity.css): `gap` da grade 10px, `.focused-card` 12px 14px, `.storage-device-body` `max-height: 164px`, `.network-adapter-list` `max-height: 96px`.
- As listas já têm `overflow-y: auto` + `min-height: 0`, então o `max-height` faz elas rolarem dentro do card em vez de alongar a linha.
- Referência medida (6 cards, 3 colunas, dados completos): 1920x1080 → 429px/linha; 1500x918 → 348; 1366x768 → 278; 1280x720 → 273 (todos sem rolagem).
- Textos longos ao lado de listas estreitas devem encurtar (ex.: volume virou "390 GB livres" em vez de "390 GB livres de 932 GB").
