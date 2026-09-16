---
name: pkill-f-mata-o-proprio-shell
description: pkill -f cujo padrão aparece na própria linha de comando mata a sessão do shell e trava o comando
module: tooling
metadata:
  type: project
  pin: true
---
**Why:** `pkill -9 -f "vite.js --port 5199"` casa também com o `zsh -c '... pkill -9 -f "vite.js --port 5199" ...'` em execução, matando a si mesmo. O sintoma é o comando "pendurar" até o timeout, repetidamente, com saída vazia.

**How to apply:**
- Sempre escapar um caractere do alvo: `pkill -9 -f "vite[.]js --port 5199"`.
- Ou indireto: `P="5199"; pkill -f "debugging-port=$P"` (a linha de comando contém `$P`, não o valor).
- Vale para qualquer `pkill`/`pgrep -f` com padrão citado literalmente: chrome `--remote-debugging-port`, `npm run dev`, etc.
