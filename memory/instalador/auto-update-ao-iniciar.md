---
name: auto-update-ao-iniciar
description: Atualização automática do Pulse ao iniciar valida SHA-256 contra release.json e instala NSIS silencioso (/S) via PowerShell desacoplado
module: instalador
metadata:
  type: project
---
**Why:** O `hooks.nsh` roda `taskkill /F /IM pulse-windows.exe /T`; se o próprio Pulse disparasse o `setup.exe` direto como filho sem encerrar antes, o `/T` mataria o instalador, e o modo `/S` do NSIS não reabre o app sozinho.
**How to apply:**
- Estado em `%LOCALAPPDATA%\Pulse\auto-update.json` (`enabled: true` por padrão), exposto no topo (`ATUALIZAR AO INICIAR`), em `Sobre o Pulse` e no menu da bandeja.
- `self_update.rs` consulta `https://pulse.lagrimante.com.br/release.json` após delay de 6s, exige URL oficial, valida SHA-256 em memória e bloqueia repetição automática por 6h se a mesma versão já foi tentada sem concluir.
- O instalador verificado vai para `%TEMP%\pulse-self-update\` (preservado por `is_pulse_owned`); um PowerShell oculto espera o PID sair, roda `setup.exe /S -Wait` e relança o Pulse com `--updated-relaunch` (e `--minimized-to-tray` se estava na bandeja).
