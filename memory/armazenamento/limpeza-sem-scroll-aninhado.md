---
name: limpeza-sem-scroll-aninhado
description: Aba Limpeza tem um único rolador interno; wrappers com height 100% criavam scroll externo que empurrava o cabeçalho
module: armazenamento
metadata:
  type: project
---
**Why:** Os wrappers das subabas usavam `height: 100%` somado à altura das tabs, estourando o módulo e criando um scroll externo no `.workspace` além do scroll da lista — ao focar/clicar a Lixeira no fim, o navegador rolava o externo e "jogava tudo para cima".
**How to apply:** Wrappers com `flex: 1; min-height: 0; height: auto; overflow: hidden`; `.storage-module` e `.disk-cleanup-container` com `overflow: hidden`; só `.cleanup-category-list` tem `overflow-y: auto`. Grade com `align-items: start` para abrir "Maiores itens" não esticar o vizinho.
