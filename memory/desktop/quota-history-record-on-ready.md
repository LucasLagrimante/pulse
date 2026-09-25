---
name: quota-history-record-on-ready
description: Histórico de cotas grava 1 ponto por janela só no scan ready, em JSONL local com piso de 10 min
module: desktop
metadata:
  type: project
---
**Why:** App, flyout e tray passam pelo mesmo `get_subscription_usage`; gravar no relatório final `ready` cobre os três sem spam. Só valores medidos entram — nunca zero fictício.

**How to apply:**
- Gravação em `quota_history::record_report()` (`%LOCALAPPDATA%\Pulse\quota-history.jsonl`), chamada no fim de `start_background_scan`; leituras `running` não geram pontos.
- Piso de 10 min por série (salto ≥5 % em ≥1 min grava); poda mantém 35 dias, consulta expõe 30 via `get_quota_history`.
- Gráfico por janela em `src/QuotaHistory.tsx` (`QuotaHistoryButton` no heading do `QuotaMeter`); série de saldo usa `windowId "__credits__"`.
- Popover com placement viewport-aware (`is-down/up` × `is-left/right` via `useLayoutEffect`): nunca ancorar fixo com `right: 0`, que jogava o popover para a esquerda, para baixo do menu lateral.
