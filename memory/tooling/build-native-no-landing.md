---
name: build-native-no-landing
description: tauri:build é o build-native.sh (rejeita --bundles); instalador local sem tocar em landing/ é `npm run build:native -- --no-landing`
module: tooling
metadata:
  type: project
---
`npm run tauri:build` aponta para `scripts/build-native.sh`, que já passa `--bundles nsis` internamente e aborta com "argumento desconhecido" para qualquer outra flag.
**Why:** README antigo mandava `npm run tauri:build -- --bundles nsis`, que quebra; e o passo 5 (`sync-landing.mjs`) suja `landing/index.html` e `landing/public/*`.
**How to apply:** só instalador local → `npm run build:native -- --no-landing`. Flags aceitas: `--rust`, `--install`, `--no-landing`. No PowerShell nativo, `npx tauri build` (`tauri.conf.json` já tem `"targets": "nsis"`; não sincroniza a landing).
