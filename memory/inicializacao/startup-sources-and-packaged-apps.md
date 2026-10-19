---
name: startup-sources-and-packaged-apps
description: Mapeamento completo de inicialização no Windows cobrindo StartupFolder, WOW6432Node e pacotes AppX/MSIX
module: inicializacao
metadata:
  type: project
---
**Why:** Apps de boot no Windows 11 residem em três fontes distintas: chaves de registro `Run` (64 e 32-bit), pastas de atalhos `Startup` (usuário e comum) e pacotes AppX/MSIX (UWP/Store, como ChatGPT). Ler apenas `HKCU/HKLM\Run` oculta softwares de atalho (`Wispr Flow`) e empacotados (`ChatGPT`).
**How to apply:** Mapeie `HKCU/HKLM\Run`, `WOW6432Node\Run`, as pastas `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup` e `%PROGRAMDATA%` (rastreadas em `StartupApproved\StartupFolder` pelo nome do arquivo) e pacotes AppX via `AppModel\SystemAppData\<pkg>\<task_id>\State` (onde DWORD 2=ativado e 1/0=desativado). Para alternar, grave no respectivo registro mantendo integridade com as Configurações do Windows.
