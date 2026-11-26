---
name: nome-oficial-app-pe
description: Rótulos de apps/jogos na Dock, Barra Superior, Runner e Atividade vêm do ProductName do recurso de versão do PE, nunca do stem do executável
module: desktop
metadata:
  type: feedback
---

**Why:** O usuário mostrou a Dock exibindo `Marvel-Win64-Shipping` como rótulo de um jogo. Jogos Unreal/Marvel usam executáveis técnicos; o nome oficial está em `ProductName`/`FileDescription` do recurso de versão, não no nome do arquivo. `Get-Process .Path` e o stem do exe não resolvem isso.
**How to apply:** Use `process_identity::official_name_for_path` (cacheado) — `ProductName` primeiro, `FileDescription` depois — e `choose_display_name(stem, official, friendly)`; apelidos conhecidos (`prettify_dock_name`/`prettify_exe_name`) têm prioridade, e nomes técnicos (`*-shipping`, `Unreal Engine`, iguais ao stem) são descartados por `clean_version_name`. Para UWP/AUMID, manter o nome amigável do pacote; sem PE legível, cair para o título da janela.
