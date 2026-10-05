---
name: cotas-sem-polling-fantasma
description: Client HTTP global no backend e cancelamento do loop de cotas ao desmontar a aba
module: telemetria
metadata:
  type: project
---
**Why:** `http_client()` construía um pool reqwest+TLS por chamada de provedor; e o loop de até 18 tentativas do `SubscriptionUsage` continuava invocando o backend por ~12 s após trocar de aba (componente lazy desmonta).

**How to apply:**
- `static HTTP_CLIENT: OnceLock<Client>` em `subscription_usage.rs`; `http_client()` retorna clone barato (Arc), mesmos timeouts.
- `disposedRef` em `SubscriptionUsage.load`: checar no loop, antes de cada `setState` e no `finally`; cleanup no unmount.
- Loops de quota do flyout (App.tsx) já eram foreground-gated com `disposed` — manter o padrão em todo polling novo.
