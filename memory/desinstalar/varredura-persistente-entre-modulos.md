---
name: varredura-persistente-entre-modulos
description: A varredura da Remoção (inventário + medição) vive em store de módulo (src/uninstallScan.ts), não em useEffect — o App desmonta módulos inativos e o cleanup abortava a medição
module: desinstalar
metadata:
  type: feedback
---
**Why:** O usuário pediu (2026-11-13) que a busca da "Remoção com rastreamento de resíduos" vá até o fim, sem parar nem reiniciar ao trocar de módulo, com cache temporário para reabrir rápido revalidando em segundo plano. `App.tsx` renderiza só o módulo ativo; o loop no `useEffect` morria no unmount e recomeçava do zero.
**How to apply:** Trabalho longo de módulo vai num store fora do React (`useSyncExternalStore`) com fila incremental (`settled` por sessão, nada de recomeçar). Cache em `localStorage` (`pulse.uninstall.scan.v1`, 7 dias): exibe na hora com aviso "Exibindo a última varredura…" e só regrava depois de revalidar tudo. `setState` ignora patch sem mudança (evita re-render e avisos `act`); testes chamam `resetUninstallScan()` + `localStorage.clear()` no `beforeEach`.
