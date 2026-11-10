---
name: sem-tela-intermediaria-abas
description: Abas/módulos abrem direto no conteúdo calculado; sem tela de boas-vindas com botão "Analisar agora"
module: ux
metadata:
  type: feedback
---

**Why:** O usuário removeu a tela "Descubra espaço que pode ser recuperado / ANALISAR AGORA" da Limpeza de disco (2026-11-10): passo extra sem valor quando a análise é somente leitura.
**How to apply:** Análises read-only disparam no mount (`useEffect` + `loading` inicial `true`) e a tela principal mostra "Calculando…"/skeleton; em erro, "Indisponível" + alerta, nunca volta a uma tela de boas-vindas. Ações destrutivas continuam exigindo confirmação explícita.
