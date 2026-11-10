---
name: nsis-upgrade-mata-processo-via-hooks
description: Hooks NSIS encerram o Pulse antes de instalar/desinstalar
module: instalador
metadata:
  type: project
  pin: true
---
**Why:** O Pulse fica residente na bandeja; no upgrade o desinstalador antigo não apaga o exe travado e o setup aborta com erros em cascata (visto no bump 0.1.0→1.0.0). Versão/nome não eram a causa.
**How to apply:** Manter `installerHooks: ./windows/hooks.nsh` no NSIS com `taskkill /F /IM` (pulse-windows.exe e Pulse.exe) em `NSIS_HOOK_PREINSTALL` e `NSIS_HOOK_PREUNINSTALL`. Se renomear o `productName`, atualizar o `/IM` junto.
- Rodar via `nsExec::Exec` (oculto, com `Push/Pop $0`), nunca `ExecWait`: taskkill é app de console e o `ExecWait` piscava um terminal por chamada durante a instalação (corrigido em 2026-11-10). O desinstalador de versões ≤1.3.0 ainda pisca no 1º upgrade.
