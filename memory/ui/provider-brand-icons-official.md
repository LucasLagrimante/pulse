---
name: provider-brand-icons-official
description: Ícones dos provedores de assinatura devem vir de fontes oficiais/curadas (lobe-icons, repo do opencode), nunca de traçado manual no componente
module: ui
metadata:
  type: feedback
---

**Why:**
O usuário reprovou os SVGs desenhados à mão em `src/ProviderIcon.tsx` ("ficou péssimo") e exigiu ícones oficiais para os provedores, incluindo o novo `opencode-go`.

**How to apply:**
Use paths de fontes verificadas: lobe-icons para claude, codex, antigravity e openrouter; para opencode-go, o `mark.svg` oficial (`anomalyco/opencode/packages/identity/mark.svg`, viewBox recortado `128 96 256 320` com painel interno em `opacity 0.45`). Mantenha `fill="currentColor"`, `fillRule="evenodd"` e as classes `.provider-icon-<id>` para o tint por `--quota-accent`. Nunca inventar geometria; antes de entregar, renderize os ícones com `sharp` em PNG e compare com o asset oficial (favicon/app icon do provedor).
