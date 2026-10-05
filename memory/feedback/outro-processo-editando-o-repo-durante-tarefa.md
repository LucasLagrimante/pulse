---
name: outro-processo-editando-o-repo-durante-tarefa
description: Quando outro agente/edição mexe nos mesmos arquivos, `cargo test` falha por erros alheios — validar o módulo num harness isolado em vez de "consertar" o código do outro.
module: feedback
metadata:
  type: feedback
---

**Why:** Durante a classificação de serviços, `src-tauri/src/uninstall/` e `src/ServicesHub.tsx`
foram alterados por outro processo **enquanto eu trabalhava** (`ls --time-style=full-iso` mostrou
mtime mudando entre comandos; `cargo test` saiu de 24 testes passando para 36 erros de compilação
em arquivos que eu não tinha tocado). Editar ou "consertar" esses arquivos seria escrever sobre
o trabalho alheio em curso.

**How to apply:**
- Antes de acusar o próprio código, checar mtime dos arquivos e confirmar que nenhum erro
  `^error` referencia o arquivo que você editou: `cargo test 2>&1 | grep -c "seu_arquivo.rs"`.
- Para validar um módulo isolado sem tocar no resto: copiar o arquivo para um crate temporário em
  disco **Windows** (ex.: `%LOCALAPPDATA%\Temp\<harness>`), remover os `#[tauri::command]` que
  dependem de `tauri::async_runtime`, e criar stubs de `crate::process` / `crate::app_icons`.
  Deps: só `serde` + `dirs`. Rodar `cargo.exe` a partir do WSL funciona via
  `/mnt/c/Users/lucas/.cargo/bin/cargo.exe`.
  - Os testes `#[cfg(windows)]` **rodam de verdade** nesse harness — foi assim que medi a
    classificação contra os 313 serviços reais da máquina.
- Remover o harness ao terminar; ele é descartável, não um artefato do repo.
