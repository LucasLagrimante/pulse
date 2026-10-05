---
name: desinstalar-whitelist-e-snapshot
description: limpeza de resíduos só apaga ids oferecidos pelo backend e conteúdo idêntico ao snapshot; nunca caminho vindo do front
module: seguranca
metadata:
  type: project
---

**Why:** O comando `cleanup_uninstall_residue` é a única parte do Pulse que
apaga em massa chaves de registro e árvores de diretório. Um `invoke` forjado
(com caminho livre) viraria `reg delete`/`rm -rf` genérico; e um uninstaller
que cria `logs\instalacao.log` depois da varredia faria o conjunto de filhos
diferir — apagar aí destruiria algo novo.

**Como aplicar:**
- O front envia **ids** (`HKLM|SOFTWARE\...|64`) e **paths que já eram nós da
  árvore ofertada**. `cleanup_with_session` recusa qualquer id fora da sessão e
  caminho fora de `is_inside_tree`.
- Antes de remover: `matches_snapshot` (comparação recursiva de caminhos
  relativos + `complete == true`) e `folder_deletion_veto` (sistema, pasta
  compartilhada, link/junction, ancestral de outro app instalado).
- Só a **raiz** da árvore aceita remoção: snapshot é por raiz, então subpasta é
  recusada com motivo explícito em vez de remover sem prova.
- Snapshot marcado `existed = false` depois de remover, senão reenviar o mesmo
  id infla `registry_removed`/`folder_removed`.
