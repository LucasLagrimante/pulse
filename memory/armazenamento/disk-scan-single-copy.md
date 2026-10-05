---
name: disk-scan-single-copy
description: Resultado da varredura de disco em cópia única no backend (PERSISTED_CACHE), sem árvore no progresso final nem SCAN_STATE
module: armazenamento
metadata:
  type: project
---
**Why:** Cada varredura clonava a árvore de até 40k nós 4× em RAM (progresso final + SCAN_STATE.result + CachedDiskScan + PERSISTED_CACHE), além da cópia no frontend.

**How to apply:**
- Progresso final com `partial_result: None`; resultado final vive só em `PERSISTED_CACHE` (+ disco).
- Persistir/publicar em memória DENTRO da thread blocking ANTES do `emit` final, para o reconcile ler o cache novo.
- `SCAN_STATE.result` só como fallback quando `persist_disk_scan` falha; frontend usa `scanState?.result ?? cache.result` sem mudanças.
