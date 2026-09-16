---
name: commit-hook-antigravity-timeout
description: Commit no Pulse dispara hook de revisão do Antigravity CLI que pode passar de 2 min em diffs grandes
module: tooling
metadata:
  type: project
---
**Why:** O hook de pre-commit roda "Antigravity CLI (Gemini 3.8 Flash)" para revisar o staged; em diffs grandes ele excedeu 120s e o shell matou o comando antes de o commit concluir, deixando a árvore staged sem commit.
**How to apply:** Ao commitar no Pulse, use timeout de 600000 ms (10 min). Se o hook estourar, os arquivos continuam staged — reexecute `git commit` com a mesma mensagem. Nunca use `--no-verify`.
