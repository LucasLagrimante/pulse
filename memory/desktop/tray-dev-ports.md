---
name: tray-dev-ports
description: Inspetor de portas e projetos dev (Windows + WSL2) no flyout e menu de contexto do tray
module: desktop
metadata:
  type: project
---
**Why:** Desenvolvedores e agentes deixam servidores locais rodando em portas do Windows ou WSL2; o tray precisa inspecionar sem acordar uma VM WSL parada e sem causar overflow no flyout de 740 px.
**How to apply:** Use `GetExtendedTcpTable` + leitura PEB em `dev_ports.rs` e consulte WSL somente após `wsl.exe -l --running -q` (priorizando entradas WSL sobre `wslrelay.exe`). No `TrayFlyout`, `PORTAS & PROJETOS DEV` e `VOLUME MIXER` recolhem-se mutuamente ao abrir e o menu do botão direito consome o snapshot em cache (`Portas e projetos`).
