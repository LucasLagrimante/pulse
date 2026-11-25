---
name: quota-burn-rate-and-projection
description: Cálculo de ritmo de consumo (%/h ou %/dia), projeção até o reset e destaque de renovação para cotas esgotadas
module: telemetria
metadata:
  type: project
---
**Why:** Exibir apenas `% restante` não mostra se o ritmo atual de gasto vai estourar a janela (5h, semanal ou mensal) antes do fechamento nem destaca claramente quando uma cota esgotada (0%) volta.
**How to apply:**
- `src/quotaPace.ts` concentra o cálculo puro (`computeQuotaPace`, `computeRecentHistoryPace`, `computeCreditsBurnEstimate`), com pisos mínimos de tempo decorrido (`15min` em 5h, `4h` em 7d, `12h` em 30d) e `elapsedMs = Math.max(0, ...)` para meses de 31 dias.
- `QuotaMeter` e `QuotaMatrixCell` exibem taxa + veredito na própria barra/célula e banner destacado `Renova em ... (data/hora)` quando esgotada (`remaining <= 0`).
- `QuotaHistoryChart` estende o eixo X até `resetsAt` com linha tracejada `.quota-history-projection-line` (protegida em `identity.css` com `:not(.quota-history-projection-line)`).
