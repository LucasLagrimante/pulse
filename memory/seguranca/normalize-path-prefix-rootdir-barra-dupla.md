---
name: normalize-path-prefix-rootdir-barra-dupla
description: Path::components emite Prefix("C:") e RootDir("\") separados — juntar com "\" produz C:\\pasta e quebra comparações de caminho
module: seguranca
metadata:
  type: project
---

**Why:** `normalize()` monta as partes de `Path::components()` e faz `parts.join("\\")`. No Windows os componentes são `Prefix("C:")`, `RootDir("\")`, `Normal("Pasta")` → o join insere mais um separador: `C:\\Pasta`. Nenhum teste de caminho do projeto pega isso hoje.

**Como aplicar:**
- Comparações por igualdade de caminho (veto de pasta do shell, id de snapshot) só funcionam se o formato for idêntico ao do front; com barra dupla o veto de pasta conhecida vira código morto.
- Também cuidado com `trim_end_matches('\\')`: a raiz da unidade vira `"c:"` (len 2) e um slice como `&lowered[3..]` entra em pânico — testar `len() <= 3` antes de indexar.
- Normalizar colapsando separadores repetidos antes do join, e treating `Prefix`/`RootDir` explicitamente.