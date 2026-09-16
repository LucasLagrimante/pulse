---
name: tauri-layout-preview-in-browser
description: Validar layout do Pulse sem o Tauri mockando window.__TAURI_INTERNALS__ num HTML de preview
module: tooling
metadata:
  type: project
---
**Why:** Ver mudanças de UI exige rodar o app Tauri no Windows; no WSL dá para validar layout/tema rapidamente no Chrome.

**How to apply:**
- Criar um HTML de preview **com nome próprio** (ex.: `preview-desempenho.html`) na raiz: outra sessão/agente pode sobrescrever `preview-mock.html`.
- O mock precisa de `metadata: { currentWindow, currentWebview }`, `transformCallback`, `unregisterCallback`, `runCallback` e `convertFileSrc`, além de `invoke` por comando (`plugin:event|listen` → 1).
- Servir com `npm run dev` e capturar com `google-chrome --headless=new --virtual-time-budget=<ms> --screenshot=...` ou via CDP (`--remote-debugging-port`) para medir estilos computados.
- Remover os artefatos ao terminar; conferir `card.scrollHeight` contra `clientHeight` para detectar sobreposição de cards.
