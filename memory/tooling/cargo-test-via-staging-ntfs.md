---
name: cargo-test-via-staging-ntfs
description: Rodar cargo test/clippy do WSL copiando src-tauri e dist para o staging NTFS e chamando um .bat com vcvars
module: tooling
metadata:
  type: project
---
**Why:** Não há cargo no WSL; testes Rust só rodam com o toolchain MSVC do Windows.
**How to apply:** `npm run build` (generate_context precisa de dist), copiar `src-tauri` (sem target) e `dist` para `/mnt/c/Users/lucas/AppData/Local/pulse-build/src`, gravar um `.bat` com vcvars64 + `CARGO_TARGET_DIR=...\pulse-build\target` + `cargo clippy --all-targets -- -D warnings` / `cargo test --lib`, rodar `cmd.exe /c x.bat < /dev/null` e apagar o .bat. Os 2 testes `services::real_enumeration_*` (WMI real) falham nesse contexto e a suíte paralela às vezes cai com ACCESS_VIOLATION (COM); use `--test-threads=1` para separar.
