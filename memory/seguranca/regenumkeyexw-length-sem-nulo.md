---
name: regenumkeyexw-length-sem-nulo
description: RegEnumKeyExW devolve o comprimento sem o NUL — subtrair 1 trunca o último caractere de toda subchave e some com o app do inventário
module: seguranca
metadata:
  type: project
---

**Why:** `lpcchname` de saída da `RegEnumKeyExW` é o número de caracteres gravados **sem** o terminador (é o que o `winreg` usa: `String::from_utf16(&name[..name_len as usize])`). Aplicar `saturating_sub(1)` corta o último caractere do nome — "Adobe" vira "Adob".

**Como aplicar:**
- Ao enumerar subchaves, usar `usable = (length as usize).min(buffer.len())` e nunca `- 1`.
- Consequência silenciosa: `read_app` abre uma chave que não existe (`RegHandle::open(...).ok()?`) → o app some da lista e o matching exato de resíduos nunca casa. Invariantes tipo `apps.iter().all(..)` passam com lista vazia — só um teste de contagem mínima pega isso.