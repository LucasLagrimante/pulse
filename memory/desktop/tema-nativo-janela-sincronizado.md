---
name: tema-nativo-janela-sincronizado
description: O tema claro/escuro do shell precisa sincronizar tema e fundo da janela nativa; a moldura do Windows não segue o CSS
module: desktop
metadata:
  type: project
---

**Why:** A barra de título nativa segue o tema do Windows, não o `data-theme` do shell. Com o app no claro e o sistema no escuro, a moldura (caption/preenchimento do WebView2) destoava — faixa escura no topo da janela.

**How to apply:**
- `set_tray_icon_theme` (`src-tauri/src/tray.rs`) agora também chama `apply_window_theme`: `WebviewWindow::set_theme(Some(Theme::Light|Dark))` e `set_background_color(Color(...))` na cor de fundo do tema (`#e6edf5` / `#181b20`).
- O frontend já invoca esse comando no `useLayoutEffect` de `useNeumorphicTheme` a cada troca, então basta manter a sincronização ali.
- `Color` é `tauri::window::Color(r, g, b, a)`; `set_theme` não está atrás de feature flag.
