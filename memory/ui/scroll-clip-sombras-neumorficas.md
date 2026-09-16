---
name: scroll-clip-sombras-neumorficas
description: Contêiner de scroll com overflow-y auto recorta box-shadow dos filhos; corrigir com padding + margem negativa
module: ui
metadata:
  type: project
---
**Why:** No tema neomorfismo as linhas (`.startup-item`) usam `box-shadow` elevada, mas `.startup-list` tinha apenas `overflow-y: auto`. Pela spec, quando um eixo de overflow não é `visible`, o outro calcula para `auto`, então a sombra era recortada em retângulo nas laterais/inferior, sem vazar.
**How to apply:** Nunca dependa de `overflow: visible` em contêiner rolável. Reserve espaço físico para a sombra com `padding` (ex.: `2px 14–18px 20px`) e compense com margem negativa equivalente (`margin: 0 -14px / -18px`) para manter o alinhamento. Padrão já usado em `.detail-grid .process-list/.recent-list` (App.css). Prefira isso a remover o `overflow: hidden` do `.panel`, que recortaria do mesmo jeito.
