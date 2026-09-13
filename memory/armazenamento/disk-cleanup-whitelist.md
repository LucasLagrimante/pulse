---
name: Limpeza de disco com lista branca
description: Mantém a limpeza local restrita a categorias conhecidas e selecionadas explicitamente pelo usuário
module: armazenamento
metadata:
  type: project
---

**Why:** Uma interface de limpeza não pode transformar caminhos arbitrários enviados pelo frontend em exclusões no sistema.

**How to apply:** Resolva no Rust os caminhos de cada categoria a partir de IDs estáveis, ignore links simbólicos e preserve itens bloqueados. A análise não remove nada; marque inicialmente apenas categorias recomendadas com espaço disponível e exija confirmação antes da limpeza permanente. Arquivos e pastas grandes devem ser apresentados como evidência da categoria, não como autorização implícita para apagar conteúdo pessoal.

**Follow-up:** A Lixeira usa `SHQueryRecycleBinW` e `SHEmptyRecycleBinW`, considera todas as unidades do usuário e nunca vem pré-selecionada. Caches reconstruíveis adicionais podem incluir Firefox, Otimização de Entrega e INetCache; não trate Prefetch nem downloads do Windows Update como limpeza recomendada. A grade de categorias deve ser o filho flexível com `min-height: 0` e `overflow-y: auto`, para que expandir “Maiores itens” role dentro do módulo sem deslocar a barra de comandos.
