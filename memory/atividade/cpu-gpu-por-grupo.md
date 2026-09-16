---
name: cpu-gpu-por-grupo
description: Medição de CPU e GPU por grupo de processos na Atividade (delta de GetProcessTimes e contadores PDH GPU Engine)
module: atividade
metadata:
  type: project
---
**Why:** CPU por processo exige delta entre amostras (`GetProcessTimes` não dá percentual) e o contador PDH de GPU só entrega valores formatáveis após a segunda coleta — a primeira falha com `PDH_INVALID_ARGUMENT (0xC0000BBA)` e faria a GPU parecer indisponível.
**How to apply:** Em `process_usage.rs`, calcule CPU com `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + delta normalizado por `available_parallelism()` (100% = máquina) e chame `PdhCollectQueryData` duas vezes antes de `PdhGetFormattedCounterArrayW` com `PdhAddEnglishCounterW` (`\GPU Engine(*)\Utilization Percentage`, independe do idioma do Windows). Agregue somando os PIDs membros; `None` = indisponível (nunca zero falso) e GPU medida sem uso = 0% legítimo.
