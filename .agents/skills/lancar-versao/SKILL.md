---
name: lancar-versao
description: Lança uma versão do Pulse de ponta a ponta e sempre do mesmo jeito — decide sozinha se é mudança pequena (patch), grande (minor) ou quebra (major) analisando commits e módulos novos, troca a versão nos 6 arquivos, gera as notas, cria a tag anotada, acompanha o CI, sincroniza a landing com o SHA-256 publicado, publica o pulse-lp e confere que o auto-update vai enxergar a versão nova. Acionar em "lançar versão", "release", "nova versão", "publicar o Pulse", "subir uma minor/patch/major", "versão nova" ou /lancar-versao.
---

# Lançar versão do Pulse

Todo release segue **exatamente** esta sequência. Não pule etapas e não improvise
ordem, mensagem de commit ou tag: é isso que mantém os releases padronizados.
A parte mecânica fica em `scripts/release.mjs` (testado em `scripts/lib/release-version.test.mjs`).

## Peças do sistema (por que a ordem importa)

| Peça | Papel |
|---|---|
| 6 arquivos de versão | `package.json`, `package-lock.json`, `landing/package.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json` — o CI recusa tag ≠ `tauri.conf.json` |
| Tag anotada `vX.Y.Z` | dispara `.github/workflows/release.yml`; a **mensagem da tag vira as notas** da GitHub Release |
| GitHub Release | hospeda `Pulse_X.Y.Z_x64-setup.exe` (sem assinatura, ver `memory/release/assinatura-signpath-ci.md`) |
| `landing/public/release.json` | SHA-256 do asset **do CI**; o auto-update do app instalado (`self_update.rs`) lê esse arquivo em `https://pulse.lagrimante.com.br/release.json` e **recusa** instalador com hash diferente |
| `../pulse-lp` | único repo que faz deploy da landing (Cloudflare). Push nele só **depois** da Release existir |

## 0. A skill decide o tipo — pequena ou grande mudança

Não pergunte ao usuário "patch ou minor?" e não escolha no olho. O classificador
(`classifyRelease` em `scripts/lib/release-version.mjs`) lê **os commits e os arquivos adicionados desde
a última tag** e decide, com as razões:

| Decisão | Critério (automático) | Exemplo real |
|---|---|---|
| **patch** `X.Y.Z+1` — mudança pequena | **padrão**: correções e `feat` pontual em módulos existentes | correção da telemetria + diagnóstico melhor no Debug |
| **minor** `X.Y+1.0` — mudança grande | módulo novo no backend (`.rs` adicionado com `#[tauri::command]`) **ou** novidades (`feat`) em 3+ áreas distintas | inspetor de portas e projetos dev (`dev_ports.rs`) |
| **major** `X+1.0.0` — quebra | `!` ou `BREAKING CHANGE` no commit. Use em: formato de `%LOCALAPPDATA%\Pulse\*` sem migração, URL/formato do `release.json`, remoção de módulo, fim de suporte a uma versão do Windows | — |

```bash
node scripts/release.mjs check     # mostra "Decisão: PATCH|MINOR|MAJOR → X.Y.Z" e as razões
```

- `bump` **sem tipo** aplica a decisão. Esse é o caminho normal.
- Major decidido: confirme com o usuário (AskUserQuestion, mostrando as razões) e rode `bump --confirmado`.
- Tipo diferente do decidido só se o usuário pedir explicitamente, com `bump <tipo> --confirmado`.
- O commit precisa dizer a verdade: quebra sem `!`/`BREAKING CHANGE` engana o classificador. Revise os
  commits do intervalo antes do `check`.
- "Uma versão menor" / "uma menor" = versão **pequena** (deixe o classificador decidir), não "minor".
- **Nunca diminua a versão.** O auto-update (`is_newer_version`) só instala versão maior que a instalada.
  A renumeração de 2026-11-13 (1.5.0 → 1.2.0, base `--from v1.1.1`) foi exceção pedida pelo usuário:
  a numeração tinha subido de 1.1 a 1.5 em dois dias, e quem tinha a 1.5.0 reinstalou à mão.

## 1. Pré-condições

1. Sessão única commitando: se houver outras sessões no repo (`ListAgents`), siga
   `memory/feedback/release-unico-committer-multissessao.md` antes de continuar.
2. Todo o trabalho do release já commitado pela skill **`commit`** (commits por assunto; nunca misturar o bump com código).
3. `node scripts/release.mjs check` → precisa terminar com `Pré-condições OK.`
   (main, sincronizada com origin, `gh` autenticado, versões coerentes).

## 2. Gate local (tudo verde, sem exceção não documentada)

```bash
npm run verify                    # typecheck + lint + vitest (inclui scripts/lib) + build
npm run lp:build && npm run lp:check
bash scripts/rust-check.sh        # rustfmt (por arquivo alterado) + clippy -D warnings + cargo test, no Windows
```

- O `rust-check.sh` roda os testes em série (em paralelo a suíte cai com ACCESS_VIOLATION, pendência
  aberta) e roda à parte, só com aviso, os testes que dependem do estado do Windows
  (`MACHINE_TESTS`; ver `memory/build/rust-check-testes-de-maquina.md`). Aviso vai no relatório;
  qualquer outra falha **para o release**.
- O CI só roda `typecheck` + `npm test`: o gate Rust é responsabilidade local.

## 3. Bump, commit e tag

```bash
node scripts/release.mjs bump          # aplica a decisão do classificador
```

Grava a versão nos 6 arquivos e as notas em `src-tauri/target/release-notes/vX.Y.Z.md` (fora do Git).
Leia as notas: se alguma linha não fizer sentido para o usuário final, corrija o arquivo de notas (não o histórico).

Commit pela skill `commit`, com **exatamente**:

- arquivos: os 6 de versão, nada mais
- mensagem: `chore(release): versão X.Y.Z`

Depois:

```bash
git tag -a --cleanup=verbatim vX.Y.Z -F src-tauri/target/release-notes/vX.Y.Z.md   # sem verbatim o git apaga os títulos "##" como comentário
git push origin main
git push origin vX.Y.Z
```

O hook de pré-commit pode passar de 120 s: rode o commit com timeout maior.

## 4. Acompanhar o CI

```bash
gh run list --workflow release.yml --limit 1
gh run watch <run-id> --exit-status
```

Se falhar:

- **Release ainda não criada** (falhou antes de publicar): corrija com commit próprio, apague a tag
  (`git push origin :refs/tags/vX.Y.Z && git tag -d vX.Y.Z`), refaça a tag no commit novo com as mesmas
  notas e dê push. A versão não muda.
- **Release já publicada com asset**: **nunca** reaproveite a versão nem mova a tag. Corrija e lance um **patch**.

## 5. Landing com o SHA-256 do CI

```bash
npm run lp:sync:release           # baixa o asset publicado, grava SHA/tamanho, builda a landing e espelha em ../pulse-lp
```

Commit pela skill `commit` com **exatamente** `chore(landing): sincronizar release vX.Y.Z com SHA-256 do asset publicado pelo CI`
(arquivos de `landing/` que o script alterou) e `git push origin main`.

Nunca rode `build:native` entre este passo e o próximo: ele regrava o hash com o do bundle local.

## 6. Publicar o pulse-lp

```bash
git -C ../pulse-lp status --short   # só pode conter o que o lp:sync:release espelhou
```

Commit no `../pulse-lp` (skill `commit`, rodando no diretório dele) com **exatamente**
`chore(release): sincronizar landing com Pulse vX.Y.Z` e push. Acompanhe o deploy:
`gh run watch --repo LucasLagrimante/pulse-lp <run-id> --exit-status`.

## 7. Verificação final (obrigatória)

```bash
node scripts/release.mjs verify X.Y.Z
```

Confere: Release publicada com o asset, `landing/public/release.json` com o SHA do asset, e o
`release.json` **no ar** anunciando X.Y.Z com o mesmo SHA (cache da Cloudflare pode levar alguns
minutos; repita antes de declarar falha). Só depois disso o release está concluído.

## 8. Relatório ao usuário

Versão, tipo e por quê, link da Release, resumo das notas, resultado do gate (com qualquer exceção
tolerada) e do `verify`. Avise que o app instalado se atualiza sozinho no próximo início
(auto-update ligado) ou pelo instalador da landing.
