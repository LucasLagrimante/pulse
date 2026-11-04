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
