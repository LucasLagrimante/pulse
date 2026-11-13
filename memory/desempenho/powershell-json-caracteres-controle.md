---
name: powershell-json-caracteres-controle
description: ConvertTo-Json do PowerShell 5.1 emite caracteres de controle crus (ex. BEL no FileDescription do Battle.net) e o serde rejeita o JSON inteiro — sanitizar antes do parse
module: desempenho
metadata:
  type: project
---

Em 2026-11-13 a tela mostrava "Telemetria indisponível." porque todo `system_overview` falhava. O `FileDescription` do Battle.net contém BEL (0x07) cru, e o `serde_json::from_str` recusou o payload inteiro. Sem cache válido, toda coleta seguinte falhava também.
**Why:** o Windows PowerShell 5.1 não escapa todos os caracteres de controle no `ConvertTo-Json`, e o audit só registra "Falha na coleta", sem mostrar a causa.
**How to apply:** todo JSON vindo de PowerShell passa por sanitização (`c.is_control()` → espaço) antes do serde, como em `parse_overview`. Para diagnosticar, extraia o script, rode com `powershell.exe -File` e valide a saída com `JSON.parse`. Para testar o Rust, use `bash scripts/rust-check.sh`, e para ler os logs do app instalado, a skill `debug-local`.
