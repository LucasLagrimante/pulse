---
name: cotas-matriz-modelo-janela
description: Provedor com cota por modelo (Antigravity) renderiza matriz modelo × janela em vez de uma barra por janela
module: ui
metadata:
  type: feedback
---
**Why:** O usuário rejeitou o card do Antigravity com 4–6 barras empilhadas ("já não fica legal"); com o modelo premium separado viraria 5–6 linhas.
**How to apply:** `groupQuotaWindowsByModel` (rótulos "Modelo · Janela", ≥2 modelos) gera linhas = modelo, colunas = duração; célula ausente mostra "—" com motivo, nunca 0%. Codex/Claude (sem " · ") seguem na lista simples. Nome curto via `shortModelName` só na UI; o rótulo bruto continua no histórico.
