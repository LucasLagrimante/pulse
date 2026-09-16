---
name: quota-interface-modal-openquota
description: Eliminação de painel divisor estático para cotas de IA usando modal e coleta oficial wham/usage
module: telemetria
metadata:
  type: project
---

**Why:**
Dividir a tela ao meio com o formulário de chaves de API desperdiçava 50% do espaço útil da tela de Cotas de IA, quebrando a harmonia visual em relação ao OpenQuota. Além disso, a leitura do Codex via sessões locais frequentemente continha leituras antigas ou vazias.

**How to apply:**
1. Manter a grade de provedores ocupando 100% da largura em grid fluido.
2. Configurar chaves externas (OpenRouter) via modal leve (`quota-modal-overlay`), acionável pela barra de ferramentas superior e diretamente pelo card do provedor.
3. No backend Rust, consultar primeiramente o endpoint oficial ChatGPT (`https://chatgpt.com/backend-api/wham/usage`) via tokens de `auth.json`, garantindo métricas idênticas às fontes oficiais.
