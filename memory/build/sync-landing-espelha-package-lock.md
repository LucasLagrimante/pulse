---
name: sync-landing-espelha-package-lock
description: scripts/sync-landing.mjs precisa espelhar package-lock.json junto com package.json para ../pulse-lp, senão npm ci falha no deploy
module: build
metadata:
  type: project
---

**Why:** O Dependabot atualizou `vite` em `landing/package.json` e `landing/package-lock.json` no monorepo `pulse`, mas `scripts/sync-landing.mjs` só espelhava `package.json` para `../pulse-lp`. No deploy do `pulse-lp`, `npm ci` falhou com `Invalid: lock file's vite@6.4.4 does not satisfy vite@6.4.5`.

**How to apply:** Manter `package-lock.json` na lista de arquivos espelhados por `scripts/sync-landing.mjs` e manter `version` sincronizada entre `landing/package.json` e `landing/package-lock.json`. Antes de publicar `../pulse-lp`, validar `npm ci --dry-run` se `package.json` tiver mudado.
