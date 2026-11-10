---
name: tray-flyout-skeleton-loading
description: O tray flyout usa skeletons nativos nas dimensões finais sem banners intrusivos, evitando overflow e duplo indicador.
module: desktop
metadata:
  type: project
---

**Why:**
Banners de loading no tray adicionavam altura vertical e empurravam o layout além dos 740px da janela fixa, duplicando o logotipo Pulse. Além disso, spinners em ::after desalinhava o logo e mensagens em colunas de 28px de CSS Grid quebravam textos.

**How to apply:**
No tray flyout, use o PulseMark com animação nativa no próprio logo, uma tag CONECTANDO inline, cards de telemetria com skeletons e itens de processo com skeletons e classe com grid-column 1 / -1, garantindo zero layout shift e zero overflow. Cards condicionais (GPU) entram como skeleton enquanto `overview == null` — o grid nasce 2×2 e só colapsa o Disco para `1 / -1` após a coleta confirmar ausência.
