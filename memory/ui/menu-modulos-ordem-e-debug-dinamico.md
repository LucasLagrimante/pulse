---
name: menu-modulos-ordem-e-debug-dinamico
description: Ordem lógica dos módulos no menu lateral e exibição condicional do Debug
module: ui
metadata:
  type: feedback
---

**Why:** A lista de módulos misturava o fluxo e dependia de index posicional rígido para ícones. Debug deve ficar ao fim e oculto quando desligado.

**How to apply:**
- Ordem oficial: Visão geral -> Atividade -> Desempenho -> Armazenamento -> Desinstalar programas -> Inicialização e segurança -> Serviços e atualizações -> Cotas de IA -> Debug.
- Resolver `ModuleIcon` por nome semântico (`name={item}`) com fallback por index para preservar componentes legados.
- Filtrar `Debug` do menu quando `!debugEnabled`; ao desativá-lo com a aba ativa, redirecionar para "Visão geral".
