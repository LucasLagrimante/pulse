---
name: network-and-physical-disks
description: Coleta de rede e discos físicos do módulo Desempenho sem PDH localizado e com cache de inventário
module: desempenho
metadata:
  type: project
---
**Why:** Contadores PDH têm nomes localizados no Windows pt-BR (quebram com `\Network Interface(*)`), e `Get-PhysicalDisk` é caro para rodar a cada ciclo de 5s.

**How to apply:**
- Rede em `performance_devices.rs` via `GetIfTable2` (IpHelper/Ndis) com delta de octetos em estado estático; ignorar loopback (Type 24) e tunnel (131).
- Discos físicos via `Get-PhysicalDisk`/`Get-StorageReliabilityCounter` com `hidden_command` e cache Mutex de 60 s.
- Taxas por segundo só após a 2ª amostra; ausência sempre vira `Capability` com `unavailableReason`, nunca zero.
- Volume/capacidade vêm de `disk_analyzer::get_system_drives` no mesmo snapshot (`get_performance_devices`).
