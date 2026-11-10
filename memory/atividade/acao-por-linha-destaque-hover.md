---
name: acao-por-linha-destaque-hover
description: Toda lista com ação por linha usa o padrão compartilhado `.row-hl` + `.row-action` (DataTable.css); tabelas densas usam `.dt-*` com colunas em `--dt-cols`
module: atividade
metadata:
  type: feedback
---
**Why:** O usuário não sabia em qual linha estava o FECHAR da Atividade (sem hover) e pediu padronizar o mesmo destaque na Remoção e no resto do app (2026-11-10).
**How to apply:** `src/DataTable.css` (importado no App.tsx) define `.row-hl` (faixa + fundo via `::before` atrás do conteúdo, funciona sob `!important` legado), `.row-hl-danger` no botão destrutivo (linha fica vermelha ao mirar) e `.row-action`. Tabelas: `.dt .dt--<tela>` com `--dt-cols`, cabeçalho `.dt-sort` (desconhecidos sempre no fim). Tabela `.dt` completa (toolbar com `.dt-search` + `.dt-summary` no lugar de cards de estatística, cabeçalho ordenável, linhas `.dt-row`) em Atividade, Desinstalar, Serviços, Atualizações e Inicialização (2026-11-10, a pedido: "levar a atualização visual da Desinstalar"). Qualquer lista nova com ação por linha deve usar o mesmo. Desinstalar abre ordenada por tamanho decrescente. Texto sobre preenchimento: `--neu-on-accent`/`--neu-on-danger`.
