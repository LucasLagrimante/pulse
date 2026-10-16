---
name: dedup-installed-apps
description: Evitar duplicação de programas instalados no inventário de desinstalação
module: desinstalar
metadata:
  type: project
---
**Why:** HKCU e HKU não possuem redirecionamento WOW64 para Uninstall no Windows 64-bit; consultar visões Native64 e Native32 ou comparar entradas incluindo a visão mantinha apps per-user duplicados na interface. Além disso, HKLM exigia o caminho completo da subchave para enumeração correta.
**How to apply:** Ao enumerar Uninstall no HKLM, passe o subcaminho completo `SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\<name>`. Em HKCU e HKU, varra apenas `Native64`. Na deduplicação, compare nome de exibição, folha da chave e comando de desinstalação, mesclando metadados e descartando a visão da chave de unicidade.
