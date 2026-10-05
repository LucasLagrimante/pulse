---
name: conta-interna-do-windows-vem-com-nt-authority
description: O WMI devolve LocalService/NetworkService/SYSTEM com e sem o prefixo "NT AUTHORITY\" — comparar só a folha, senão 90 serviços de sistema viram "de usuário".
module: seguranca
metadata:
  type: project
  pin: true
---

**Why:** `Win32_Service.StartName` não é consistente. Nesta máquina, os 313 serviços trouxeram
as mesmas três contas internas em **duas grafias**: `LocalSystem` e `NT AUTHORITY\SYSTEM`,
`NT Authority\LocalService`, `LocalService`. Comparar a string inteira deixou **90 serviços de
sistema** (LocalService e NetworkService) classificados como "de usuário".

**How to apply:**
- Normalizar em minúsculas e remover o prefixo `NT AUTHORITY\` antes de comparar com a lista
  `SYSTEM_ACCOUNTS = ["system", "localservice", "networkservice", "localsystem"]`
  (`is_system_account` em `src-tauri/src/services.rs`).
- Grupos de serviços que o Windows cria **por usuário e por sessão** têm `StartName` **nulo** —
  25 dos 30 sem conta nesta máquina. São reconhecidos pelo sufixo `_<id da sessão>` no nome
  (`is_per_user_service_name`, ex.: `UserDataSvc_128821`), mesmo padrão do Gerenciador de Tarefas
  e do Autoruns. Sem isso, 10% da lista ficaria em "sem dados".
- Conta **explícita** ganha do sufixo: se `StartName` é do sistema, o serviço é do sistema,
  mesmo que o nome finja o padrão de sessão.
- 6 serviços continuam em "sem dados" de verdade (sem conta e sem `PathName`): LSM, NetSetupSvc,
  WaaSMedicSvc, IsolationSession, IsoSessionCore, PrintScanBrokerService. Isso é honestidade, não bug.
