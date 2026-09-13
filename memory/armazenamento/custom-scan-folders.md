---
name: custom-scan-folders
description: Suporte e persistência de pastas personalizadas em outros discos no módulo de armazenamento
module: armazenamento
metadata:
  type: project
---
**Why:** O escopo padrão de varredura cobria apenas o `USERPROFILE` do disco `C:`. Sem incluir pastas personalizadas em `roots()`, caminhos em outros discos (ex: `D:\`) são ignorados e rejeitados na validação de segurança do backend.
**How to apply:** Persista as pastas em `%LOCALAPPDATA%\Pulse\custom-scan-folders.json` e mescle-as em `roots()` no Rust para alimentar tanto `scan_all()` quanto `validated()`. Na interface, disponibilize barra de adição e remoção reativa que invalida o cache do backend (`CACHE`) e recarrega os arquivos.
