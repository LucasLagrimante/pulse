---
name: Initial Chart Skeleton
description: Transição visual dos gráficos entre a abertura do painel e a primeira coleta real
module: telemetria
metadata:
  type: project
---
**Why:**
A tela inicial pode ocultar toda a primeira coleta e fazer o painel saltar diretamente para dados prontos ou para "Indisponível". Ao entrar antecipadamente no painel, uma série ainda vazia também não deve parecer uma falha de sensor.

**How to apply:**
Mantenha um estado explícito para a primeira coleta e exiba o skeleton dos gráficos por uma janela curta após a tela inicial. O skeleton deve usar apenas formas neutras, preservar `aria-busy` e respeitar `prefers-reduced-motion`. Depois da janela inicial, uma série sem amostras volta ao estado honesto "Indisponível"; nunca desenhe zero como fallback.
