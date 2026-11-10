---
name: tamanho-real-vs-estimatedsize
description: EstimatedSize do registro é pouco confiável; a Remoção mede programa (declarado ou deduzido) + dados à parte (footprint.rs) e só cai na estimativa como fallback rotulado
module: desinstalar
metadata:
  type: project
---
**Why:** Medição real (2026-11-10, 172 apps): registro errava muito (SignalRgb 308 MB vs 1,9 GB; Postman 165 MB vs 1 GB); 86 apps sem `InstallLocation` (6 entre aspas). Deduzindo a pasta, a cobertura subiu de 86 para 115 (DaVinci Resolve 4,1 GB em `Program Files\Blackmagic Design\…`); 43 apps têm dados em AppData/ProgramData.
**How to apply:** Pasta deduzida = exe do `DisplayIcon` (`program_dir_from_exe`: recusa unins*, Package Cache, InstallShield, Installer, Common Files, `{GUID}`) → `named_folder_candidates` `is_program` → `Program Files\<Fabricante>\<nome>`; sempre selo "deduzido". Dados: mesma regra da desinstalação (`residue::named_folder_candidates`, nunca divergir), somados à parte ("+ X de dados"); ordenar por programa+dados.
Rápido: lotes ≤24, 60k entradas/1,5 s por pasta, 4 s/lote, "Medição adiada" reenfileira; ~4,5 s total, 17 ms com cache. Completo (`measure_installed_app_size_full`): 1 por vez (AtomicBool), 3M entradas/120 s; cache nunca rebaixa completo para parcial. Testar na máquina real: copiar `src-tauri` para `%LOCALAPPDATA%\Temp\pulse-src` e rodar cargo via `cmd.exe` + vcvars; se der `bindgen.rs` ausente, apagar `pulse-target\debug\build\libsqlite3-sys-*`.
