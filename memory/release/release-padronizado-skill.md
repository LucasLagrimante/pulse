---
name: release-padronizado-skill
description: Todo release do Pulse sai pela skill lancar-versao (scripts/release.mjs) — tipo SemVer pelos commits, notas na tag anotada, mensagens fixas e verify no ar
module: release
metadata:
  type: feedback
  pin: true
---
Em 2026-11-13 o usuário disse que os releases estavam "muito despadronizados" e pediu um fluxo único para patch, minor e major. Exemplos do problema: a tag v1.5.0 foi para um commit `fix` posterior ao `chore(release)`, as mensagens de sync variavam entre `build(release)` e `chore(landing)`, e as notas da Release só traziam o SHA.
**Why:** cada release improvisado decidia ordem, mensagem e tipo de um jeito, e o auto-update depende de o `release.json` no ar ter o SHA exato do asset do CI.
**How to apply:** sempre usar a skill `.agents/skills/lancar-versao`: `release.mjs check` → gate → `bump <tipo>` → `chore(release): versão X.Y.Z` → tag anotada com as notas → CI → `lp:sync:release` → pulse-lp → `release.mjs verify`. "Uma menor" = minor. Major só com confirmação explícita. Nunca reaproveitar uma versão já publicada.
