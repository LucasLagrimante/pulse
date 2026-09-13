---
name: disk-analyzer-tree-size
description: Padrão para varredura e navegação de disco estilo TreeSize com diferenciação de arquivos e pastas pesadas
module: armazenamento
metadata:
  type: project
---
**Why:** Varreduras de disco completas produzem árvores volumosas que exigem agregação de nós menores para evitar sobrecarga de IPC e DOM, além de diferenciação clara entre arquivos gigantes e diretórios.
**How to apply:** No Rust, ordene filhos por tamanho decrescente e agrupe itens secundários além do limite em nós sintéticos. Na interface, separe filtros de pastas e maiores arquivos, omita o botão de focar no próprio nó raiz ativo e utilize ResizeObserver em vez de setInterval para cálculo dimensional do treemap.
