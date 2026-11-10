---
name: winget-upgrade-progress-indeterminado
description: "Progresso de instalação do winget é indeterminado + log por linha; cada pacote tem job próprio no ServicesHub (paralelo)"
metadata:
  type: project
---

`upgrade_winget_package` / `upgrade_all_winget_packages` aceitam `AppHandle` e emitem `winget-upgrade-progress` (`{ id, phase, line }`, fases `started|log|finished|failed|timeout`, id `*` reservado ao lote) a cada linha de stdout/stderr, lida em threads próprias com `BufReader::lines()`.

**Why:** o winget **não expõe porcentagem** — uma barra com "62%" seria dado fictício, proibido pelo AGENTS.md §1.2. E o `winget list --upgrade-available` do host tem 57 pacotes: um único `busyUpdate: string` fazia o segundo clique cancelar o loading visual do primeiro (que continuava rodando em background, invisível).

**How to apply:**
- Front: estado das instalações (`jobs` por pacote, `busyAll`, `batchLog`, listener de progresso) vive em `src/upgradeJobsStore.ts` (escopo de módulo + `useSyncExternalStore`), nunca em `useState` do `ServicesHub`: o hub é desmontado ao trocar de área e o progresso sumia com o winget ainda rodando. Fim de instalação marca `needsRefresh`, consumido pelo hub ao (re)montar.
- Barra sempre `sh-progress-indeterminate` + texto literal do winget + tempo decorrido (`formatElapsed`).
- Só reconsultar `list_pending_updates` quando `runningUpdates.size === 0`; reconferir a cada item derruba a lista e embaralha o estado visual.
- Nunca capturar stdout só com `wait_with_output` se a UI precisa de progresso: o buffer do OS enche e o winget trava escrevendo.
- Em teste, `listen` precisa ser mockado e `cleanup()` explícito no `afterEach` (o auto-cleanup do RTL não roda com `globals: true` neste projeto — buscas por role/text acham nós do teste anterior).