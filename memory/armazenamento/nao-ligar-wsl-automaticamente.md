---
name: nao-ligar-wsl-automaticamente
description: Detecção automática (sem clique do usuário) nunca pode ligar a VM do WSL; consultar só distros em execução e cachear em disco
module: armazenamento
metadata:
  type: feedback
---
**Why:** Em 2026-11-04 o botão do assistente de IA do Armazenamento passou a detectar as CLIs (agy/claude/codex) ao abrir a tela. `wsl -d <distro> -e sh` liga a VM (`vmmem`, segundos e centenas de MB). O usuário pediu: o Pulse não ativa o WSL sozinho, para economizar recurso.
**How to apply:**
- Detecção automática/prévia: `WslProbe::RunningOnly` (`wsl -l --running -q`, cruzado com `wsl -l -q`). Só ação explícita (ex.: "Abrir terminal") usa `AllowStart`.
- Resultado negativo sem consultar WSL desligado não prova ausência: não remover do cache.
- Cache persistente em `%LOCALAPPDATA%\Pulse\ai-cli-providers.json` (TTL 24 h), corrigido quando o modal detecta diferente.
