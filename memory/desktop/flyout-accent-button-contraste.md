---
name: flyout-accent-button-contraste
description: Botões sobre --neu-accent usam texto escuro porque branco sobre âmbar claro falha no tema escuro
module: desktop
metadata:
  type: feedback
---
**Why:** `.flyout-btn-primary` com `color: #ffffff` sobre `--neu-accent: #f59e0b` (dark) tem contraste ~2:1 e o hover `#fbbf24` piora; no light (`#d97706`) era tolerável, por isso o bug só aparecia no escuro.
**How to apply:** Em `identity.css`, botões com fundo `var(--neu-accent)` usam `color: #141619 !important` (também no `:hover`) — passa nos dois temas. Regra vale para qualquer botão novo sobre accent/âmbar.
