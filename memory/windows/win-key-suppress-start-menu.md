---
name: win-key-suppress-start-menu
description: Runner na tecla Windows — reter o keydown SEM prazo; o Iniciar só abre se o shell vir Win-down+Win-up isolados
module: windows
metadata:
  type: project
  pin: true
---

**Why:** Tentativas anteriores falharam: injetar 0xE8/0xFF antes ou depois do `CallNextHookEx`, consumir só o keyup e reter o keydown por 220ms. Com prazo, todo toque lento virava Win-down injetado + Win-up físico e o Iniciar abria. Trace/I/O dentro do callback arrisca estourar `LowLevelHooksTimeout` (hook removido em silêncio → Iniciar sempre).
**How to apply:** Em `launcher/win_key.rs`, a decisão fica no `Arbiter` puro (testável no Linux): keydown físico da Win é sempre retido; keyup sem outra tecla → engole e abre o runner; outra tecla/clique/roda → `SendInput([Win-down, evento atual])` e engole o original; `Win+L` → `LockWorkStation`. Retenção órfã expira só se ficar >1,5s sem auto-repeat. Callback sem I/O: trace e ações via `PostThreadMessageW` para a thread de despacho. Limite conhecido: janela elevada em foco (UIPI) não passa pelo hook e o Iniciar abre.
