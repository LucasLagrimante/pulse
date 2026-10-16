---
name: windows-native-toolchain
description: Build do Tauri no WSL via ponte NTFS para o toolchain Rust MSVC do Windows
module: build
metadata:
  type: project
---

**Why:** O repositório vive no WSL (`/home/lucas/projects/pulse`), mas o backend Rust/Tauri exige toolchain MSVC (`cargo.exe`, `link.exe`, `vcvars64.bat`) e bundler NSIS no Windows. Compilar via rede `\\wsl.localhost\...` (9P) é lento e `cmd.exe` rejeita caminhos UNC como `cwd`.

**How to apply:**
- Use `npm run build:native` (ou `npm run tauri:build`), que executa `scripts/build-native.sh`.
- O script compila o frontend no WSL, copia os fontes para NTFS (`C:\Users\lucas\AppData\Local\Temp\pulse-src`), compila com `CARGO_TARGET_DIR=C:\Users\lucas\AppData\Local\Temp\pulse-target`, traz os binários de volta para `src-tauri/target/release/` e sincroniza a Landing Page via `scripts/sync-landing.mjs`.
