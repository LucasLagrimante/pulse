---
name: sinal-forte-obrigatorio-para-revisao-de-servico
description: Em detecção, revisão só entra com sinal forte; fatos corroborantes apenas explicam. Medido: 3 heurísticas "razoáveis" deram 21 falsos positivos em 313 serviços.
module: seguranca
metadata:
  type: project
  pin: true
---

**Why:** A primeira versão de `classify_service` (`src-tauri/src/services.rs`) tratava
qualquer coincidência como sinal e marcou **25 de 313 serviços (8%)** em revisão, **21 deles
legítimos**: ChatGPT, Claude, WSL Service, Google Updater, AnyDesk, Edge Update, Microsoft
Defender. Cada "heurística plausível" era descrição de software normal:

- "automático + conta de sistema + fora do Windows" = *qualquer* software instalado
- "`%PROGRAMDATA%` é gravável pelo usuário" = onde mora o Defender (`ProgramData\Microsoft\Windows Defender\Platform`)
- "sem `CompanyName`" = Easy Anti-Cheat, Corsair Device Listing, SignalRgb
- "`%LOCALAPPDATA%` é gravável" = `AppData\Local\Programs\ChatGPT\ChatGPT.exe`, local padrão de instalação por usuário

**How to apply:**
- **Revisão exige sinal forte**: executável em pasta gravável *exceto* subárvore de instalação
  (`\appdata\local\programs\`, `windowsapps\`, `packages\`), ou nome imitando componente do Windows.
  Fatos corroborantes (conta de sistema, automático, sem fabricante) só *acrescentam* motivos
  quando um sinal forte já existe.
- `suspicionReasons` é **vazio fora da revisão** — senão a tela pinta aviso na maioria da lista.
- Antes de dar credibilidade a qualquer heurística de segurança: rodar contra a máquina real e
  listar os positivos. Medido final: 313 serviços → 282 sistema / 25 usuário / **0 revisão** / 6 sem dados.
- Teste que impede regressão de alarme falso: `review * 100 < total * 3` com dados reais.
- Ver também `memory/seguranca/lookalike-limiar-1-damerau.md`.
