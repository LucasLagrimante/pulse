---
name: npm-install-wsl-quebra-nativos-windows
description: npm install rodado no WSL troca os nativos opcionais por Linux e quebra o vite build no Windows
module: build
metadata:
  type: project
---

**Why:** Um `npm install` executado no WSL em 2026-09-16 16:33 removeu os binários opcionais win32 de `node_modules` (rolldown, lightningcss, esbuild, rollup) e deixou só os `linux-x64`, fazendo `npm run build` (rolldown-vite) falhar no Windows com "Cannot find native binding".

**How to apply:**
- Rodar `npm install`/`npm ci` sempre no mesmo SO em que se executa `tauri build` (Windows, via PowerShell/cmd).
- Se precisar manter os dois (testes no WSL + build no Windows), restaure o nativo win32 sem npm: `npm pack <pkg-win32-x64>@<versão-do-lock>` + extrair em `node_modules/<pkg>`.
- Pacotes afetados no Pulse: `@rolldown/binding-win32-x64-msvc`, `lightningcss-win32-x64-msvc`, `@esbuild/win32-x64`, `@rollup/rollup-win32-x64-msvc`.
- Sintoma típico: `Error beforeBuildCommand` com "Cannot find native binding" ou falha ao carregar `lightningcss/node`.
