---
name: winget-falhas-codigo-de-saida-e-admin
description: Erro de upgrade do winget se explica pelo código de saída (HRESULT), não pela saída; MSIX com serviço (0x80073D28) exige admin; causa real fica no DiagOutputDir
module: desktop
metadata:
  type: project
---
**Why:** em 2026-11-14 o "Atualizar tudo" mostrou a tabela de pacotes como mensagem de erro: o backend repassava os primeiros 4000 caracteres da saída, e o `upgrade --all` só devolve `0x8A15002C` ("concluído com falhas"). O log do winget mostrou a causa: Claude, WSL e Codex (MSIX que registra serviço) falham com `0x80073D28` sem elevação; iCUE e Anaconda falharam no próprio instalador (`0x8A150006`).
**How to apply:**
- Traduzir pelo código (`describe_exit_code` em `app_updates.rs`). A saída entra só como `detail` filtrado, sem tabela e sem barra `\r`.
- O lote roda um `upgrade --id --exact` por pacote e devolve `BatchUpgradeReport`. Nunca voltar ao `--all`.
- `needsAdmin` → `upgrade_winget_package_elevated` (`Start-Process -Verb RunAs` via `-EncodedCommand`; 1223 = UAC recusado). Repetir em sequência, um aviso do UAC por vez.
- Elevado com o app aberto: o log mostra `0x80073D02`, mas o winget sai com 0 ("Leaf command succeeded") e o Windows aplica o pacote quando o app fecha. Não é falso positivo do Pulse.
- `0x8A15008E` = o instalador mudou de tecnologia (desinstalar e instalar de novo). `0x8A150014` no PowerShell 7.6: `--id` não acha o pacote que a listagem mostra, só a busca pelo moniker acha. Não fazer fallback pelo nome.
- Para diagnosticar: `/mnt/c/Users/lucas/AppData/Local/Packages/Microsoft.DesktopAppInstaller_8wekyb3d8bbwe/LocalState/DiagOutputDir/WinGet-*.log`, filtrando por `Terminating context` e `installer failed`.
