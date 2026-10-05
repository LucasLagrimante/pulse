---
name: lazy-chunks-por-aba
description: Módulos pesados do frontend em chunks React.lazy para flyout e primeira pintura não os carregarem
module: desempenho
metadata:
  type: project
---
**Why:** `main` e `tray-flyout` carregam o mesmo bundle; imports estáticos de DiskAnalyzer/SubscriptionUsage faziam o flyout parsear ~54 KB de JS jamais usado, dobrando a base de memória WebView2.

**How to apply:**
- `DiskAnalyzer`, `DiskCleanup`, `SubscriptionUsage` via `React.lazy` + `<Suspense fallback={<PulseLoading compact/>}>` em `App.tsx`.
- Helpers puros usados pelo flyout ficam em módulo leve (`subscriptionHelpers.ts`, sem JSX); o componente pesado re-exporta para compat.
- Nunca importar o componente pesado estaticamente no shell; `Charts.tsx` (434 linhas) pode ficar estático (flyout usa Sparklines).
