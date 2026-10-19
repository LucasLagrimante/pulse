---
name: services-hub-skeleton-e-ux
description: "Skeletons de alta fidelidade e toolbar com busca e botões agrupados no ServicesHub"
module: ui
metadata:
  type: feedback
---

**Why:** Deixar telas vazias com texto simples ("Consultando…") e botões soltos em barras isoladas passa sensação de lentidão e quebra de layout no Windows 11.
**How to apply:** Em listas de pacotes/serviços, exiba banners de status com spinner e múltiplos skeletons com shimmer sem cores fixas (`color-mix` sobre `--neu-text-primary`). Agrupe o campo de busca de aplicativos à esquerda e os botões de ação ("VERIFICAR" + "ATUALIZAR TUDO") à direita em `.sh-toolbar-actions`.
