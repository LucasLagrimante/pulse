---
name: clippy-manual-as-chunks
description: Clippy recente recusa chunks_exact com tamanho constante (-D warnings); usar as_chunks::<N>()
module: tooling
metadata:
  type: project
---
**Why:** Após atualizar o toolchain, `cargo clippy --all-targets -- -D warnings` falhou em 8 pontos pré-existentes (`ai_disk_advisor`, `app_icons`, `process_identity`, `subscription_usage`, `tray`) com "using `chunks_exact` with a constant chunk size".
**How to apply:** Trocar `x.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]]))` por `x.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p))` (e `as_chunks_mut` para escrita). Não usar `cargo fmt --check` como gate: o crate não está todo formatado; o `rust-check.sh` checa só os `.rs` alterados (ver [[rustfmt-arquivo-legado-alterado]]).
