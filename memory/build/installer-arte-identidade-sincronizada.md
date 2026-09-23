---
name: installer-arte-identidade-sincronizada
description: Artes NSIS seguem a identidade do app; regenerar após trocar ícone ou paleta
module: build
metadata:
  type: project
---

**Why:** `sidebar.bmp`/`header.bmp` embutem o `128x128.png` e a paleta da época em que foram gerados; trocar `pulse.svg` sem regenerar deixa o instalador com marca defasada (ocorreu em set/2026: BMPs com ícone pré-neumórfico).

**How to apply:**
- Após mudar `public/pulse.svg` ou a paleta de `identity.css`, rodar na ordem: `npx tauri icon public/pulse.svg` → `powershell ./scripts/installer-art.ps1` → `npm run tauri:build` (tudo no Windows; BMP + NSIS exigem PowerShell/MSVC).
- Tamanhos fixos do NSIS MUI: sidebar 164x314, header 150x57; idioma PT-BR via `languages: ["PortugueseBR"]` + `displayLanguageSelector: false` no `tauri.conf.json`.
