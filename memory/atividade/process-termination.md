# Encerramento de processos na Atividade

- A ação pertence a cada processo, tanto no módulo Atividade quanto no painel rápido do tray.
- Exigir seleção e confirmação explícita antes de chamar `terminate_process`; avisar sobre trabalho não salvo.
- O backend aceita somente PIDs presentes na coleta atual, bloqueia PID zero e o próprio processo do Pulse, e deixa o Windows recusar processos protegidos.
- Para aplicativos comuns, tentar primeiro o encerramento normal e repetir com `taskkill /F` somente após falha e confirmação explícita do usuário.
- `vmmemWSL` não deve ser morto por PID: usar `wsl.exe --shutdown` e avisar que isso encerra todas as distribuições e processos Linux em execução.
- Após sucesso, remover a linha imediatamente e invalidar/recarregar a coleta para evitar estado visual obsoleto.
- Manter o ranking da Atividade em uma única coluna para preservar a ordem de leitura.
