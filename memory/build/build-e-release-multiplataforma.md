---
name: build-e-release-multiplataforma
description: Pipeline multiplataforma de build local, sincronização de landing e verificação de release (Windows 11, Linux .deb/.AppImage e macOS experimental)
module: build
metadata:
  type: project
---

**Why:** O Pulse passou a suportar Windows 11 e Linux (com macOS experimental no CI). `self_update/unix.rs` lê `linux.appimage` e `linux.deb` em `release.json`, e o Worker da landing precisa redirecionar `.exe`, `.deb`, `.AppImage` e `.dmg` sem jamais inventar SHA-256 ou tamanho para plataformas ainda não publicadas.

**How to apply:**
- Build local: `npm run build:native` (Windows no WSL, delega automático para `build-linux.sh`/`build-macos.sh` fora do WSL), `npm run build:linux` (`.deb` + `.AppImage`, com `APPIMAGE_EXTRACT_AND_RUN=1` automático sem FUSE 2), `npm run build:macos` e `bash scripts/rust-check.sh [--linux]`.
- Release: aguarde todos os jobs de `release.yml` antes de rodar `npm run lp:sync:release` e confirme todos os hashes por plataforma com `node scripts/release.mjs verify <versão>`.
