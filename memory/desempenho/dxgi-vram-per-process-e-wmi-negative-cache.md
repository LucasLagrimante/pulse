---
name: DXGI VRAM per-process e cache negativo de subprocessos WMI/nvidia-smi
description: IDXGIAdapter3::QueryVideoMemoryInfo mede apenas o processo atual; nunca spawnar PowerShell/nvidia-smi em loop curto quando o provedor não existe
module: desempenho
metadata:
  type: project
---

**Why:** `IDXGIAdapter3::QueryVideoMemoryInfo().CurrentUsage` retorna só a VRAM do próprio processo do Pulse (~20 MB) e sobrescrevia a VRAM global real da GPU; além disso, rodar `powershell.exe` a cada 6s sem LibreHardwareMonitor/OpenHardwareMonitor queimava CPU em repouso.
**How to apply:** Em `src-tauri/src/hardware_telemetry/windows.rs`, use DXGI apenas para `vram_total_gb`/nome/LUID e leia VRAM usada global via `nvidia-smi` ou PDH `\GPU Adapter Memory(*)\Dedicated Usage`. Rode metadados WMI estáticos 1× na sessão (`OnceLock`), aplique TTL negativo de 60s para WMI externo ausente e desative `nvidia-smi` na sessão ao receber `ErrorKind::NotFound`.
