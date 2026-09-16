---
name: npm-install-wsl-ebadplatform
description: Instalar dependências npm no WSL exige --force por causa da devDep win32-only do Tauri CLI
module: tooling
metadata:
  type: project
---

**Why:** `npm install <pkg>` no WSL falha com `EBADPLATFORM` porque `@tauri-apps/cli-win32-x64-msvc` declara `os: win32`/`cpu: x64`. Sem `--force` nenhuma dependência é adicionada.

**How to apply:**
- Rodar `npm install <pkg> --force`; o `package-lock.json` permanece consistente (só a nova dep é inserida, nada win32 é removido).
- O aviso "removed 5 packages" refere-se a pacotes opcionais de plataforma em `node_modules`, não ao lockfile.
- Conferir com `git diff package-lock.json` que apenas a dep nova foi adicionada e validar com `npm run typecheck` + `npm test`.
