---
name: antigravity-cli-modelo-premium-sem-remaining
description: agy /usage ganhou linha de modelo premium (Gemini 4 Argon) sem a palavra "Remaining"; parser deve ler por TAB
module: telemetria
metadata:
  type: project
---
**Why:** A saída passou a ter `Gemini 4 Argon\tGemini 4 Argon Weekly Limit\t100%\t<reset>`; o parser antigo exigia o token "Remaining" e descartava a cota em silêncio.
**How to apply:** `split_antigravity_line` separa por TAB (modelo, tipo, %, reset) e classifica a duração só por "weekly"/"five hour" na coluna do tipo. Ids `agy-N` são posicionais e indexam o histórico de 30 dias — não renomear. Antes de mexer no parser, capture a saída real com `agy -p '/usage' --output-format text | cat -A`.
