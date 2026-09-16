# Cache local da última análise de disco

- A última árvore é persistida em `%LOCALAPPDATA%\Pulse\last-disk-scan.json` ao final da varredura, junto do horário de conclusão e da duração. Desde a cota global, a árvore é podada para `MAX_TREE_NODES` (somas reais em nós agregados) e a escrita é atômica (`write` + `rename`); caches >32 MB de versões antigas são ignorados.
- A restauração na abertura é explicitamente identificada como `RESULTADO EM CACHE`; o tempo relativo é atualizado a cada minuto e a data completa fica visível.
- Não persistir resultados parciais, cancelados ou com falha. Uma falha ao salvar o cache não deve esconder o resultado recém-concluído.
