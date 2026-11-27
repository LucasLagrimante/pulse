# Pulse

Central nativa para **Windows 11** e **Linux** (com port experimental para **macOS** em desenvolvimento) que reúne observabilidade, diagnóstico e controle local do sistema em uma interface única. O Pulse explica atividade de processos, desempenho, armazenamento, inicialização, rede, serviços, áudio e cotas de IA sem conta e sem telemetria. Veja em [Privacidade e rede](#privacidade-e-rede) as únicas funções opcionais que acessam a internet.

> [!IMPORTANT]
> **Build local por plataforma:**
> - **Windows 11 (via WSL)**: `npm run build:native` compila o frontend no WSL e o backend Rust/MSVC + instalador NSIS no Windows.
> - **Windows 11 (PowerShell nativo)**: `npx tauri build`.
> - **Linux nativo (`.deb` + `.AppImage`)**: `npm run build:linux` (ou `npm run build:native`, que detecta o sistema automaticamente).
> - **macOS (experimental, `.app` + `.dmg`)**: `npm run build:macos` (ou `npm run build:native`).

## Plataformas suportadas

| Plataforma | Status | Formatos | Observações |
| :--- | :--- | :--- | :--- |
| **Windows 11 x64** | Principal (Windows-first) | Instalador NSIS (`.exe`) | WebView2 incluso, APIs Win32/PDH/DXGI/Core Audio/SCM/winget, auto-update silencioso com SHA-256 |
| **Linux x86_64** | Suportado | `.deb`, `.AppImage` (e `.rpm` opcional) | WebKitGTK 4.1 (Ubuntu 22.04+, Linux Mint 21+, Debian 12+, Fedora 38+; testado no Mint 22.3). `/proc`, `/sys`, `systemd`, PipeWire/PulseAudio, XDG, elevação via `pkexec`, auto-update atômico no AppImage |
| **macOS 11+ (Apple Silicon)** | Em breve / Experimental | `.app`, `.dmg` | Compila no CI sem assinatura/notarização; ainda não validado em hardware e vários módulos seguem em andamento |

## Recursos

- **Visão geral e Atividade**: CPU, memória, disco, GPU e processos agrupados por aplicativo com ordenação multi-coluna e encerramento seguro de árvore.
- **Desempenho**: gráficos SVG em tempo real e sensores térmicos reais, com tratamento explícito de sensores **Indisponível** (nunca zero fictício).
- **Armazenamento (TreeSize + Treemap) e Limpeza controlada**: mapa visual proporcional, pastas personalizadas, arquivos recentes, limpeza em whitelist com opção agendada e diagnóstico via CLI de IA local.
- **Desinstalação rastreada**: medição real de espaço e revisão de resíduos (Ponto de Restauração + registro no Windows; pacotes do sistema/Flatpak/Snap + resíduos XDG no Linux).
- **Inicialização e segurança**: controle reversível (`StartupApproved` + impacto em ms no Windows; XDG Autostart + `systemd` no Linux).
- **Serviços e atualizações**: inspeção de serviços (`SCM`/WMI no Windows; `systemd` no Linux), revisão de segurança via CLI de IA e atualizações (`winget` no Windows; `apt`/`dnf`/`pacman`/`flatpak`/`snap` no Linux).
- **Rede e segurança**: conexões TCP/UDP e portas em escuta por processo e sinais de postura verificáveis, sem rotular executáveis arbitrariamente como malware.
- **Cotas de IA**: acompanhamento local (com histórico de 30 dias em SQLite e chaves no cofre nativo do sistema) para Claude Code, OpenAI Codex, Google Antigravity, OpenRouter e OpenCode Go.
- **Bandeja (Tray Flyout), Áudio e Debug**: painel rápido com mixer por aplicativo (Core Audio / PipeWire / PulseAudio), portas dev em escuta e trilha de auditoria local.

## Requisitos de desenvolvimento

### Windows 11 x64

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

### Linux x86_64 (Debian/Ubuntu/Mint/Fedora)

Suportado em distribuições com WebKitGTK 4.1 (Ubuntu 22.04+, Linux Mint 21+, Debian 12+, Fedora 38+). Testado em Linux Mint 22.3 (KDE Plasma, X11). Funções exclusivas do Windows (ponto de restauração, reinício elevado via UAC, winget) aparecem como indisponíveis com o motivo; ações privilegiadas pedem senha na hora via `pkexec`.

```bash
# Debian / Ubuntu / Linux Mint
sudo apt update
sudo apt install -y build-essential curl file patchelf \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libxdo-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust stable
```

Node.js 22+ também é necessário. Em Fedora, os equivalentes são `webkit2gtk4.1-devel`, `libappindicator-gtk3-devel`, `librsvg2-devel` e `openssl-devel`.

## Instalação e desenvolvimento

Clone o repositório e rode em modo dev na sua plataforma (PowerShell no Windows, terminal no Linux ou macOS):

```bash
git clone https://github.com/LucasLagrimante/pulse.git
cd pulse
npm install
npm run tauri:dev
```

O primeiro build demora mais porque o Cargo precisa baixar e compilar as dependências Rust. Os próximos builds reutilizam o cache em `src-tauri/target`.

## Build local (gerar instaladores)

No Windows o formato padrão em `src-tauri/tauri.conf.json` é NSIS (`"targets": "nsis"`). Linux e macOS sobrepõem esse alvo em `src-tauri/tauri.linux.conf.json` e `src-tauri/tauri.macos.conf.json`, mesclados automaticamente pelo Tauri na plataforma correspondente.

`npm run build:native` detecta a máquina atual automaticamente: no **WSL** compila para Windows (MSVC + NSIS); em **Linux nativo** delega para `scripts/build-linux.sh`; no **macOS** delega para `scripts/build-macos.sh`. Todos os scripts aceitam as mesmas flags (`--rust`, `--install`, `--no-landing`).

### Windows via WSL (fluxo principal do projeto)

O repositório vive no WSL e o backend é compilado no Windows (MSVC) pelo `scripts/build-native.sh`. Só o instalador local, sem alterar `landing/`:

```bash
npm run build:native -- --no-landing
```

| Comando | O que faz |
| :--- | :--- |
| `npm run build:native -- --no-landing` | Frontend + Rust + instalador NSIS, sem sincronizar a Landing Page |
| `npm run build:native` | Igual, mas ao final sincroniza versão/SHA-256 em `landing/` e `../pulse-lp` |
| `npm run build:native -- --rust --no-landing` | Reaproveita o `dist/` atual e recompila só backend + instalador |
| `npm run build:native -- --install --no-landing` | Gera o instalador e já o abre (pede elevação via UAC se necessário) |
| `npm run build:native -- --linux --no-landing` | Força build Linux (`.deb` + `.AppImage`) mesmo dentro do WSL |

`npm run tauri:build` e `npm run tauri:build:install` são apelidos do mesmo script.

### Windows nativo (PowerShell)

Com o repositório em um caminho NTFS e os requisitos instalados:

```powershell
npm install
npm run verify
npx tauri build
```

Saída Windows:

```text
src-tauri/target/release/bundle/nsis/Pulse_<versão>_x64-setup.exe
```

> [!NOTE]
> Encerre o Pulse antes de compilar (`node scripts/kill-running-app.mjs`; os scripts de build já fazem isso). No Windows, o app minimizado no tray trava `pulse-windows.exe` e o link falha com `Access is denied (os error 5)`.

### Linux (`.deb`, `.AppImage` e `.rpm`)

```bash
npm run build:linux -- --no-landing      # gera .deb + .AppImage e SHA256SUMS-linux.txt
npm run build:linux                      # igual + sincroniza metadados na Landing Page
npm run build:linux -- --install         # gera e instala o .deb via apt
npm run build:linux:rpm -- --no-landing  # opcional: gera .rpm (requer rpmbuild)
```

Saída Linux:

```text
src-tauri/target/release/bundle/deb/Pulse_<versão>_amd64.deb
src-tauri/target/release/bundle/appimage/Pulse_<versão>_amd64.AppImage
src-tauri/target/release/bundle/SHA256SUMS-linux.txt
```

O `scripts/build-linux.sh` limpa pacotes de versões antigas, verifica `webkit2gtk-4.1` e liga `APPIMAGE_EXTRACT_AND_RUN=1` automaticamente quando FUSE 2 não está presente. O AppImage roda sem instalação (`chmod +x` e executar; sem FUSE 2, passe `--appimage-extract-and-run`).

### macOS (experimental)

```bash
npm run build:macos -- --no-landing      # gera .app + .dmg e SHA256SUMS-macos.txt
```

O pacote `.dmg` gerado (macOS 11+, Apple Silicon no CI) **não é assinado nem notarizado**: o Gatekeeper bloqueia a primeira abertura (clique com o botão direito → **Abrir**, ou `xattr -dr com.apple.quarantine /Applications/Pulse.app`). Vários módulos ainda aparecem como indisponíveis no macOS e não foram validados em hardware.

## Publicação e Release

O release oficial é disparado por tag anotada `v<versão>` no workflow [`.github/workflows/release.yml`](.github/workflows/release.yml), que compila e publica na mesma GitHub Release:

1. **Windows 11 x64**: `Pulse_<versão>_x64-setup.exe`
2. **Linux x86_64**: `Pulse_<versão>_amd64.deb`, `Pulse_<versão>_amd64.AppImage` e `SHA256SUMS-linux.txt`
3. **macOS Apple Silicon (experimental)**: `Pulse_<versão>_aarch64.dmg` e `SHA256SUMS-macos.txt`

Depois que o workflow termina:

```bash
npm run lp:sync:release                  # lê os assets publicados (Windows + Linux + macOS), grava SHA-256 em release.json/landing e espelha em ../pulse-lp
node scripts/release.mjs verify <versão> # valida Release, landing local e release.json no ar para todas as plataformas publicadas
```

Para publicação manual de fallback a partir de builds locais sem assinatura:

```bash
npm run release:publish -- --unsigned [--clobber] [--platform windows|linux|macos]
```

## Verificação

```bash
npm run typecheck             # tipos TypeScript
npm run lint                  # ESLint em src/
npm test                      # testes Vitest (frontend + scripts/lib)
npm run build                 # bundle web de produção
npm run verify                # executa os quatro passos acima
npm run lp:build && npm run lp:check  # build e auditoria SEO/JSON-LD/release.json da Landing Page
bash scripts/rust-check.sh            # fmt + clippy + cargo test (Windows via WSL, ou nativo no Linux/macOS)
bash scripts/rust-check.sh --linux    # força gate Rust nativo no Linux mesmo dentro do WSL
```

## Estrutura

```text
src/          Interface React 19 + TypeScript
src-tauri/    Backend nativo Rust + Tauri 2 (Windows, Linux e preparo macOS)
landing/      Landing page oficial (espelhada em ../pulse-lp para Cloudflare Workers)
scripts/      Build local, verificação Rust, sincronização da landing e release
```

A memória de engenharia e as auditorias internas não são versionadas aqui: vivem no repositório privado `pulse-docs`, montado localmente com `npm run docs:mount`.

## Princípios do projeto

- Métricas sem suporte aparecem como **Indisponível**, nunca como zero fictício.
- Ações exclusivas de um sistema operacional são ocultadas ou mostradas com justificativa nas demais plataformas, nunca simuladas.
- Sinais de risco apresentam evidências técnicas sem rotular executáveis arbitrariamente como malware.
- Varreduras de arquivos são limitadas para preservar a responsividade do sistema.
- Operações potencialmente destrutivas não fazem parte do produto.

A implementação Python original foi removida da árvore atual e permanece no histórico Git (commit `50ac7f4`).

## Privacidade e rede

O monitoramento (processos, desempenho, armazenamento, inicialização, serviços, rede, áudio e debug) roda inteiramente na máquina, sem conta e sem telemetria. Com o modo Debug ligado, a trilha de auditoria é gravada localmente por 7 dias (`%LOCALAPPDATA%\Pulse\logs` no Windows, `~/.local/share/Pulse/logs` no Linux e `~/Library/Application Support/Pulse/logs` no macOS); com ele desligado, nada é gravado. Três funções acessam a internet (e todas podem ser desativadas ou só rodam sob demanda):

- **Atualização automática ao iniciar**: ao abrir, o Pulse consulta `https://pulse.lagrimante.com.br/release.json`. No Windows (NSIS) e no Linux (AppImage em pasta gravável), baixa o binário oficial da GitHub Release, valida o `SHA-256` e atualiza automaticamente; em `.deb` ou macOS, apenas notifica com o link da Release. Pode ser desativada a qualquer momento no topo do app (`ATUALIZAR AO INICIAR`) ou no menu da bandeja.
- **Cotas de IA**: consulta o uso das suas assinaturas diretamente nos endpoints oficiais de cada provedor (Anthropic, OpenAI, Google, OpenRouter, OpenCode) com credenciais locais ou chaves salvas no cofre nativo (Credential Manager no Windows; Secret Service no Linux). Nada passa por servidores do Pulse.
- **Assistentes com IA (Armazenamento / Serviços)**: abrem o CLI escolhido (Claude Code, Codex ou Antigravity) com um resumo local. O comando completo é exibido antes da execução.

Atualizações de aplicativos usam `winget` no Windows e os gerenciadores de pacotes nativos (`apt`, `dnf`, `pacman`, `flatpak`, `snap`) no Linux.

## Política de assinatura de código

Os instaladores do Pulse **ainda não têm assinatura de código**. O pedido ao programa da SignPath Foundation foi recusado em 2026-11 por falta de visibilidade pública do projeto; será refeito quando o projeto tiver mais adoção. Até lá, o Windows pode exibir o aviso do SmartScreen ("Mais informações" → "Executar assim mesmo") e o macOS exibe aviso do Gatekeeper.

- Os instaladores oficiais são compilados a partir deste repositório pelo workflow [`release.yml`](.github/workflows/release.yml), em runners hospedados pelo GitHub, e publicados na [GitHub Release](https://github.com/LucasLagrimante/pulse/releases) da versão.
- Confira a integridade pelo SHA-256 publicado nas notas da Release, nos arquivos `SHA256SUMS-*.txt` e na página de download.
- O workflow Windows já tem a etapa de assinatura preparada: ela só é ativada quando houver um certificado configurado.
- Committers e revisores: [Lucas Lagrimante](https://github.com/LucasLagrimante)
- Aprovadores: [Lucas Lagrimante](https://github.com/LucasLagrimante)
- Privacidade: o Pulse não transfere informação para outros sistemas em rede, a menos que o usuário peça explicitamente. As exceções estão em [Privacidade e rede](#privacidade-e-rede).

## Segurança

Encontrou uma vulnerabilidade? Não abra uma issue pública: use [GitHub Security Advisories](https://github.com/LucasLagrimante/pulse/security/advisories/new) ou escreva para llagrimante@gmail.com.

## Licença

Distribuído sob a licença [MIT](LICENSE).
