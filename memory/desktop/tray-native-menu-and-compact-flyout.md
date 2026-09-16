---
name: tray-native-menu-and-compact-flyout
description: Separação de clique direito (menu nativo de contexto com áudio) e esquerdo (flyout compacto sem scrollbar)
module: desktop
metadata:
  type: project
---
**Why:** O botão direito no tray deve fornecer o menu físico nativo do Windows (com troca direta de endpoints de áudio, minimização e encerramento) em vez de duplicar o flyout, e o painel flyout nunca pode exibir barra de rolagem.
**How to apply:** No Rust, configure o TrayIcon com `show_menu_on_left_click(false)` e anexe um menu nativo com submenus dinâmicos de áudio (`audio_out:*` e `audio_in:*`), delegando apenas o clique esquerdo para `toggle_flyout_internal`. No flyout, force `overflow: hidden !important;`, organize telemetria e cotas em grids 2 colunas/2x2 e exiba os top 5 processos com linhas dedicadas e legíveis.
