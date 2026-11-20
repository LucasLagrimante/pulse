# Pulse

Central nativa para Windows 11 e Linux que reúne observabilidade, diagnóstico e controle local do sistema em uma interface única. O Pulse explica atividade de processos, desempenho, armazenamento, inicialização e sinais de segurança sem conta e sem telemetria. Veja em [Privacidade e rede](#privacidade-e-rede) as únicas funções opcionais que acessam a internet.

> [!IMPORTANT]
> O instalador Windows só compila com o toolchain MSVC: no PowerShell use `npx tauri build`; no WSL use `npm run build:native`, que compila no Windows a partir do WSL. No Linux o build é nativo (`npm run build:linux`). Veja [Gerar o instalador](#gerar-o-instalador) e [Linux](#linux).

## Recursos

- Visão geral de CPU, memória, disco e processos.
- Telemetria e gráficos de desempenho com tratamento explícito de sensores indisponíveis.
- Análise de armazenamento em árvore e arquivos recentes.
- Inventário de inicialização e evidências de impacto no boot.
- Rede e segurança: conexões e portas em escuta por processo e sinais de postura verificáveis.
- Diagnóstico local e painel compacto na bandeja do sistema.
- Dados locais por padrão e ações operacionais não destrutivas.

## Requisitos

- Windows 11 x64 (para Linux, veja [Linux](#linux)).
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

Clone o repositório e abra o PowerShell na raiz dele:

```powershell
git clone https://github.com/LucasLagrimante/pulse.git
cd pulse
npm install
npm run tauri dev
```

O primeiro build demora mais porque o Cargo precisa baixar e compilar as dependências Rust. Os próximos builds reutilizam o cache em `src-tauri\target`.

## Gerar o instalador

No Windows o formato configurado em `src-tauri/tauri.conf.json` é o NSIS (`"targets": "nsis"`), então nenhum comando abaixo precisa de `--bundles`. Linux e macOS sobrepõem esse alvo em `src-tauri/tauri.linux.conf.json` e `src-tauri/tauri.macos.conf.json`, que o Tauri mescla automaticamente só na plataforma correspondente.

### No WSL (fluxo do projeto)

O repositório vive no WSL e o backend é compilado no Windows (MSVC) pelo `scripts/build-native.sh`. Só o instalador local, sem alterar `landing/`:

```bash
npm run build:native -- --no-landing
```

| Comando | O que faz |
| :--- | :--- |
| `npm run build:native -- --no-landing` | Frontend + Rust + instalador NSIS, sem sincronizar a Landing Page |
| `npm run build:native` | Igual, mas ao final sincroniza versão/SHA-256 em `landing/index.html` e `landing/public/*` |
| `npm run build:native -- --rust --no-landing` | Reaproveita o `dist/` atual e recompila só backend + instalador |
| `npm run build:native -- --install --no-landing` | Gera o instalador e já o abre (pede elevação via UAC) |

`npm run tauri:build` e `npm run tauri:build:install` são apelidos do mesmo script e aceitam as mesmas flags (`--rust`, `--install`, `--no-landing`). Qualquer outra flag, como `--bundles`, aborta com `argumento desconhecido`.

### No PowerShell (Windows nativo)

Com o repositório em um caminho NTFS e os [requisitos](#requisitos) instalados:

```powershell
npm install
npm run verify
npx tauri build
```

Esse caminho não sincroniza a Landing Page.

### Saída

```text
src-tauri\target\release\bundle\nsis\Pulse_<versão>_x64-setup.exe
```

> [!NOTE]
> Encerre o Pulse antes de compilar (`node scripts/kill-running-app.mjs`; `build:native` já faz isso). O app minimizado no tray trava `src-tauri\target\release\pulse-windows.exe` e o link falha com `Access is denied (os error 5)`. O lock só aparece quando fontes Rust mudam e forçam o relink — builds só-frontend passam mesmo com o app aberto.

> [!WARNING]
> `npx tauri build` executado **dentro do WSL** gera pacotes **Linux** (ou falha com `failed to run 'cargo metadata'` se não houver Rust no WSL), nunca o instalador Windows. Para o `.exe`, no WSL use sempre `npm run build:native`.

## Linux

Suportado em distribuições com WebKitGTK 4.1 (Ubuntu 22.04+, Linux Mint 21+, Debian 12+, Fedora 38+). Testado em Linux Mint 22.3 (KDE Plasma, X11). Funções que só existem no Windows (ponto de restauração, reinício como administrador via UAC, winget) aparecem como indisponíveis com o motivo; ações privilegiadas pedem a senha na hora via `pkexec`.

### Requisitos (Debian/Ubuntu/Mint)

```bash
sudo apt update
sudo apt install -y build-essential curl file patchelf \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libxdo-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust stable
```

Node.js 22+ também é necessário. Em Fedora, os equivalentes são `webkit2gtk4.1-devel`, `libappindicator-gtk3-devel`, `librsvg2-devel` e `openssl-devel`.

### Desenvolvimento e build

```bash
npm install
npm run tauri:dev      # app em modo dev com recarga do frontend
npm run build:linux    # gera .deb e .AppImage
npm run build:linux:rpm  # opcional: .rpm (precisa de rpmbuild/rpm instalado)
```

Saída:

```text
src-tauri/target/release/bundle/deb/Pulse_<versão>_amd64.deb
src-tauri/target/release/bundle/appimage/Pulse_<versão>_amd64.AppImage
```

O `.deb` declara dependência de `libwebkit2gtk-4.1-0` e `libayatana-appindicator3-1` (ícone na bandeja). O AppImage roda sem instalação (`chmod +x` e executar); em sistemas sem FUSE 2, use `--appimage-extract-and-run`.

## macOS (experimental)

`tauri.macos.conf.json` gera `.app` + `.dmg` (macOS 11+) com `npm run build:macos`, e o CI publica um `.dmg` para Apple Silicon. O pacote **não é assinado nem notarizado**: o Gatekeeper bloqueia a primeira abertura (clique com o botão direito → **Abrir**, ou `xattr -dr com.apple.quarantine /Applications/Pulse.app`). Vários módulos ainda aparecem como indisponíveis no macOS e não foram validados em hardware.

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

A implementação Python original foi removida da árvore atual e permanece no histórico Git (commit `50ac7f4`).

## Privacidade e rede

O monitoramento (processos, desempenho, armazenamento, inicialização, serviços, áudio e debug) roda inteiramente na máquina, sem conta e sem telemetria. Com o modo Debug ligado, a trilha de auditoria também é gravada em `%LOCALAPPDATA%\Pulse\logs` (um `.jsonl` por dia, mantido por 7 dias); com ele desligado, nada é gravado. Três funções acessam a internet (e todas podem ser desativadas ou só rodam sob demanda):

- **Atualização automática ao iniciar**: por padrão, ao abrir o Pulse ele consulta `https://pulse.lagrimante.com.br/release.json` e, quando há versão mais nova, baixa o instalador da GitHub Release oficial, confere o `SHA-256` e atualiza silenciosamente. Pode ser desativada a qualquer momento no topo do app (`ATUALIZAR AO INICIAR`) ou no menu da bandeja.
- **Cotas de IA**: consulta o uso das suas assinaturas diretamente nos endpoints oficiais de cada provedor (Anthropic, OpenAI, Google, OpenRouter, OpenCode) com as credenciais que já existem na máquina ou chaves salvas no Credential Manager do Windows. Nada passa por servidores do Pulse.
- **Assistente de armazenamento com IA**: abre o CLI escolhido (Claude Code, Codex ou Antigravity) com um resumo do scan de disco. Esse resumo inclui nomes e caminhos de pastas e arquivos e é enviado ao provedor do CLI. O comando completo é exibido antes da execução.

Atualizações de aplicativos usam o `winget` do próprio Windows.

## Política de assinatura de código

O instalador do Pulse **ainda não tem assinatura de código**. O pedido ao programa da SignPath Foundation foi recusado em 2026-11 por falta de visibilidade pública do projeto; será refeito quando o projeto tiver mais adoção. Até lá, o Windows pode exibir o aviso do SmartScreen ("Mais informações" → "Executar assim mesmo").

- O instalador oficial é compilado a partir deste repositório pelo workflow [`release.yml`](.github/workflows/release.yml), em runner hospedado pelo GitHub, e publicado na [GitHub Release](https://github.com/LucasLagrimante/pulse/releases) da versão.
- Confira a integridade pelo SHA-256 publicado nas notas da Release e na página de download.
- O workflow já tem a etapa de assinatura preparada: ela só é ativada quando houver um certificado configurado.
- Committers e revisores: [Lucas Lagrimante](https://github.com/LucasLagrimante)
- Aprovadores: [Lucas Lagrimante](https://github.com/LucasLagrimante)
- Privacidade: o Pulse não transfere informação para outros sistemas em rede, a menos que o usuário peça explicitamente. As exceções estão em [Privacidade e rede](#privacidade-e-rede).

## Segurança

Encontrou uma vulnerabilidade? Não abra uma issue pública: use [GitHub Security Advisories](https://github.com/LucasLagrimante/pulse/security/advisories/new) ou escreva para llagrimante@gmail.com.

## Licença

Distribuído sob a licença [MIT](LICENSE).
