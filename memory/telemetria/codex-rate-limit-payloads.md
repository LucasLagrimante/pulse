---
name: codex-rate-limit-payloads
description: Formatos distintos de rate limit do Codex por fonte (wham/usage oficial, logs locais e app-server) e a necessidade de aceitar snake_case, camelCase e singular/plural
module: telemetria
metadata:
  type: project
---

**Why:**
O endpoint oficial `GET https://chatgpt.com/backend-api/wham/usage` responde com `plan_type`, `rate_limit` (singular) contendo `primary_window`/`secondary_window` com `used_percent`, `limit_window_seconds`, `reset_at` e `reset_after_seconds`; `credits` e `rate_limit_reset_credits.available_count` ficam na raiz. O parser antigo procurava apenas `rate_limits.primary` (camel/snake) e ignorava a resposta 200, caindo no log local de dias atrás e exibindo "janela reiniciada" no lugar da cota de 5h.

**How to apply:**
1. Ao ler cotas do Codex, aceitar `rate_limits`/`rateLimits`/`rate_limit` e janelas `primary`/`primary_window`.
2. Reset: `resets_at`/`reset_at` (epoch s) com fallback `reset_after_seconds`/`resets_in_seconds` (relativo a agora), nunca inventar timestamp.
3. Logs locais `~/.codex/sessions/**.jsonl` usam o payload interno com `primary.resets_at`; o app-server usa `rateLimits` camelCase — os três formatos são válidos.
4. Snapshot cujo `reset_at` já passou continua marcado como indisponível; não presumir 100% restante.
