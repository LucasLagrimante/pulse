---
name: release-padronizado-skill
description: Todo release do Pulse sai pela skill lancar-versao, que decide sozinha entre patch/minor/major pelo tamanho real da mudança; a numeração foi refeita de 1.5.0 para 1.2.0 em 2026-11-13
module: release
metadata:
  type: feedback
  pin: true
---
Em 2026-11-13 o usuário reclamou que os releases estavam "despadronizados" e que a versão subia rápido demais: de 1.1 para 1.5 em dois dias, e eu ia lançar a 1.6 só por causa de um bug. Ele também disse que a skill tem que saber sozinha se a mudança é grande ou pequena. As Releases v1.2.0 a v1.5.0 foram apagadas e a versão foi refeita como 1.2.0, calculada a partir da v1.1.1.
**Why:** a versão precisa refletir o tamanho real da mudança. "Uma menor" queria dizer versão pequena, mas eu entendi como minor do SemVer.
**How to apply:** use a skill `lancar-versao` e rode `release.mjs bump` sem informar o tipo; o `classifyRelease` decide. Patch é o padrão. Minor exige módulo `.rs` novo com `#[tauri::command]` ou `feat` em 3 ou mais áreas. Major exige `!`/`BREAKING CHANGE` e confirmação do usuário. Nunca diminua a versão, porque o auto-update só instala versão maior.
