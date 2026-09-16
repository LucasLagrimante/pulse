---
name: hooks-antes-do-early-return-do-tray
description: No App.tsx, todo hook deve ficar antes do early return do tray-flyout para não quebrar as regras dos hooks
module: ui
metadata:
  type: project
---
**Why:** O componente `App` tem um `return <TrayFlyout />` condicional (`windowLabel === "tray-flyout"`). Ao adicionar um hook (ex.: `useMemo`) depois desse return, a contagem de hooks muda ao alternar entre a janela principal e o flyout e o React lança "Rendered fewer hooks than expected" e desmonta a árvore.
**How to apply:** Declare `useState`/`useMemo`/`useEffect` sempre acima do comentário `// Se esta janela for o painel flutuante do tray icon`; cálculos derivados e handlers (`const`, funções) podem ficar abaixo. Ao investigar telas em branco nos testes do tray-flyout, suspeite primeiro de hook após esse early return.
