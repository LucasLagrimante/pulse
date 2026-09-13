---
name: unified-agents-guidelines
description: Diretrizes consolidadas em AGENTS.md para otimização de I/O, Fluent UI sem altura estática e controle unificado do Windows
module: desktop
metadata:
  type: project
---
**Why:** O desenvolvimento por agentes exige alinhamento estrito para não regredir proteções de I/O, DPC e contratos do Windows.
**How to apply:** Mantenha `AGENTS.md` como fonte canônica do projeto. Siga a trindade: I/O offloaded com `spawn_blocking` e `yield_now`, interface fluida sem alturas fixas em pixels (`flex: 1; min-height: 0`), e integridade do SO usando `StartupApproved` e zero dados fictícios.
