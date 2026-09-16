---
name: lto-fat-link-e-crate-types
description: Passo final lento do tauri build release é o link LTO fat (agravado por crate-type staticlib/cdylib), não o bundler NSIS
module: build
metadata:
  type: project
---

**Why:** Medido no build de 2026-09-16 (Ryzen 9 5900X): o "final" do `tauri build --bundles nsis` (~3 min) é o LTO do crate `pulse-windows` — 95 s para gerar rlib/.dll/.lib e 84 s para linkar o .exe. O makensis/NSIS leva só ~3 s (exe de 9,3 MB → setup de 3,3 MB), então não é o gargalo.

**How to apply:**
- Se o build parecer travado nas últimas linhas (`Compiling pulse-windows` / `Bundling`), é o link LTO fat, não a compactação NSIS.
- `crate-type = ["staticlib", "cdylib", "rlib"]` faz o cargo gerar 3 saídas LTO no release (rlib 24 MB, .dll 2,85 MB, .lib 119 MB); no desktop Windows só o `rlib` é usado — corte para `["rlib"]`.
- `lto = true` + `codegen-units = 1` maximiza tempo de link; `lto = "thin"` com CGU padrão acelera sem perda relevante em app de telemetria.
- Defender com RTP ativo também pesa no link; exclusão da pasta `target` ajuda (requer admin).
