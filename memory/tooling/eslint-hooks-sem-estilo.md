---
name: eslint-hooks-sem-estilo
description: ESLint do frontend cobre só bugs (react-hooks + typescript-eslint recommended); exhaustive-deps se corrige sem inflar deps de funções
module: tooling
metadata:
  type: project
---

**Why:** Adotado em 2026-11-10 (`eslint.config.js`, `npm run lint`, dentro do `verify`). Acrescentar uma função recriada a cada render às deps de um `useEffect` o faz rodar em todo render e repete IPC ao Rust (ex.: `analyze_disk_cleanup`) sem parar.
**How to apply:** Para `exhaustive-deps`: valor derivado com `?? []` → `useMemo`; efeito só de mount/troca de área → `eslint-disable-next-line` com justificativa; nunca só aumentar a lista com funções. Regras do React Compiler (v7 recommended) ficam desligadas; `catch {}` liberado; sem Prettier/regras de estilo. typescript-eslint exige TS < 6.1.
