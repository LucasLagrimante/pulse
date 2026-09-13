# Varredura completa e responsiva do analisador de disco

- Limites globais de 2.500 itens ou 1,5 s tornam o Tree Size inutilizável: discos grandes sempre terminam quase imediatamente com uma amostra parcial.
- Ao solicitar `ESCANEAR DISCO`, percorra toda a árvore acessível. Proteja a responsividade com `spawn_blocking`, exclusão de symlinks/junctions, `yield_now` a cada 15 itens, throttling curto a cada 250 itens, progresso e cancelamento explícito — nunca com truncamento automático.
- A árvore serializada agora desce todos os níveis encontrados (com teto técnico de 64 níveis), para que o usuário possa localizar as pastas maiores; a proteção de volume continua sendo a agregação de excesso de filhos por pasta.
- O streaming precisa propagar o ramo ativo até a raiz; publicar somente quando um filho direto da unidade termina deixa árvore e treemap vazios durante diretórios grandes. Envie snapshots parciais ordenados, limitados a 40 filhos por pasta e com cadência mínima de 120 ms para não saturar o IPC.
- No React, reconcilie cada snapshot pelo `id` dos nós para preservar foco e expansão durante a atualização, em vez de reinicializar a navegação a cada evento.
- Progresso textual e snapshots da árvore devem ter relógios de emissão separados. Compartilhar `last_emit` faz eventos leves de 120 ms reiniciarem continuamente o intervalo do snapshot e pode impedir qualquer resultado parcial de chegar à interface.
- Ao remontar Armazenamento durante uma varredura, restaure diretamente `get_disk_scan_state` e não leia o cache final anterior. No scanner, mantenha filhos compactos separados dos nós completos para que cada snapshot não clone novamente todos os ramos já concluídos.
