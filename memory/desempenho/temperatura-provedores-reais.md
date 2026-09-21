---
name: temperatura-provedores-reais
description: Temperatura real via nvidia-smi e zonas térmicas ACPI porque Win32_VideoController nunca expõe sensor térmico
module: desempenho
metadata:
  type: project
---
**Why:** `Win32_VideoController` não tem campo de temperatura em nenhum hardware, então `temperature_celsius` era sempre `None` por construção — não era bug de permissão ou parsing.
**How to apply:** Em `performance.rs`, tente nesta ordem: `nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits` (maior valor 0–150 °C) e depois `MSAcpi_ThermalZoneTemperature` em `root/wmi` (°C = `CurrentTemperature`/10 − 273.15, só zonas ativas, faixa bruta 2732–4232). Sem sensor, retorne `Capability{None}` com `unavailableReason` honesto; o frontend já exibe a seção só quando há valor.
