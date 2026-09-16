---
name: tauri-html5-drag-drop
description: Drag & drop HTML5 no frontend exige dragDropEnabled false na janela Tauri
module: desktop
metadata:
  type: project
  pin: true
---

**Why:** No Windows o Tauri substitui o handler de drag and drop do WebView2 para emitir `DragDropEvents` de arquivos. Com isso, o DnD HTML5 do frontend não dispara e o cursor fica em `no-drop` — o card não "encaixa" em lugar nenhum.

**How to apply:**
- Definir `"dragDropEnabled": false` na janela em `src-tauri/tauri.conf.json` (só é necessário onde há DnD HTML5; ex.: janela `main`).
- Isso mapeia para `WebviewBuilder::disable_drag_drop_handler`; mudanças no `tauri.conf.json` exigem reiniciar o `tauri dev`.
- Se algum dia for preciso aceitar drop de arquivos do SO, reativar o campo e usar `window.on_drag_drop_event` em vez do DnD HTML5.
