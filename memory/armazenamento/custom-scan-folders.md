---
name: custom-scan-folders
description: Suporte e persistência de pastas personalizadas em outros discos no módulo de armazenamento
module: armazenamento
metadata:
  type: project
---
**Why:** O escopo padrão cobria apenas pastas de usuário no `C:`. Além disso, pastas customizadas no final da lista ou orçamento global único de 1.5s faziam com que diretórios padrão pesados consumissem o prazo antes de alcançar unidades secundárias (`D:\`).
**How to apply:** Em `recent_files.rs`, priorize pastas customizadas no início de `roots()` antes das padrão e aloque `ScanBudget` (1.5s e limites de I/O) individualmente por raiz em `scan_all()`, evitando descarte precoce e garantindo a inclusão de discos externos.
