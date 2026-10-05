---
name: lookalike-limiar-1-damerau
description: Detectar imitação de nome de binário do Windows exige Damerau com limiar 1 — Levenshtein limiar 2 acusa "servico.exe" em português.
module: seguranca
metadata:
  type: project
---

**Why:** `windows_component_lookalike` (`src-tauri/src/services.rs`) compara o nome do executável
com os componentes do Windows. Com **Levenshtein e limiar 2**, `servico.exe` fica a distância 2 de
`services.exe` — e "serviço" é a palavra que todo brasileiro dá ao próprio executável. Todo serviço
chamado "servico" cairia em revisão — e o Pulse é pt-BR. Com limiar 2, `servico.exe`
foi o primeiro falso positivo que apareceu num teste.

**How to apply:**
- Distância de **Damerau-Levenshtein** (transposição = 1) e limiar **1**.
  - `svch0st.exe` → `svchost.exe`: 1 (dígito por letra) — pego.
  - `scvhost.exe` → `svchost.exe`: 1 com transposição, **2** em Levenshtein puro — precisa de Damerau.
  - `servico.exe` → `services.exe`: 2 — corretamente ignorado.
- Testes que fixam os dois lados: `portuguese_service_name_is_not_a_windows_component_lookalike`
  e `transposition_letter_swap_counts_as_one_edit`.
- Lembrete geral: filtro por semelhança de nome precisa ser testado contra **palavras do idioma do
  usuário**, não só contra o caso de ataque.
