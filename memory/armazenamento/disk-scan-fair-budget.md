---
name: disk-scan-fair-budget
description: Varredura de disco com alta vazão e prioridade de segundo plano sem contenção no Windows
module: armazenamento
metadata:
  type: project
---
**Why:** Sleeps artificiais e yield excessivo transformam varreduras de SSD em minutos de espera, enquanto chamadas redundantes de metadados saturam o antivírus do kernel.
**How to apply:** Use `THREAD_MODE_BACKGROUND_BEGIN` para prioridade IoPriorityVeryLow sem travar o PC; aproveite metadados em cache de `DirEntry` sem syscalls extras para cada arquivo; processe arquivos comuns sem descer em recursão e use emissão preguiçosa de progresso com yield cooperativo a cada 2048 itens.
