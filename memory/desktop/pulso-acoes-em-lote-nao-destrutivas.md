---
name: pulso-acoes-em-lote-nao-destrutivas
description: "Atualizar tudo" no hub só executa ação de pacote (winget upgrade --all) ou reconsulta inventários — nunca iniciar/parar serviços em massa
metadata:
  type: project
---

No `ServicesHub`, "ATUALIZAR TUDO" na aba Atualizações executa `upgrade_all_winget_packages` (`winget upgrade --all`, com accept-source/package-agreements e `--disable-interactivity`); na aba Serviços o mesmo rótulo apenas reconsulta `list_windows_services` + `list_pending_updates` em paralelo. Não existe iniciar/parar serviço em lote.

**Why:** iniciar/parar serviços em massa é ação destrutiva, depende de elevação (`EnumServicesStatusEx`/SCM sem admin falha) e quebraria o princípio de ações não destrutivas do AGENTS.md §1.4. `winget upgrade --all` é reversível e é o que o usuário espera de "atualizar tudo".

**How to apply:** qualquer novo botão em lote no hub segue a mesma regra — lote só para leitura/atualização de pacote; mudança de estado continua por item e com o erro do Windows propagado. `run_winget_upgrade` centraliza os flags comuns; não replique a lista de argumentos em outro comando.