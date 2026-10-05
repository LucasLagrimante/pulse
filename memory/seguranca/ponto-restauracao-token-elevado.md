---
name: ponto-restauracao-token-elevado
description: SRSetRestorePointW exige token efetivo elevado e quem confirma a criação é o evento do System Restore, não o BOOL
module: seguranca
metadata:
  type: project
---

**Why:** `IsUserAnAdmin` (shell32) responde pelo SID **filtrado** do UAC e
devolve `TRUE` para administrador não elevado — com ele, o Pulse criaria o
ponto com token sem privilégio e falharia com ERROR_ACCESS_DENIED. E o Windows
pode devolver `TRUE`/`nStatus = 0` **sem criar nada** (limite padrão de um
ponto a cada 24 h), então o `BOOL` não prova nada.

**Como aplicar:**
- Elevação real: `CheckTokenMembership` com `WinBuiltinAdministratorsSid` no
  token efetivo (SID admin é deny-only no token filtrado → `false`).
- `srclient.dll` carregada por `LoadLibraryExW` + `GetProcAddress`: edição sem
  System Restore vira erro declarado, não falha de link do app inteiro.
- `BEGIN_SYSTEM_CHANGE` (100) depois `END_SYSTEM_CHANGE` (101) com o mesmo
  `llSequenceNumber`; erro vem de `STATEMGRSTATUS.nStatus`, nunca do `BOOL`.
- Confirmação = evento do provedor `System Restore` no log `Application`
  contendo a descrição que gravamos **e** `TimeCreated` posterior ao início.
  Sem log, o resultado é "criado, sem confirmação" — não "criado".
