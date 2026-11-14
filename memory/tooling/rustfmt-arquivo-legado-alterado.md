---
name: rustfmt-arquivo-legado-alterado
description: rust-check roda rustfmt --check em todo .rs alterado; mexer num arquivo legado não formatado obriga formatá-lo inteiro (rustfmt do Windows via stdin)
module: tooling
metadata:
  type: project
---
**Why:** em 2026-11-14 trocar só strings de fixture em `services.rs` e `startup_inventory.rs` fez `bash scripts/rust-check.sh` falhar com "fora do rustfmt": os arquivos nunca estiveram formatados, e o gate checa qualquer `.rs` alterado vs HEAD.
**How to apply:** não existe rustfmt no WSL. Formate pelo Windows no espelho NTFS: `(cd "$STAGING/src-tauri" && cmd.exe /c 'set PATH=%USERPROFILE%\.cargo\bin;%PATH%&& rustfmt --edition 2021' < src-tauri/src/X.rs > /scratch/X.rs)`, confira a saída (sem CRLF) e copie de volta. Avise no resumo: o diff ganha reformatação além da mudança real.
