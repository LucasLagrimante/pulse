---
name: no-hardcoded-oauth-secrets
description: Proibido embutir client secrets de OAuth no código-fonte mesmo que ofuscados por concatenação
module: seguranca
metadata:
  type: project
  pin: true
---

**Why:** Clientes desktop são públicos e não mantêm segredos confidenciais. Hardcodar segredos (inclusive via concatenação) aciona bloqueios de segurança e expõe credenciais.

**How to apply:**
- Nunca embutir client secrets ou API keys no binário ou frontend.
- Usar variáveis de ambiente opcionais (`env::var`) ou o keyring nativo do sistema operacional.
- Para renovação de tokens de terceiros, falhar graciosamente e instruir reautenticação pelo aplicativo de origem.
