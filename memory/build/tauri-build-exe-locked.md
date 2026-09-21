---
name: tauri-build-exe-locked
description: tauri build falha com os error 5 no target/release exe porque o Pulse ainda está em execução no tray
module: build
metadata:
  type: project
---
**Why:** O Windows trava executáveis em execução; o Pulse minimiza para a bandeja e continua rodando, então o cargo não consegue remover `target\release\pulse-windows.exe` e o build aborta com `Access is denied (os error 5)` — parece erro de compilação mas é lock de arquivo.
**How to apply:** Use sempre `npm run tauri:build` (roda `scripts/kill-running-app.mjs` → `taskkill /IM pulse-windows.exe /F` antes do build; no-op fora do Windows e nunca falha o build). Para buildar e abrir o instalador NSIS mais recente em seguida: `npm run tauri:build:install` (`scripts/run-installer.mjs`, pode pedir UAC). O lançador usa `cmd /c start` (ShellExecute): spawn direto usa CreateProcess, que não eleva e falha silenciosamente quando o instalador exige elevação. O lock só aparece quando fontes Rust mudam e forçam relink — builds só-frontend passam mesmo com o app aberto. Se persistir sem processo ativo, suspeite do Defender RTP e exclua a pasta `target` (requer admin).
