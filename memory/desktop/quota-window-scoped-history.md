---
name: quota-window-scoped-history
description: Gráficos de cotas de IA devem ter histórico limitado à duração da janela (5h, 7d, 30d) e resetar com o ciclo
module: desktop
metadata:
  type: project
---
**Why:** Controlar 30 dias para uma janela de 5 horas polui a leitura com ciclos passados e datas repetidas. Cada janela faz sentido quando dura estritamente o seu ciclo e reseta quando o período termina.

**How to apply:**
- `computeCycleInfo` ancora o ciclo: 5h usa os 300 min atuais (`resetsAt - 5h`), semanal usa 7 dias e mensal 30 dias; créditos usam 30 dias.
- Quando `resetsAt <= now`, o medidor e backend renovam para 100% restante ("Ciclo renovado") e o gráfico inicia a nova janela a partir do reset.
- Backend (`get_quota_history`) e frontend filtram pontos por `since_unix_ms = cycleStartMs`.
- No SVG do gráfico, intervalos curtos (≤ 24h) exibem horário (`HH:MM`) nos eixos em vez de datas do mesmo dia repetidas.
