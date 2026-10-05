---
name: classe-panel-estica-cartoes-pequenos
description: A classe .panel do App.css (flex:1 + height:100%) estica cards pequenos — não reaproveitar em grid/linha
module: ui
metadata:
  type: project
  pin: true
---

**Why:** `.panel` do `App.css` foi desenhada para o contêiner principal de um módulo:

```css
.panel { display:flex; flex-direction:column; flex:1; min-height:0;
         height:100%; padding:18px 20px; overflow:hidden; }
```

Reaproveitá-la em **cartão de estatística** ou em **linha de lista** faz `flex:1` + `height:100%` esticarem o elemento até a altura do contêiner, produzindo exatamente o defeito reportado: "fields retangular vazio dentro" — bordas e fundo de um painel grande com um número minúsculo no canto.

**Como aplicar:**
- `.panel` serve ao **contêiner** (módulo, painel principal), nunca a item dentro de grid ou de lista rolável.
- Cartão/linha pequenos: definir border/border-radius/background **no próprio prefixo** do módulo (ex.: `.sh-stat`, `.sh-item`) em vez de reaproveitar `.panel`.
- Em `.sh-list` (coluna com `flex:1` + `overflow-y:auto`), cada `.sh-item` precisa de `flex-shrink:0` e **sem** `height:100%` — quem rola é a lista, não a linha.
- Diagnóstico rápido de "retângulo vazio": procurar no JSX `className="<prefixo> panel"` e conferir se o elemento tem `flex:1`/`height:100%` na CSS.
- Comparar com `.startup-stat-card` (App.css), que já é o padrão correto do projeto para stat card: `display:flex; flex-direction:column; gap:4px; padding:12px 14px; border; border-radius:4px;` — sem flex/height.