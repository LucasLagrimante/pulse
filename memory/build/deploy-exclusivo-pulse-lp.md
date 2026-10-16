---
name: deploy-exclusivo-pulse-lp
description: O deploy da Landing Page na Cloudflare é exclusivo do repositório pulse-lp
module: build
metadata:
  type: project
---

**Why:** O repositório `pulse` hospeda o app desktop Windows e não possui secrets da Cloudflare. A Landing Page é implantada exclusivamente pelo workflow do repositório irmão `pulse-lp`.
**How to apply:** Nunca crie workflows de deploy web (`deploy-landing.yml`) no repositório `pulse`. Atualizações na pasta `landing/` do monorepo devem ser sincronizadas via `npm run lp:sync` para `pulse-lp`, onde o deploy no Cloudflare Workers ocorre de forma isolada.
