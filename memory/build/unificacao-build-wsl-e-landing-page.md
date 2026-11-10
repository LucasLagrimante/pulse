---
name: unificacao-build-wsl-e-landing-page
description: Pipeline de build nativo WSL->Windows e sincronização obrigatória do instalador e funcionalidades com a Landing Page
module: build
metadata:
  type: project
  pin: true
---

**Why:** O repositório vive no WSL (`~/projects/pulse`), enquanto o compilador Rust/MSVC e o bundler NSIS rodam no Windows em staging NTFS (`%LOCALAPPDATA%\pulse-build\src` e `\target`). Invocar `npm.cmd install` via interop trava em stdin, e rodar `cargo.exe build --release` antes de `tauri.js build` compila o binário duas vezes (sem e com `custom-protocol`).

**How to apply:**
- Provisionar `@tauri-apps/cli-win32-x64-msvc` em NTFS via `npm pack` + `tar` no WSL e invocar `cmd.exe /c run-build.bat < /dev/null` direto de `cd "$STAGING"` chamando apenas `tauri.js build --bundles nsis -- --locked`.
- Os artefatos voltam para `src-tauri/target/release/pulse-windows.exe` e `src-tauri/target/release/bundle/nsis/Pulse_<versão>_x64-setup.exe`, e `scripts/sync-landing.mjs` atualiza `landing/` e `../pulse-lp` (instalador, tamanho, SHA-256 e `release.json`).
- Toda nova funcionalidade ou módulo adicionado ao Pulse DEVE atualizar o simulador interativo e as seções de recursos em `landing/`.
