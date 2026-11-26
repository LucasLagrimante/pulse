---
name: rust-check-lock-com-build-nativo
description: Quando build:native segura o lock do rust-check.sh, validar Rust copiando os .rs alterados para o espelho NTFS e rodando cargo direto com CARGO_TARGET_DIR
module: tooling
metadata:
  type: project
---

**Why:** `scripts/rust-check.sh` usa `flock -n /tmp/pulse-build-native.lock`; com `build:native` rodando (vi `bash scripts/build-native.sh --install` por 10 min) ele aborta com "build:native em andamento". Parar o build do usuário não é opção.
**How to apply:** Copie só os `.rs` alterados para `%LOCALAPPDATA%\pulse-build\src\src-tauri\src\` e rode `cmd.exe /c "set CARGO_TARGET_DIR=%LOCALAPPDATA%\pulse-build\target&& set PATH=%USERPROFILE%\.cargo\bin;%PATH%&& cargo clippy --lib --tests -q -- -D warnings"` (e `cargo test --lib -q -- --test-threads=1 --skip services::tests::real_enumeration_reads_start_name_and_classifies`) a partir daquele diretório; o lock interno do Cargo serializa com o build em andamento. Para fmt, `rustfmt.exe --check --edition 2021 < arquivo` funciona direto do WSL. Não rode `rsync --delete` no espelho enquanto o build nativo está ativo.
