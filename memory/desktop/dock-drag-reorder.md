---
name: dock-drag-reorder
description: Arraste na Dock nativa guarda chave do app (não índice) e SetCapture/ReleaseCapture ficam em Input::KeyboardAndMouse
module: desktop
metadata:
  type: project
---

**Why:** O `TIMER_RECONCILE_ID` reconstrói `DockState.items` a cada 1,1 s; guardar o índice de origem do arraste quebrava a soltura quando a lista mudava durante o gesto. Além disso, `SetCapture`/`ReleaseCapture` NÃO são exportados de `Win32::UI::WindowsAndMessaging` nesta versão do crate `windows` — estão em `Win32::UI::Input::KeyboardAndMouse` (como em `topbar.rs`).
**How to apply:** Em `custom_dock/dock_window.rs`, `drag_source: Option<(String, bool, f32)>` guarda (chave, fixado?, x inicial); `commit_dock_reorder` reordena só dentro do mesmo grupo (fixados/abertos), persiste via `beta_features::save_dock_order` e chama `notify_config_changed`. `rebuild_items` aplica `dock_unpinned_order` com `sort_by_key` estável. Durante o arraste, congele auto-hide, desligue magnificação e use `SetCapture`/`ReleaseCapture` com `WM_CAPTURECHANGED` para abortar sem clique.
