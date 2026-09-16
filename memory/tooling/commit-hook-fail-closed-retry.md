---
name: commit-hook-fail-closed-retry
description: Pre-commit hook Antigravity pode falhar "fail-closed" por erro transitório de IA; repetir o commit, nunca usar --no-verify
module: tooling
metadata:
  type: project
---

**Why:** No commit `05d4aac` o hook de revisão bloqueou com "revisores indisponíveis (fail-closed): Objeto estruturado de veredito ausente no retorno da IA". Não era problema do diff — na segunda tentativa o mesmo commit passou ("Nenhum risco crítico encontrado").

**How to apply:**
- Ao ver `❌ Commit bloqueado — revisores indisponíveis (fail-closed)`, rode o `git commit` de novo (o stage está intacto).
- Nunca contornar com `git commit --no-verify`; a regra do fluxo de commit proíbe explicitamente.
- Se falhar repetidamente, é indisponibilidade real do serviço de revisão — aguardar e tentar, não burlar.
