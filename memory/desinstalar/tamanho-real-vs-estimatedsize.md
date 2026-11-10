---
name: tamanho-real-vs-estimatedsize
description: EstimatedSize do registro é pouco confiável; a Remoção mede a pasta de instalação (footprint.rs) e só cai na estimativa como fallback rotulado
module: desinstalar
metadata:
  type: project
---
**Why:** Medição real (2026-11-10, 172 apps): registro errava muito (SignalRgb 308 MB vs 1,9 GB medido; Postman 165 MB vs 1 GB); ~metade dos apps não informa `InstallLocation`, e 6 o gravam **entre aspas** (pareciam caminho relativo e eram vetados).
**How to apply:** `measure_installed_app_sizes` (lotes ≤24, 60k entradas/1,5 s por app, 4 s por lote, `yield_now` a cada 15, cache 10 min, veto `folder_deletion_veto`) — custo real ~1,6–7 s em segundo plano, 18 ms com cache. Sempre `clean_install_location` (tira aspas). UI: selo medido/parcial(≥)/estimado; sem dado = "Indisponível" e vai para o fim da ordenação. Teste em máquina real: copiar o crate para `%LOCALAPPDATA%\Temp\pulse-src` e rodar `cargo test` via `cmd.exe` + vcvars.
