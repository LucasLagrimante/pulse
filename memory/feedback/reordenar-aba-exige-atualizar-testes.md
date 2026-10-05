---
name: reordenar-aba-exige-atualizar-testes-que-esperam-a-anterior
description: Trocar a aba padrão quebra helpers de teste que clicavam para chegar nela — corrijam o helper, não assinem teste por teste
metadata:
  type: feedback
---

**Why:** Trocar a aba padrão do `ServicesHub` (Serviços → Atualizações) quebrou 14 de 18 testes de uma vez, por dois motivos encadeados:

1. Os testes da aba que deixou de ser padrão faziam `render()` e já seguiam — sem helper de troca.
2. O helper da aba antiga (`openUpdatesTab`) fazia `waitFor(getByText("Audio do Windows"))` **antes** de clicar na aba — ou seja, esperava o conteúdo da aba *contrária* para depois trocar. Com a inversão, essa espera nunca resolvia e derrubava também os testes que estavam corretos.

**Como aplicar:**
- Inverter aba padrão é mudança de contrato dos testes, não ajuste de um teste. Ao fazê-lo, grep pelos helpers `open*Tab`/`abrir*Aba` e corrija **o helper** (remova a espera pela aba antiga e troque por `aria-selected` da aba nova).
- Prefira asserção de estado de aba (`getByRole("tab", {name})` com `aria-selected`) a asserção de conteúdo alheio. `aria-selected` é o contrato do componente; "apareceu o texto X" é acoplamento frágil de timing.
- Ao usar script Python para inserir uma chamada helper em vários testes, delimite o bloco por `describe(...)` de verdade. Um detector mal delimitado vazou `await abrirAbaServicos()` para dentro do `describe("Aba de atualizações")` e causou 6 falhas opostas — cada inserção em bloco alheio precisa ser desfeita na mesma passada.
- Vale registrar teste novo fixando a **ordem** das abas (`getAllByRole("tab")[0]`), senão a próxima inversão volta silenciosa.
- Relacionado: `memory/ui/classe-panel-estica-cartoes-pequenos.md` — o mesmo arquivo tem armadilha estrutural de CSS; ao caprichar no visual, não reintroduzir `panel` em item de grid.