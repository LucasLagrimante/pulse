# CLAUDE.md

Guia para quem (humano ou IA) for mexer neste projeto.

## O que é

TUI (estilo `htop`) que mostra arquivos recém-criados nas pastas de usuário
(Downloads, Desktop, Documents, Pictures, Videos, Music), filtrados por
categoria, com histórico persistente em SQLite e atalho para abrir a pasta
do arquivo. Ver `README.md` para o "porquê" e o desenho de produto.

## Arquitetura e responsabilidade de cada módulo

| Módulo | Responsabilidade | Natureza |
|---|---|---|
| `categories.py` | `categorize(path)` (extensão → categoria) e `is_ignored(path)` (path está numa pasta ruidosa?) | Funções puras |
| `config.py` | Defaults de pastas monitoradas + load/save de `~/.filewatch/config.toml` | Puro + I/O de arquivo simples |
| `storage.py` | Schema SQLite, `upsert_file`, `query_recent` (filtro por categoria/busca, ordenado por mais recente) | I/O determinístico (SQLite) |
| `scanner.py` | Varredura única e limitada (idade máx., poda de diretórios ignorados via `os.walk`) para popular o histórico | I/O de filesystem |
| `watcher.py` | Configura `watchdog.Observer` por pasta, filtra eventos (`categories.py`) e empilha numa `queue.Queue` thread-safe | I/O + threads |
| `app.py` | App Textual: `DataTable`, toggles de categoria, busca, atalho de abrir pasta, preview de imagem (`textual-image`), drena a fila do watcher via timer | UI |
| `main.py` | Liga tudo: carrega config, varredura inicial, sobe watchers, lança a app | Wiring |

Fluxo de dados: `config.toml` → pastas monitoradas → `scanner.py` (uma vez,
no início) + `watcher.py` (contínuo) → ambos escrevem em `storage.py` →
`app.py` lê de `storage.py` e re-renderiza a tabela.

## Decisões de design (e o porquê)

- **Eventos, não polling.** `watchdog` usa APIs nativas do SO
  (`ReadDirectoryChangesW`/`inotify`) para notificação push. Isso é o que
  permite "tempo real sem pesar" — evitar isso e voltar a um loop de
  `os.walk` periódico seria uma regressão de performance, não uma
  simplificação.
- **Escopo limitado a pastas de usuário**, nunca o drive inteiro. Se alguém
  pedir para "monitorar tudo", pondere o custo: variedade de ruído (system,
  cache, `AppData`) explode, e a varredura inicial deixa de ser barata.
- **SQLite, não only-memory.** O caso de uso central é "não lembro em qual
  pasta ficou aquele arquivo de alguns dias atrás" — sem persistência entre
  execuções, a ferramenta não resolve o problema que motivou o projeto.
- **`upsert` por path (chave primária).** Um arquivo que já existia e foi
  pego de novo pela varredura inicial (ou por um evento duplicado do
  watchdog) atualiza a linha em vez de duplicar.
- **Ignore-list por nome de diretório (`is_ignored`), casada com poda em
  `scanner.py`.** A poda em `os.walk` (`dirnames[:] = [...]`) não é só
  filtro — ela evita *descer* em `node_modules`/`.git`/etc, o que importa
  para performance em pastas de projeto grandes.
- **Multi-disco por convenção, não por configuração manual.** Além das
  pastas de usuário em `home` (`C:`), `config.default_config` detecta todos
  os drives disponíveis (`available_drive_roots`, Windows apenas) e inclui
  as mesmas pastas-padrão (`Downloads`, `Music` etc.) quando existem **na
  raiz** de outro disco (`other_drive_watched_folders`). O disco onde fica o
  `home` é pulado (já coberto pela varredura baseada em `home`). Isso
  assume que discos secundários guardam essas pastas direto na raiz — se o
  usuário usar outra convenção (ex: `D:\Users\lucas\Downloads`), a detecção
  automática não encontra e a pasta precisa ser adicionada manualmente em
  `config.toml`.

## Armadilha já encontrada: detecção automática de protocolo de imagem é instável

`textual-image` decide UMA VEZ, na importação do módulo (antes até de
`Textual` iniciar), qual protocolo gráfico usar (`AutoRenderable` em
`textual_image/renderable/__init__.py`), consultando o terminal com uma
sequência de escape e um timeout de 0.1s. A própria lib documenta que essa
consulta é "flaky". Na prática, em duas máquinas/terminais reais:

- **Warp**: nenhum protocolo detectado → cai em `UnicodeImage` (tons de
  cinza, sem cor) ou `HalfcellImage`.
- **Console clássico do PowerShell** (`conhost.exe`, não o Windows
  Terminal): a detecção **erra e reporta suporte a Sixel que não existe de
  verdade** — a imagem renderiza como um bloco preto, já que os códigos de
  escape Sixel são enviados mas não interpretados.

Por isso `app.py` não usa `AutoImage` direto: `PREVIEW_WIDGET_CLASSES`
mapeia strings (`"auto"`, `"sixel"`, `"tgp"`, `"halfcell"`, `"unicode"`) para
as classes concretas de `textual_image.widget`, e `preview_protocol` no
`config.toml` (lido em `main.py`, repassado a `RecentFilesApp.__init__`)
permite ao usuário forçar um protocolo quando a detecção automática erra.
**Lição:** nunca assuma que a detecção automática de capacidade de terminal
de uma lib de terceiros é confiável o bastante para não expor um jeito de
sobrescrevê-la manualmente — especialmente quando a própria lib avisa que
é flaky. `HalfcellImage` (colorido) é sempre preferível a `UnicodeImage`
(cinza) como fallback manual, já que ambos não dependem de protocolo real.

## Armadilha já encontrada: colchetes em texto renderizado pelo Textual

A barra de categorias (`_category_bar_text` em `app.py`) originalmente usava
`"[x]"`/`"[ ]"` como indicador de ligado/desligado. `Static.render()` no
Textual interpreta o texto como **markup do Rich**, então `[x]` era tratado
como uma tag desconhecida e silenciosamente descartado do texto renderizado
(`"[1] [x] Código"` virava `"[1]  Código"`, sem o indicador). Trocamos por
`"(1) ✓ Código"` / `"(1) · Código"` — parênteses e símbolos Unicode não
colidem com a sintaxe de markup. **Lição:** qualquer texto literal exibido
via `Static`/`Label` no Textual que use colchetes precisa ou escapar (`\[`)
ou evitar colchetes de propósito.

## Armadilha já encontrada: cuidado com nomes genéricos na ignore-list

A ignore-list já teve `"Temp"` (para cobrir `AppData\Local\Temp`). Isso
quebrava qualquer path que passasse por uma pasta chamada `Temp` — inclusive
o próprio diretório temporário de testes do pytest (`AppData\Local\Temp\
pytest-of-...`), fazendo um teste de integração falhar silenciosamente por
"filtro correto demais". Trocamos por `"AppData"`, mais específico (nenhuma
pasta de usuário legítima chama-se `AppData`). **Lição:** qualquer entrada
nova na ignore-list em `categories.py` deve ser o mais específica possível —
prefira o nome do diretório-alvo real (`AppData`, `node_modules`) a um termo
genérico que possa aparecer em paths legítimos do usuário.

## Convenções deste repo

- Sem `src/` layout — scripts na raiz, seguindo o padrão dos outros projetos
  Python do usuário (`lmstudio-dashboard`, `agents-sync`).
- `requirements.txt` + `venv` (não Poetry, não `uv`, mesmo estando
  disponível), por consistência com o resto do workspace.
- **TDD é o padrão aqui**: toda função nova em `categories.py`, `config.py`,
  `storage.py` e `scanner.py` deve ter teste escrito *antes* da
  implementação (red → green). Para `app.py`/`watcher.py` (integração com
  Textual/watchdog/threads), teste de integração real é preferível a mock
  pesado — veja `test_app.py` (usa `App.run_test()`/pilot real) e
  `test_watcher.py` (usa filesystem real, não mocka o `Observer`).
- Diretórios temporários de teste **não podem** estar sob
  `AppData\Local\Temp` quando o teste envolve `is_ignored`/`watcher.py` — use
  uma pasta local ao repo (ver fixture `watched_dir` em `test_watcher.py`)
  para não colidir com a ignore-list.

## Gotchas de Textual

- `DataTable` tem binding próprio para `Enter` (`action_select_cursor`), que
  tem prioridade sobre bindings do mesmo nome no `App`. Por isso
  `action_open_selected` é acionado via `o` (binding do App) **e** via
  `on_data_table_row_selected` (mensagem que o `DataTable` posta ao
  selecionar uma linha com Enter) — não adicione `("enter", ...)` de volta
  em `BINDINGS`, ele nunca vai disparar.
- `RecentFilesApp` só inicia o timer de drenagem da fila do watcher
  (`set_interval`) dentro de `on_mount`, nunca antes de `app.run()`/
  `run_test()` — chamar `set_interval` fora do loop de eventos ativo da app
  falha.
- A barra de categorias (`#category-bar`) é sempre re-renderizada dentro de
  `refresh_table()`, não separadamente — isso garante que ela nunca fique
  dessincronizada do que a tabela está de fato mostrando (mesmo `feed_event`
  chama `refresh_table()`, então a barra também reflete arquivos novos
  chegando via watchdog).
- **Preview de imagem** (`#preview`, classe resolvida por
  `resolve_preview_widget_class`/`PREVIEW_WIDGET_CLASSES`, ver seção de
  armadilha acima) atualiza em dois pontos: `on_data_table_row_highlighted`
  (navegação com setas move o cursor e dispara esse evento) e
  `_sync_preview_with_cursor` (chamado no fim de `refresh_table`, para o
  preview não ficar apontando para uma linha que sumiu depois de um toggle
  de categoria ou de um evento novo do watcher). `RowHighlighted` ≠
  `RowSelected` — o primeiro dispara ao simplesmente mover o cursor, o
  segundo só em `Enter`/clique; o preview quer o primeiro, o atalho de abrir
  pasta quer o segundo. Todas as classes de preview (`AutoImage`,
  `SixelImage`, `TGPImage`, `HalfcellImage`, `UnicodeImage`) compartilham a
  interface de `BaseImage` (`textual_image.widget._base.Image`) — use
  `BaseImage` no `query_one`/type hints em vez de uma classe concreta, para
  não quebrar quando `preview_protocol` mudar a classe real em uso.
- **Toggle de categoria** (`action_toggle_category`) opera sobre índices de
  `self._all_categories`, que vem ordenado alfabeticamente do config
  (`sorted(EXTENSIONS_BY_CATEGORY.keys())`). Ao escrever testes/exemplos que
  fixam qual tecla afeta qual categoria, confira a ordem alfabética real —
  já erramos isso uma vez assumindo "installer" como a 5ª quando na verdade
  é "media" (código: `code`, `document`, `image`, `installer`, `media`).

## Rodando testes

```bash
venv\Scripts\activate
pytest -q
```

47 testes, sem skips. Se algum teste de `test_watcher.py`/`test_app.py`
ficar flaky por timing, aumente o timeout do polling (`_drain_until`) em vez
de adicionar `sleep` fixo maior — a lógica já usa polling com timeout, não
espera cega.

Testes de `other_drive_watched_folders` **não** usam discos reais (não dá
para simular um `D:` de verdade em CI) — usam `Path("D:/")` como
`PureWindowsPath` mesmo sem existir de fato, com o parâmetro `exists=` para
injetar a checagem de existência. Se mexer nessa função, mantenha esse
parâmetro injetável — é o que torna a lógica de detecção de disco testável
sem depender de quais drives a máquina de quem roda os testes tem.

## Extendendo

- **Nova categoria de arquivo**: adicione extensões em
  `EXTENSIONS_BY_CATEGORY` (`categories.py`) — o resto (config default,
  filtro na TUI, scanner, watcher) já usa esse dicionário como fonte única
  de verdade. Adicione também um rótulo em `CATEGORY_LABELS` (`app.py`),
  senão a barra de categorias e a coluna "Categoria" caem no fallback (nome
  cru da categoria, ex: `"installer"` em vez de `"Instalador"`). Se
  ultrapassar 5 categorias, adicione o binding correspondente em
  `RecentFilesApp.BINDINGS` (`("6", "toggle_category(5)", ...)` etc) — hoje
  só há teclas de `1` a `5`.
- **Nova pasta ignorada**: adicione em `IGNORED_DIR_NAMES` (`categories.py`)
  — mas leia a seção "Armadilha" acima antes de escolher o nome.
- **Nova pasta monitorada por padrão**: adicione em `DEFAULT_FOLDER_NAMES`
  (`config.py`); lembre que só entra se a pasta existir de fato
  (`default_watched_folders`/`other_drive_watched_folders` já filtram
  isso). Como o nome é compartilhado entre `home` e outros discos, vale
  para os dois de uma vez.
- **Suporte a Linux para "outros discos"**: hoje `available_drive_roots`
  retorna `[]` fora do Windows (não há letras de drive). Se for necessário,
  a extensão natural é ler pontos de montagem (`/media/$USER`, `/mnt`) — mas
  isso é uma convenção bem menos garantida que letras de drive no Windows,
  então vale confirmar com o usuário antes de assumir um padrão.
