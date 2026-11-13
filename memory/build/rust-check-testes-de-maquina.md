---
name: rust-check-testes-de-maquina
description: Testes Rust que leem o Windows real falham por estado da máquina e a suíte paralela cai com ACCESS_VIOLATION; o gate é scripts/rust-check.sh (em série) e o CI não roda cargo test
module: build
metadata:
  type: project
---
`services::tests::real_enumeration_reads_start_name_and_classifies` exige que 90% dos serviços tenham `StartName`. Em 2026-11-13 esta máquina tinha 281 de 313 (89,8%), então o teste falhou sem nenhuma mudança em `services.rs`.
**Why:** o teste lê os serviços instalados de verdade. Um serviço novo sem conta (driver, por exemplo) muda o resultado. O `release.yml` só roda `typecheck` + `npm test`, então quem garante o Rust é o gate local.
**How to apply:** rode `bash scripts/rust-check.sh`, que sincroniza `src-tauri/` com `%LOCALAPPDATA%\pulse-build\src` via rsync e roda fmt (só nos `.rs` alterados, porque o repo já tem divergência antiga de formatação), clippy `-D warnings` e `cargo test`. Essa falha específica não bloqueia o release, mas precisa constar no relatório. Qualquer outra falha bloqueia.
**Crash em paralelo:** `cargo test --lib` sem `--test-threads=1` cai de forma intermitente com `STATUS_ACCESS_VIOLATION` (0xc0000005). Em 2026-11-13 isso acontecia em 3 de 5 rodadas no código da v1.5.0; em série, 4 de 4 rodadas passaram. Provável FFI/COM não seguro entre threads em algum teste que usa o Windows real. A causa ainda não foi investigada: pode ser um bug real do app, não só dos testes.
