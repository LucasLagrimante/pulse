---
name: paginas-ocupam-largura-total
description: Telas do app e seções da landing devem ocupar 100% da largura do container; evitar cards com max-width fixo alinhados à esquerda
module: ui
metadata:
  type: feedback
---
**Why:** Na tela "Sobre o Pulse" (`.info-page` / `.info-card`) o card tinha `width: min(100%, 720px)` com `align-items: flex-start`, deixando-o colado à esquerda. O mesmo aconteceu na FAQ da landing (`.faq { max-width: 840px }`, 2026-11-04). Nas duas vezes o usuário pediu que preenchesse a largura disponível.
**How to apply:** Em telas de módulo sob `.module-dashboard`, usar `align-items: stretch` e `width: 100%` no card principal; na landing, listas de cards (`.faq` etc.) usam `width: 100%` do `.container`. Reservar `max-width` só para parágrafos de leitura, nunca para o contêiner da superfície.
