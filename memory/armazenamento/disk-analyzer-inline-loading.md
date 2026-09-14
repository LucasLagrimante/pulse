# Carregamento local do analisador de disco

- Ao montar `DiskAnalyzer`, mantenha um estado explícito enquanto `get_disk_scan_state`, `get_cached_disk_scan` e `get_system_drives` restauram a última análise e as unidades.
- Mostre o feedback dentro dos painéis de árvore e treemap; uma busca automática em segundo plano não deve ocupar o toast global.
- Preserve o estado vazio somente depois que a tentativa de restauração terminar.
- A entrada automática no módulo pode carregar arquivos recentes silenciosamente; reserve o toast para uma atualização solicitada pelo usuário.
- Animações de loading devem respeitar `prefers-reduced-motion`.
