---
name: theme-tokens-mandatory
description: É estritamente obrigatório usar tokens de tema CSS (var(--neu-*) e var(--theme-*)) em qualquer elemento, tela ou modal
module: ui
metadata:
  type: project
  pin: true
---
**Why:** Telas e modais novos (como UninstallHub e ServicesHub) foram criados com cores hexadecimais escuras fixas (#191c20, #141619, #2b3138), quebrando totalmente a legibilidade no tema claro (Porcelain) com cartões pretos e texto invisível.
**How to apply:** Nunca use cores hex/rgb fixas em fundos, textos ou bordas de novos componentes ou arquivos CSS separados. Use estritamente as variáveis semânticas do tema: `var(--neu-surface)`, `var(--neu-surface-sunken)`, `var(--neu-surface-hover)`, `var(--neu-divider)`, `var(--neu-text-title)`, `var(--neu-text-primary)`, `var(--neu-text-secondary)`, `var(--neu-text-muted)`, `var(--neu-accent)`, `var(--neu-danger)` e `var(--neu-success)`. Em modais ou diálogos, garanta que o backdrop e cartão respeitem `[data-theme]`.
