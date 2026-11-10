---
name: sidebar-largura-rotulo-em-uma-linha
description: Itens do menu de módulos ficam sempre em uma linha; a largura da sidebar é dimensionada pelo maior rótulo medido
module: ui
metadata:
  type: feedback
---
**Why:** Rótulos de duas palavras ("Inicialização e segurança") quebravam em duas linhas na sidebar de 228px; o usuário pediu linha única padronizada (2026-11-10).
**How to apply:** `.nav-item` usa `white-space: nowrap` e `.nav-title` reticências só como rede de segurança. Sidebar com 268px (228px ≤1050px, com fonte .8rem) — o maior rótulo mede ~158px em Segoe UI semibold e ~169px em fonte de fallback. Ao renomear ou adicionar módulo, medir `scrollWidth` vs `clientWidth` de `.nav-title` no preview, inclusive a 960px (largura mínima da janela).
