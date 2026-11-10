---
name: estilo-global-de-input-vaza-no-dt-search
description: Regras globais com !important do identity.css (input search, botões) vazam ou faltam em componentes novos; neutralize/estilize no escopo do componente
module: ui
metadata:
  type: project
---
**Why:** `input[type="search"]` no identity.css tem fundo/borda/sombra/padding com `!important` e desenhava uma segunda caixa dentro da cápsula `.dt-search`. Já `.button-primary`/`.button-secondary` só ganham forma dentro de `.modal-actions`; fora dela, nos modais de IA, apareciam crus.
**How to apply:** `DataTable.css` zera o input com `.dt-search input[type="search"]` + `!important`; os botões dos modais `.disk-ai-modal-card` têm regra própria com tokens no identity.css. Ao criar campo ou botão, conferir os estilos computados no preview do navegador, não só a classe.
