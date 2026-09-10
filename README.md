# Recent Files Monitor

Monitor de arquivos recentes em tempo real, no estilo `htop`/`bashtop`, para
Windows e Linux — mas em vez de CPU e memória, ele mostra **arquivos
recém-criados** nas suas pastas de uso comum.

Resolve um problema específico: você tira um print, baixa algo, salva um
script — e minutos (ou dias) depois não lembra em qual pasta o arquivo foi
parar. O monitor mantém um histórico pesquisável e filtrado por categoria,
com um atalho para abrir a pasta do arquivo direto.

## Por que existe

- **Tempo real sem pesar o sistema**: usa eventos nativos do SO
  (`ReadDirectoryChangesW` no Windows / `inotify` no Linux, via
  [`watchdog`](https://pypi.org/project/watchdog/)) em vez de varrer pastas em
  loop. Nada de polling caro.
- **Só o que importa**: filtra por categoria (código, imagens, mídia,
  documentos, instaladores/arquivos compactados) e ignora ruído —
  `node_modules`, `.git`, `__pycache__`, `AppData`, `venv`, `dist`, `build`
  etc.
- **Histórico persistente**: grava em SQLite local, então "aquele arquivo de
  3 dias atrás" continua lá mesmo depois de fechar e reabrir o programa.
- **Preview de imagem**: ao navegar até uma linha de imagem com as setas, um
  painel lateral mostra a imagem renderizada (via
  [`textual-image`](https://pypi.org/project/textual-image/), que detecta o
  melhor protocolo gráfico do seu terminal — Sixel, Kitty, iTerm2 — e cai
  para um fallback em blocos Unicode se nenhum for suportado).
- **Leve**: escopo limitado a pastas de usuário (Downloads, Desktop,
  Documents, Pictures, Videos, Music), não o drive inteiro.

## Instalação

Requer Python 3.12+.

```bash
python -m venv venv
venv\Scripts\activate        # Windows
# source venv/bin/activate   # Linux
pip install -r requirements.txt
```

## Uso

```bash
python main.py
```

```
┌─ Recent Files Monitor ────────────────────────────────────────────┐
│ (1) ✓ Código   (2) ✓ Documento   (3) ✓ Imagem   (4) ✓ Instalador  │
│ (5) ✓ Mídia                                                       │
├─────────────────────────────┬──────────┬───────────┬─────────────┤
│ Nome                        │ Categoria│ Quando     │ Pasta       │
├─────────────────────────────┼──────────┼───────────┼─────────────┤
│ screenshot_2026-07-02.png   │ Imagem   │ 2 min atrás│ Downloads   │
│ analyzer.py                 │ Código   │ 15 min     │ Projects\x  │
│ relatorio.pdf               │ Documento│ 1h atrás   │ Documents   │
└─────────────────────────────┴──────────┴───────────┴─────────────┘
```

A barra do topo mostra o nome de cada categoria ao lado do número —
`✓` = habilitada, `·` = desligada. A ordem é sempre alfabética por nome de
categoria (Código, Documento, Imagem, Instalador, Mídia), então a tecla `N`
sempre corresponde à N-ésima categoria mostrada na barra, não a uma posição
fixa.

Ao navegar (setas ↑/↓) até uma linha de **imagem**, um painel abre à direita
com a imagem renderizada; em qualquer outra categoria, o painel some.

Atalhos:

| Tecla        | Ação                                            |
|--------------|--------------------------------------------------|
| `1`-`5`      | Liga/desliga a categoria correspondente (veja a barra no topo) |
| `Enter`/`o`  | Abre a **pasta** do arquivo selecionado (não o arquivo) |
| `q`          | Sai                                              |

## Configuração

Na primeira execução é criado `~/.filewatch/config.toml`, com as pastas
monitoradas e as categorias habilitadas. Edite esse arquivo para
adicionar/remover pastas ou categorias.

As pastas monitoradas por padrão vêm de duas fontes:

- **No disco do seu usuário** (`C:`): `Downloads`, `Desktop`, `Documents`,
  `Pictures`, `Videos`, `Music` dentro da pasta do usuário — só as que
  existem.
- **Em outros discos** (Windows): o programa detecta todos os drives
  disponíveis (`D:`, `E:`, ...) e inclui `Downloads`, `Desktop`, `Documents`,
  `Pictures`, `Videos`, `Music` que existirem **na raiz** de cada um — útil
  se você guarda mídia/projetos num disco separado do sistema.

O histórico fica em `~/.filewatch/history.db` (SQLite) e persiste entre
execuções — a cada início, uma varredura rápida (últimos 30 dias, pastas
ignoradas puladas) recupera o que mudou enquanto o programa estava fechado;
depois disso, só eventos ao vivo alimentam a tabela.

### Preview de imagem: forçando o protocolo

Por padrão (`preview_protocol = "auto"`), o programa detecta uma vez, na
inicialização, o melhor protocolo gráfico do seu terminal (Sixel, Kitty).
Essa detecção é conhecidamente instável — a própria lib usada avisa que
leituras concorrentes do terminal podem dar falso positivo. Na prática:

- Terminais **sem nenhum protocolo gráfico** (ex: Warp) caem num fallback em
  texto: `unicode` (cinza, sem cor) ou `halfcell` (colorido, resolução
  vertical dobrada — sempre melhor que `unicode` quando nenhum protocolo
  real está disponível).
- Alguns terminais (ex: o console clássico do PowerShell/`conhost.exe`)
  reportam suporte a Sixel que na prática não funciona, e a imagem aparece
  **preta**.

Se isso acontecer, force um valor específico em `preview_protocol` no
`config.toml`:

| Valor | Quando usar |
|---|---|
| `"auto"` (padrão) | Terminal com suporte real a Sixel/Kitty (ex: WezTerm, Windows Terminal recente) |
| `"halfcell"` | Terminal sem protocolo gráfico, ou onde `"auto"` deu imagem preta/quebrada — melhor fallback colorido |
| `"unicode"` | Fallback em tons de cinza, sem depender de cor do terminal |
| `"sixel"` / `"tgp"` | Força um protocolo específico manualmente |

Para a melhor qualidade possível (pixels reais, não uma aproximação em
caracteres), rode o programa num terminal com suporte nativo a Sixel ou ao
protocolo gráfico do Kitty — por exemplo **WezTerm** (suporte robusto ao
protocolo Kitty, funciona bem no Windows) ou o **Windows Terminal** mais
recente (suporte experimental a Sixel).

## Arquitetura

```
config.toml ──► config.py ──► main.py ──┬─► scanner.py  (varredura inicial limitada)
                                          ├─► watcher.py  (watchdog: eventos ao vivo)
                                          ├─► storage.py  (SQLite: upsert/query)
                                          └─► app.py      (Textual: TUI)
```

Veja [CLAUDE.md](./CLAUDE.md) para detalhes de arquitetura, decisões de
design e convenções para quem for mexer no código.

## Testes

```bash
pytest -q
```

47 testes cobrindo categorização/ignore-list, config, storage, scanner,
watcher (com filesystem real) e a TUI (via `textual` `run_test()`/pilot).
