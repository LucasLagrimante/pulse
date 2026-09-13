---
name: windows-startup-approved-and-impact
description: Gerenciamento seguro de status ativado/desativado e identificação dos mais lentos na inicialização do Windows
module: inicializacao
metadata:
  type: project
---
**Why:** No Windows, desativar itens de boot pelo Gerenciador de Tarefas não apaga a chave `Run`; grava um registro binário em `StartupApproved\Run` (byte inicial 0x03=desativado, 0x02=ativado). Apagar chaves destrói o comando e impede reativação, e ignorar `StartupApproved` faz todos os itens parecerem ativados.
**How to apply:** Nunca use `reg delete` para desativar itens de boot. Atualize `StartupApproved\Run` com `03...` (desativar) ou `02...` (ativar) para preservar o comando e sincronizar com o Windows. Para identificar os mais lentos, colete eventos de degradação em `Diagnostics-Performance/Operational` (IDs 101/102) e ordene decrescentemente pelo tempo de boot e nível de impacto.
