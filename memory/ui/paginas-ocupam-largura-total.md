---
name: paginas-ocupam-largura-total
description: Telas de módulo do Pulse devem ocupar 100% da largura do workspace; evitar cards com max-width fixo alinhados à esquerda
module: ui
metadata:
  type: feedback
---
**Why:** Na tela "Sobre o Pulse" (`.info-page` / `.info-card`) o card tinha `width: min(100%, 720px)` com `align-items: flex-start`, deixando-o colado à esquerda e desproporcional em janelas largas. O usuário reportou como "feio" e pediu preenchimento da largura disponível.
**How to apply:** Em telas de módulo sob `.module-dashboard`, usar `align-items: stretch` e `width: 100%` no card principal; reservar `max-width` apenas para blocos de texto de leitura (ex: `.info-summary`), nunca para o contêiner da superfície.
