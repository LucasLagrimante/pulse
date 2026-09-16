---
name: keyring-windows-target-user-first
description: keyring 4.x no Windows grava/lê target "{account}.{service}"; entradas criadas com outro formato não são enxergadas pelo Pulse
module: telemetria
metadata:
  type: project
---

**Why:**
`keyring::Entry::new("Pulse", "opencode-go-api-key")` gera o target `opencode-go-api-key.Pulse` (windows-native-keyring-store põe o user antes do service, divisor `.`). Uma credencial manual `Target: Pulse` / `User: opencode-go-api-key` não era lida nem removida pelo app, mantendo o provedor "não configurado" e invisível.

**How to apply:**
1. Targets do Pulse no Credential Manager: `openrouter-api-key.Pulse` e `opencode-go-api-key.Pulse` (inspecionar com `cmd.exe /c cmdkey /list`).
2. Ao debugar "chave configurada mas indisponível", comparar o target gravado com `{account}.{service}` antes de suspeitar da API.
3. Para resetar, `cmdkey /delete:<target>`; o app só enxerga o target que ele mesmo gerencia.
