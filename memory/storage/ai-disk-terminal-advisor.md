---
name: ai-disk-terminal-advisor
description: Integração entre varredura de disco e CLIs de IA locais para diagnóstico e liberação de espaço
module: storage
metadata:
  type: project
---
**Why:**
Ao passar contexto de disco completo para CLIs de IA via terminal, o limite de argumentos do shell do Windows (`cmd.exe`/`PowerShell`) causa overflow se a árvore for serializada no comando.

**How to apply:**
Gere um relatório estruturado em Markdown (`%TEMP%\pulse-disk-analysis-*.md`) com o detalhamento das maiores pastas, maiores arquivos e candidatos a limpeza. No comando de lançamento do terminal (`claude`, `agy`, `codex`), passe um pré-prompt focado com o objetivo de liberação de espaço, o resumo do top 5 e o caminho do arquivo de contexto salvo, suportando execução transparente em Windows nativo ou WSL.
Plano da IA: o pré-prompt padrão pede gravar `pulse-disk-cleanup-plan-<UNIDADE>.md` ao lado do contexto (no WSL, caminhos como `/mnt/<letra>/...` via `to_wsl_path`). O modal checa a cada 4 s (`get_ai_cleanup_plan_status`) e mostra o card ABRIR PLANO; `open_ai_cleanup_plan` deriva o caminho do scan em cache, nunca recebe da UI. Só um botão "ANALISAR COM … NO TERMINAL" (barra de cache).
