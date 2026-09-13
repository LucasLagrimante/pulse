---
name: safe-capability-metrics
description: Validação segura de métricas de telemetria com valores nulos vindos de Option no Rust
module: telemetria
metadata:
  type: project
---
**Why:** No Rust `Option::None` serializa para `null` no JSON. `null === undefined` é falso em JS, logo checagens estritas falham e chamadas a `toLocaleString()` sobre `null` lançavam `TypeError` fatal no módulo Desempenho.
**How to apply:** Trate valores ausentes com `value != null` em vez de `value !== undefined`. Na função `metric()`, valide `v == null || typeof v !== "number" || Number.isNaN(v)` antes de qualquer formatação numérica.
