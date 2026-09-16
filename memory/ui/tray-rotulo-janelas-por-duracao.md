---
name: tray-rotulo-janelas-por-duracao
description: No flyout do tray, o rótulo curto das janelas de cota deve derivar de windowMinutes, não de heurística sobre o texto do label
module: ui
metadata:
  type: feedback
---

**Why:**
A heurística antiga procurava "sess"/"sem"/"5h" no label; "Gemini Models · 5 horas" não casava com nada e caía no truncamento genérico, exibindo "Gemi" no tray. A janela semanal casava por acaso ("Semanal"), ficando correta.

**How to apply:**
1. `shortQuotaWindowLabel` usa `windowMinutes` (300 → "5h", 10080 → "7d", 43200 → "30d") e só cai no texto como fallback.
2. `selectFlyoutQuotaWindows` agrupa por duração, escolhe a janela mais crítica (menor percentual) de cada bucket e ordena da menor para a maior janela.
3. Plataformas como Antigravity têm 4 janelas (2 modelos × 5h/semanal); exibir 2 sem agrupar perde a janela de 5h e duplica "7d".
