---
name: winget-tabela-localizada-byte-vs-char
description: Parse da tabela do winget deve cortar colunas por CARACTER, nunca por byte — acentos no cabeçalho deslocam tudo
module: seguranca
metadata:
  type: project
  pin: true
---

**Why:** `winget list --upgrade-available` devolve tabela com colunas de largura variável, **localizada** (pt-BR: `Nome/ID/Versão/Disponível/Origem`). As linhas de dados são ASCII puro, mas o cabeçalho pt-BR tem acentos (`Versão`, `Disponível` — 2 bytes por letra). Calcular os limites de coluna em BYTE a partir do cabeçalho e aplicá-los nas linhas de dados desloca as colunas 3, 4 e 5: a versão atual vira `2024.06-1                     2` e o nome ganha resíduo da coluna seguinte. Numa simulação com a saída real, isso produziu `2024.06-1                     2` / `026.07-1                     wi` / `nget` em 100% das 57 linhas.

**How to apply:**
- Descobrir as colunas pelos **tokens do cabeçalho** (funciona em qualquer idioma), mas contar em `char_indices`, não em bytes.
- Validar a última coluna contra origens conhecidas (`winget`, `msstore`) — é o que filtra o rodapé (`57 atualizações disponíveis.`) e o aviso de versão indeterminada sem depender de idioma.
- Exigir que a coluna ID contenha `.` e nenhum espaço (IDs são sempre `Publicador.Pacote`).
- Fixtures de teste alinhadas à mão erram: gerar as colunas com `max(len(header), len(célula))` e colar a saída real capturada. Um fixture desalinhado "passa" no parser errado e falha no certo — foi exatamente o que aconteceu.
- `winget` desta máquina (1.29.380) **não aceita `--output json`** em `list` nem em `upgrade`; só a tabela texto. `list --upgrade-available` é o comando correto (~5 s).
- Rodar winget com `hidden_command` + `--disable-interactivity`; cache de 30 min (consome rede) e nunca no ciclo de polling da Visão geral.