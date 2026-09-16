---
name: git-status-estatizado-wsl
description: No WSL o git status pode marcar arquivos como modificados mesmo com conteúdo idêntico ao HEAD
module: tooling
metadata:
  type: project
---
**Why:** `git status` listava arquivos (tsconfig, vite.config, main.tsx, tauri.svg…) como `M`, mas `git diff`/`git diff --summary` vinham vazios e `git hash-object` batia com `git rev-parse HEAD:arquivo`; é cache de stat desatualizado no filesystem compartilhado WSL/Windows.
**How to apply:** Rode `git add <arquivo>` nos arquivos com diff vazio: como o conteúdo é idêntico, nada entra no commit e o stat é atualizado. Confirme com `git status --porcelain` antes de agrupar commits.
