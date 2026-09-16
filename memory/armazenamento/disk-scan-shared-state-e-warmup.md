---
name: disk-scan-shared-state-e-warmup
description: Análise de disco usa store compartilhado no frontend, cache em memória no Rust e warm-up no startup; árvore final podada por cota global
module: armazenamento
metadata:
  type: project
---
**Why:** Sem cota global o `last-disk-scan.json` chegou a 405 MB (2,9 mi de itens) e cada abertura da aba custava ~8 s lendo/desserializando o cache; a aba ainda remontava e reconsultava tudo a cada troca de área.
**How to apply:**
- Rust: poda a árvore final com `prune_tree_to_budget(MAX_TREE_NODES = 40_000)` antes de persistir/enviar (somas reais em nós `::[budget]`). O orçamento é distribuído proporcionalmente ao tamanho do filho (não em partes iguais), para que os maiores ramos continuem navegáveis em profundidade; árvores que já cabem não são tocadas.
- Rust: `CachedDiskScan.tree_version` (atual `CACHE_VERSION`) invalida caches do algoritmo anterior; caches >32 MB são ignorados e a escrita é `write` + `rename`.
- Rust: mantém o cache persistido em `PERSISTED_CACHE` (memória) para `get_cached_disk_scan` não reler o arquivo.
- Rust: `disk_analyzer::init` roda no setup; após 4 s, se o cache estiver ausente/parcial/velho (>30 min), varre a última raiz em segundo plano.
- Frontend: `src/diskScanStore.ts` (useSyncExternalStore) guarda resultado/drives/progresso entre remontagens; `App` chama `ensureDiskAnalyzerLoaded()` só na janela main.
- Frontend: snapshots parciais de warm-up não substituem a árvore em cache; a troca só ocorre na reconciliação final.
