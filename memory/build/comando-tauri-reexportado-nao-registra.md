---
name: comando-tauri-reexportado-nao-registra
description: #[tauri::command] reexportado por pub use não é encontrado pelo generate_handler — registre pelo caminho do módulo de origem
module: build
metadata:
  type: project
---

**Why:** `#[tauri::command]` gera itens ocultos (`__cmd__*`, `__tauri_command_name_*`) no **módulo onde a fn está**, e `pub use` só reexporta o nome público da função. Com `uninstall::list_installed_apps` no handler, `generate_handler!` emitia 10× `E0433: cannot find __cmd__list_installed_apps in uninstall` — mesmo com a função corretamente anotada e reexportada.

**How to apply:**
- No `invoke_handler`, sempre usar o caminho completo até o arquivo de origem: `uninstall::installed::list_installed_apps`, `uninstall::session::run_app_uninstaller`.
- O `pub use` continua válido para a UI e para os testes; só o registro do handler exige o caminho de origem.
- Sintoma characteristico: erro só no `generate_handler`, com a mesma função existindo e testes unitários passando — impossível de achar por leitura, porque a anotação está certa.

Ver também `memory/build/windows-061-assinaturas-quebram-compilacao.md`.