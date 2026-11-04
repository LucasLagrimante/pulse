---
name: ai-cli-prompt-flags
description: Cada CLI de IA recebe o prompt inicial de um jeito; agy exige -i e encerra com prompt posicional (console abre e fecha)
module: armazenamento
metadata:
  type: project
---
**Why:** Em 2026-11-04 o botão do assistente de IA do Armazenamento abria e fechava o console para `agy`: ele recusa argumento posicional (`Error: unexpected argument ... Prompts are read only from -p/--print, -i/--prompt-interactive, or stdin`) e sai na hora. `claude` e `codex` aceitam prompt posicional interativo.
**How to apply:**
- Flags por CLI ficam em `cli_prompt_flags()` (`ai_disk_advisor.rs`): `agy` → `-i <prompt>` (nunca `-p`, que é não interativo), valendo para Windows nativo e WSL.
- Ao adicionar um provedor, testar `<cli> "teste"` antes: se encerrar sem sessão interativa, mapear a flag correta.
