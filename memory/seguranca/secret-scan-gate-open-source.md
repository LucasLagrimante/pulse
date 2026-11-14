---
name: secret-scan-gate-open-source
description: Repo é open source e público; o secret-scan global (dotfiles) bloqueia chave e dado pessoal — fixtures usam usuário genérico (dev, DEV-PC)
module: seguranca
metadata:
  type: project
  pin: true
---
**Why:** em 2026-11-14 a varredura achou 0 chaves, mas 70+ caminhos com o usuário real (`C:\Users\<eu>`, `/home/<eu>`) e o hostname em fixtures, docs e memória. O usuário decidiu NÃO reescrever o histórico (sem segredo real) e bloquear daqui pra frente. Como o repo é público, dado pessoal bloqueia (não só avisa).
**How to apply:**
- Fixtures/exemplos/memória: `C:\Users\dev`, `/home/dev`, `DEV-PC\dev`, `/mnt/c/Users/<usuario-windows>`; caminho do repo como `~/projects/pulse` ou relativo.
- O gate é global (pre-commit/pre-push em `dotfiles/modules/antigravity/hooks/secret-scan/`); aqui só fica `.secret-scan.json` (e-mail público de contato liberado).
- Auditoria: `node ~/projects/dotfiles/modules/antigravity/hooks/secret-scan/secret-scan.mjs [--history]`.
