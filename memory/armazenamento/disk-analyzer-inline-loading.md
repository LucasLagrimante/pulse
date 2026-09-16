---
name: disk-analyzer-inline-loading
description: O loading dos painéis de análise de disco depende apenas do cache local e não do inventário de unidades
module: armazenamento
metadata:
  type: project
---
**Why:** Acoplar o loading da árvore/treemap ao inventário de unidades ou rodar PowerShell no backend atrasa em segundos a renderização do cache local já salvo.
**How to apply:** O loading interno de `DiskAnalyzer` deve depender exclusivamente de `isRestoringScan` (`get_cached_disk_scan`). A enumeração de unidades (`get_system_drives`) deve rodar em segundo plano via APIs Win32 nativas (`GetLogicalDrives`, `GetVolumeInformationW`, `GetDiskFreeSpaceExW`) sem travar a árvore.
