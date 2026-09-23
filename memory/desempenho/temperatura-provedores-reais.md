---
name: temperatura-provedores-reais
description: Temperatura real via nvidia-smi e zonas térmicas ACPI porque Win32_VideoController nunca expõe sensor térmico
module: desempenho
metadata:
  type: project
---
**Why:** `Win32_VideoController` não tem campo de temperatura em nenhum hardware; e sem driver kernel (WinRing0/LHM) não há API de pacote da CPU — só zonas ACPI genéricas, que não podem ser rotuladas de "CPU". E cada `powershell.exe` custa centenas de ms de cold start: 2 chamadas por coleta triplicavam o custo de abrir o flyout.
**How to apply:** Em `performance.rs`, 1 snapshot PS por coleta (`run_wmi_snapshot`: gpu+zonas+LHM+OHM+`Win32_TemperatureProbe`, esta última de graça no mesmo snapshot) + `nvidia-smi`; exponha `thermal_sensors[]` (`temperature_celsius` = maior valor, compat). Tooltip (`tray.rs`) prefere o par CPU+GPU, fallback 2 mais quentes; thread 60 s com sleep antes da 1ª leitura (sem custo no boot) + carona no comando (`AppHandle`, zero spawn extra). Flyout: 1ª térmica adiada 1,2 s (`THERMAL_INITIAL_DELAY_MS`, caminho crítico só overview+áudio), depois 15 s. Nunca card extra de temperatura: Desempenho CPU no `small` do card CPU, GPU+VRAM no card GPU; flyout temp inline na CPU, 2ª linha `1fr 1fr` com disco (`%` no head) e GPU (`MiniUsageBar`; trilho `is-unavailable` hachurado sem leitura).
