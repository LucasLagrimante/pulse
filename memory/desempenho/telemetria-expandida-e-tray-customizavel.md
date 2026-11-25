---
name: telemetria-expandida-e-tray-customizavel
description: Telemetria expandida 100% user-mode (DXGI, PDH inglês, HWiNFO Shared Memory, LHM/OHM e hwmon) com expansão em Desempenho (botão Voltar) e até 4 gráficos personalizáveis no Tray
module: desempenho
metadata:
  type: project
---

**Why:** O usuário pediu leitura estilo HWiNFO (CPU por núcleo/clock, VRAM, fans, temperaturas, disco I/O), expansão dos cards na aba Desempenho com botão Voltar + seletor de métricas, e até 4 gráficos personalizáveis no Tray Flyout — mantendo o Pulse 100% user-mode sem driver kernel (evita conflitos com antivírus e anti-cheats).
**How to apply:** Backend em `src-tauri/src/hardware_telemetry.rs` (`windows.rs`, `linux.rs`, `macos.rs`): no Windows usa DXGI (`IDXGIAdapter3::QueryVideoMemoryInfo` com 3 args), PDH em inglês (`PdhAddEnglishCounterW`), `Global\HWiNFO_SENS_SM2` via `OpenFileMappingW` e WMI completo de LHM/OHM; persiste `telemetry-preferences.json` (máx 4 `trayMetrics` + `cardMetrics`) emitindo `pulse://telemetry-preferences-updated`. No frontend, `src/telemetry.ts` unifica o catálogo e `src/PerformanceModule.tsx` renderiza a grade + drill-down com Voltar.
