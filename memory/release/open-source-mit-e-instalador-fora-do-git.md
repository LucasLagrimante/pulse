---
name: open-source-mit-e-instalador-fora-do-git
description: Pulse é open source sob MIT; o instalador .exe vive na GitHub Release (nunca no Git) e a landing não pode prometer "nenhum byte sai da máquina"
module: release
metadata:
  type: project
---

**Why:** Na abertura do repositório (2026-11-04) o usuário escolheu MIT, removeu `legacy-python/` e optou por publicar o instalador em GitHub Releases: cada release commitava ~3,6 MB de `.exe` no `pulse` e no `pulse-lp` (de onde sai o deploy Cloudflare). A landing dizia "nenhum byte sai da máquina", mas Cotas de IA e o assistente de disco usam rede.

**How to apply:**
- `sync-landing.mjs` lê `Pulse_<versão>_x64-setup.exe` do bundle, grava `installer.assetUrl` em `release.json`, apaga `download/` e falha se o Git rastrear `.exe`; o `worker.js` faz 302 de `/download/*.exe` para esse `assetUrl`.
- Ordem do release: tag `v<versão>` → CI publica (sem assinatura, ver [[assinatura-signpath-ci]]) → `npm run lp:sync:release` → só então push no `pulse-lp`. O download exige o repo `pulse` público.
- Toda função nova com rede vai no README ("Privacidade e rede") e em `#privacidade` da landing; scripts de build sem caminho fixo de usuário (`PULSE_WIN_TEMP`).
