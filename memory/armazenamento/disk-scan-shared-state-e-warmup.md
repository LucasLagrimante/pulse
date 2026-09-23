---
name: disk-scan-shared-state-e-warmup
description: Análise de disco usa store compartilhado no frontend e cache permanente; varredura automática no startup é proibida, só o botão Escanear varre
module: armazenamento
metadata:
  type: project
---
**Why:** O warm-up automático (revarrer se ausente/parcial/velho >30 min) escondia o cache por ~19 min a cada reabertura e o usuário percebeu como "cache perdido"; a regra agora é cache permanente até o próximo "Escanear".
**How to apply:**
- Rust: `disk_analyzer::init` só pré-carrega `get_cached_disk_scan` em memória; nunca chama `run_disk_scan`. `read_cached_scan_from_disk` aceita qualquer JSON parseável (sem expiração por idade/versão/stop_reason; só ignora arquivos >32 MB).
- Rust: `CachedDiskScan.tree_version` é só diagnóstico; poda com `prune_tree_to_budget(MAX_TREE_NODES = 40_000)` antes de persistir/enviar; escrita `write` + `rename`; `PERSISTED_CACHE` evita reler o arquivo.
- Frontend: `src/diskScanStore.ts` guarda resultado entre remontagens; `reconcileDiskAnalyzer` nunca limpa `cachedScan`/`scanResult` com retorno nulo — só uma nova varredura substitui.
