---
name: tmux-escape-time-osc-garbage
description: tmux 3.6 com escape-time baixo vaza respostas OSC de cor como texto no input do opencode
module: tooling
metadata:
  type: project
---

**Why:** tmux 3.6+ não estende o timer para respostas OSC 4/10/11 (`rgb:RRRR/GGGG/BBBB`) do Windows Terminal via WSL; com `escape-time 10` os fragmentos chegam após o timeout e o tmux os repassa ao pane, e o parser do opencode insere como texto (ex.: `4c4c/4c4cd1d1/8b8bf5f5/...`).

**How to apply:** Manter `set -sg escape-time 500` em `~/.tmux.conf` (mudança ao vivo via `tmux set -sg` já vale; reiniciar serve para limpar input sujo). tmux >= 3.7 corrige na origem e permite voltar a 10.
