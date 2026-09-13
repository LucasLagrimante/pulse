---
name: recent-files-scan-and-ui-smoothness
description: Restrições de I/O em varreduras de arquivos e inicialização assíncrona escalonada para prevenir travamento do mouse no Windows
module: desempenho
metadata:
  type: project
---
**Why:** Varreduras recursivas síncronas de arquivos no Windows saturam filas de I/O e acionam picos de DPC no filtro do Windows Defender (WdFilter.sys), travando o cursor do mouse e a interface.
**How to apply:** Em `recent_files.rs`, execute o scanner em `tauri::async_runtime::spawn_blocking` com limites estritos (`MAX_DEPTH=3`, `MAX_FILES_TOTAL=2500`, timeout de 1.5s), ignorando symlinks e aplicando `std::thread::yield_now()` a cada 15 arquivos. No frontend, escalone as requisições iniciais (staggered startup) e exiba toast de loading e skeleton shimmer durante a busca.
