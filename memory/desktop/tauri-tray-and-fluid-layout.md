---
name: tauri-tray-and-fluid-layout
description: Padrão para janelas de bandeja flutuante e dimensionamento vertical completo no Tauri
module: desktop
metadata:
  type: project
---
**Why:** Janelas flyout da bandeja precisam fechar automaticamente ao perder foco para evitar ficarem presas na tela, e grids/listas em Tauri ficam cortadas sem adaptação de altura flexível.
**How to apply:** No Tauri v2, monitore `WindowEvent::Focused(false)` na janela `tray-flyout` para ocultá-la. No CSS, remova alturas estáticas e utilize `display: flex; flex-direction: column; flex: 1; min-height: 0; height: 100%` no container e nas listas scrolláveis (`.process-list`, `.recent-list`).
