# Cache local da última análise de disco

- A última árvore completa é persistida em `%LOCALAPPDATA%\Pulse\last-disk-scan.json` ao final da varredura, junto do horário de conclusão e da duração.
- A restauração na abertura é explicitamente identificada como `RESULTADO EM CACHE`; o tempo relativo é atualizado a cada minuto e a data completa fica visível.
- Não persistir resultados parciais, cancelados ou com falha. Uma falha ao salvar o cache não deve esconder o resultado recém-concluído.
