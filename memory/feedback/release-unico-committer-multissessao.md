---
name: release-unico-committer-multissessao
description: Com várias sessões do Claude editando o pulse, só uma faz os commits e publica a tag, e só depois que todas terminam e tudo é revalidado
module: feedback
metadata:
  type: feedback
---
**Why:** Em 2026-11-10 três sessões editavam o repo ao mesmo tempo. Uma delas publicou a v1.1.1 sem o trabalho das outras e um commit ficou pela metade porque outra sessão alterou `footprint.rs` no meio dele. O usuário decidiu: "um único agente faz o commit e publica a tag" quando todos estiverem alinhados e testados.
**How to apply:**
- Antes de commitar, rodar `ListAgents`, avisar as outras sessões por `SendMessage` (com `notify_when_idle`) e pedir que não rodem `git add`, `reset` ou `commit`.
- Só commitar quando todas confirmarem que terminaram. Revalidar tudo nesse momento (typecheck, testes, lp:check, clippy e cargo test), porque a validação anterior perde o valor quando alguém edita depois.
- O hook de pré-commit roda uma revisão que passa de 120 s: rodar o `git commit` com timeout maior ou em background.
