# Pulse

Central nativa para Windows 11 que reúne observabilidade, diagnóstico e controle local do sistema em uma interface única. O Pulse explica atividade de processos, desempenho, armazenamento, inicialização e sinais de segurança sem conta, telemetria ou envio de dados para a nuvem.

> [!IMPORTANT]
> Compile e execute o Pulse em um terminal **Windows** (PowerShell, Prompt de Comando ou Windows Terminal com perfil PowerShell). O build nativo não funciona a partir do WSL sem instalar e configurar um toolchain Rust para Linux com cross-compilation para MSVC.

## Recursos

- Visão geral de CPU, memória, disco e processos.
- Telemetria e gráficos de desempenho com tratamento explícito de sensores indisponíveis.
- Análise de armazenamento em árvore e arquivos recentes.
- Inventário de inicialização e evidências de impacto no boot.
- Diagnóstico local e painel compacto na bandeja do sistema.
- Dados 100% locais e ações operacionais não destrutivas.

## Requisitos

- Windows 11 x64.
- [Node.js 22 ou superior](https://nodejs.org/).
- [Rust stable com toolchain MSVC](https://www.rust-lang.org/tools/install).
- Microsoft C++ Build Tools com a carga de trabalho **Desenvolvimento para desktop com C++** e um Windows SDK.
- WebView2 Runtime, já incluído nas versões atuais do Windows 11.

Confirme os itens principais no **PowerShell**:

```powershell
node --version
npm --version
rustc --version
cargo --version
```

Se `cargo` não for reconhecido depois de instalar o Rust, feche e abra o terminal para atualizar o `PATH`.

## Instalação e desenvolvimento

Abra o PowerShell na raiz do repositório:

```powershell
cd C:\Users\lucas\Projects\pulse
npm install
npm run tauri dev
```

O primeiro build demora mais porque o Cargo precisa baixar e compilar as dependências Rust. Os próximos builds reutilizam o cache em `src-tauri\target`.

## Gerar o instalador

No PowerShell, execute:

```powershell
cd C:\Users\lucas\Projects\pulse
npm install
npm run verify
npm run tauri:build -- --bundles nsis
```

O instalador é criado em:

```text
src-tauri\target\release\bundle\nsis\
```

Para gerar todos os formatos configurados no Tauri, também é possível usar `npm run tauri:build` sem o `-- --bundles nsis`.

Para buildar e já abrir o instalador em seguida (pede elevação via UAC):

```powershell
npm run tauri:build:install
```

> [!NOTE]
> Prefira `tauri:build` a `tauri build`: ele encerra a instância do Pulse em execução antes de compilar. O app minimizado no tray trava `src-tauri\target\release\pulse-windows.exe` e o link falha com `Access is denied (os error 5)`. O lock só aparece quando fontes Rust mudam e forçam o relink — builds só-frontend passam mesmo com o app aberto.

## Atenção ao WSL

Estar em `/mnt/c/Users/lucas/Projects/pulse` **não transforma o terminal WSL em um terminal Windows**. Neste caso, o comando pode falhar assim:

```text
failed to run 'cargo metadata': No such file or directory (os error 2)
```

Esse erro significa que o Tauri tentou encontrar o `cargo` dentro do Linux. Saia do WSL e rode o build no PowerShell. Se estiver em um prompt WSL e quiser iniciar o comando no Windows sem trocar de janela, use:

```bash
powershell.exe -NoProfile -Command "Set-Location 'C:\Users\lucas\Projects\pulse'; npm run tauri:build -- --bundles nsis"
```

## Verificação

```powershell
npm run typecheck  # tipos TypeScript
npm test           # testes Vitest
npm run build      # bundle web de produção
npm run verify     # executa os três passos acima
```

## Estrutura

```text
src/          Interface React 19 + TypeScript
src-tauri/    Backend nativo Rust + Tauri 2
memory/       Decisões e lições arquiteturais do projeto
```

## Princípios do projeto

- Métricas sem suporte aparecem como **Indisponível**, nunca como zero fictício.
- Sinais de risco apresentam evidências técnicas sem rotular executáveis arbitrariamente como malware.
- Varreduras de arquivos são limitadas para preservar a responsividade do Windows.
- Operações potencialmente destrutivas não fazem parte do produto.

A implementação Python original permanece preservada em [`legacy-python/`](legacy-python/) e no commit `50ac7f4`.
