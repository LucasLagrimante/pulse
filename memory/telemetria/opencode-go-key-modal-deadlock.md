---
name: opencode-go-key-modal-deadlock
description: Provedores com chave gerenciada pelo Pulse (OpenRouter/OpenCode Go) devem permanecer visíveis sem cota confirmada, senão o modal de chave fica inalcançável
module: telemetria
metadata:
  type: project
---

**Why:**
Os cards eram filtrados por `hasMeasuredQuota` e o modal de chave só abria pelo card. OpenCode Go sem chave (ou com chave recusada) sumia da lista e ficava sem qualquer ponto de entrada para `openModal("opencode-go")` — deadlock. O OpenRouter escapava por ter botão fixo na toolbar.

**How to apply:**
1. Renderizar provedores com chave gerenciada pelo Pulse (`keyManagedProvider`: openrouter, opencode-go) mesmo com `status === "unavailable"`, exibindo o empty state com botão `CONFIGURAR CHAVE …`.
2. Manter botão dedicado na toolbar para cada provedor gerenciado (`CHAVE OPENCODE GO` / `CONFIGURAR OPENCODE GO`), espelhando o OpenRouter.
3. Ao adicionar provedor por chave de API, revisar `displayProviders` e a toolbar para não repetir o deadlock.
