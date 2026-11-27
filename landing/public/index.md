# Pulse — seu sistema, em perspectiva

Central nativa de observabilidade, diagnóstico e controle local para **Windows 11** e **Linux** (com port experimental para **macOS** em desenvolvimento). O Pulse reúne em um único aplicativo o que antes exigia vários utilitários separados: monitor de recursos, gerenciador de tarefas, analisador de disco (TreeSize), inventário de inicialização, inspetor de conexões e portas em escuta, desinstalador com varredura de resíduos, console de serviços/atualizações e mixer de volume por aplicativo, além de monitorar localmente suas assinaturas de IA.

- **Gratuito**, código aberto (MIT) e sem cadastro
- **Local por padrão**: sem conta, sem telemetria e sem servidor do Pulse
- **Windows 11 x64**: instalador NSIS de 4,2 MB · https://pulse.lagrimante.com.br/download/Pulse-latest-x64-setup.exe
- Versão atual: 1.5.0
- SHA-256 (Windows): `91bec09add8c9176df0dc03c91b449e6eab7ad9277249b73bd8fa2a01b416eb4`
- **Linux x86_64**: [.deb (7,8 MB)](https://pulse.lagrimante.com.br/download/Pulse-latest-amd64.deb) · SHA-256 `71b86c79e43ca062cc4e554538ff23950a3a1bf471d87c51c163c122e44d4c4f` | [AppImage (82,0 MB)](https://pulse.lagrimante.com.br/download/Pulse-latest-amd64.AppImage) · SHA-256 `7da9bdcc98b9328e44b4a7cff183cb8ec92be0cd2cb096dd35c270bf29363b24`
- **macOS (em breve · experimental)**: port em desenvolvimento para macOS 11+ (Apple Silicon `.dmg`, ainda não validado em hardware e sem assinatura/notarização)
- Assinatura de código: os instaladores ainda não são assinados, então o SmartScreen no Windows pode avisar ("Mais informações" → "Executar assim mesmo"); confira o SHA-256 publicado. Política: https://github.com/LucasLagrimante/pulse#política-de-assinatura-de-código
- Site: https://pulse.lagrimante.com.br/

## O que o Pulse faz (11 Módulos + Tray Flyout)

### 1. Visão geral da máquina
CPU, memória, disco por volume/ponto de montagem, GPU, rede e processos em uma tela unificada com cache nativo de 3,5 s e contagem de itens de inicialização.

### 2. Atividade (Processos agrupados)
Processos agrupados por aplicativo com consumo real de Memória, CPU e GPU, ordenação multi-coluna, filtro por nome/editor, linha destacada com ação de fechar própria, nome/editor/ícone resolvidos também para processos elevados ou protegidos (no Windows, com identificação dedicada de `vmmemWSL` e Compressão de memória do kernel; no Linux, via `/proc` e arquivos `.desktop`) e encerramento seguro de árvore de processos.

### 3. Desempenho e telemetria expandida em tempo real
Gráficos SVG em curva suave com preenchimento translúcido e crosshair interativo para CPU, RAM, GPU, download, upload e discos físicos. Cards clicáveis e reordenáveis (drag-and-drop ou setas) que expandem qualquer subsistema para inspecionar uso por núcleo lógico, frequência real, cache/commit de RAM, VRAM dedicada global (PDH/`nvidia-smi`), ventoinha/clocks/watts da GPU, leitura/escrita em MB/s e IOPS por disco, além de um catálogo de sensores de hardware 100% user-mode (integração automática com LibreHardwareMonitor, OpenHardwareMonitor, HWiNFO Shared Memory e PDH térmico no Windows; `/proc`, `/sys/class/hwmon` e DRM no Linux) com seletor de métricas por card. Sensores sem suporte do hardware aparecem como **Indisponível** com justificativa técnica, nunca como zero fictício.

### 4. Análise de armazenamento (TreeSize + Treemap), Explorador de Arquivos em Rust e IA CLI
Explorador de arquivos nativo em Rust integrado ao Pulse (múltiplas abas, breadcrumbs editáveis, busca rápida com guardrails de I/O, pré-visualização leve, unidades com espaço livre e exclusão segura exclusiva para a Lixeira), ao lado do mapa visual proporcional (Treemap), árvore agregada da ocupação do disco, arquivos acessados recentemente e varredura de pastas personalizadas em múltiplos volumes/montagens. Inclui integração com terminal de IA (Claude Code, Antigravity e Codex CLI) que gera relatórios estruturados das maiores pastas e pré-prompts prontos para liberar espaço com segurança; o plano de limpeza gravado pela IA abre direto no Pulse.

### 5. Limpeza local controlada
Categorias seguras em whitelist (temporários, caches de navegador, shaders, caches de ferramentas de desenvolvimento como npm/uv/Gradle e relatórios conhecidos), com estimativa exata antes da ação. Temporários das últimas 24 horas são preservados. Limpeza automática opcional (desativada por padrão) limpa em segundo plano só com o computador ocioso, preservando arquivos dos últimos 7 dias e sem nunca esvaziar a Lixeira. No Windows, a recuperação guiada também mostra comandos oficiais para compactar discos virtuais WSL/Docker e limpar a WinSxS via DISM.

### 6. Desinstalação com rastreamento de resíduos
No Windows: inventário deduplicado (`HKLM` 64/32 + `HKCU`) com tamanho real medido no disco, Ponto de Restauração do Sistema, desinstalador oficial do fabricante e varredura conservadora de pastas e chaves de registro órfãs. No Linux: inventário de pacotes do sistema (`apt`/Flatpak/Snap e `.desktop`), medição real de espaço, remoção com elevação via `pkexec` e revisão de resíduos XDG (`~/.config`, `~/.cache`, `~/.local/share`).

### 7. Inicialização e segurança
No Windows: padrão reversível `StartupApproved` (sem `reg delete` destrutivo) com impacto real em milissegundos via Event Log (`Diagnostics-Performance` IDs 101/102). No Linux: Autostart XDG (`~/.config/autostart` e `/etc/xdg/autostart`) e unidades `systemd` de usuário e sistema. Sempre apontando anomalias verificáveis sem rotular executáveis arbitrariamente como malware.

### 8. Serviços e atualizações
No Windows: inspeção nativa do Service Control Manager / WMI (classificados em *Sistema*, *Usuário* e *Revisão*) e atualizações via `winget`. No Linux: unidades `systemd` (com controle via `pkexec`) e atualizações pelos gerenciadores de pacotes instalados (`apt`, `dnf`, `pacman`, `flatpak`, `snap`). Inclui revisão de segurança dos serviços em execução via terminal de IA local (só investigação, sem alterar nada).

### 9. Cotas de assinaturas de IA (com ritmo de consumo, alerta recente e histórico de 30 dias)
Acompanhe localmente limites de janela (5h, semanal e mensal), velocidade de consumo com projeção de autonomia até o reset, alerta de aceleração recente direto na barra principal, no Tray Flyout e na Barra Superior do Pulse Shell, destaque de renovação quando uma cota esgota, saldo de créditos e histórico de 30 dias para **Claude Code**, **OpenAI Codex**, **Google Antigravity**, **OpenRouter** e **OpenCode Go**, protegendo chaves opcionais no cofre nativo do sistema (Windows Credential Manager ou Secret Service no Linux).

### 10. Segurança (auditoria determinística de acesso remoto)
Verifica de forma determinística se há acesso remoto ativo ou superfície exposta: lê a configuração e a porta real do RDP no Registro (`fDenyTSConnections`, `PortNumber` mesmo fora da 3389, `NLA` e `TermService` no Windows; `xrdp.ini` no Linux), lista sessões do sistema (`qwinsta` / `who`), inspeciona conexões TCP estabelecidas e portas em escuta direto do kernel e classifica processos em execução — diferenciando claramente componentes internos do WSLg (`msrdc.exe` dentro da pasta do WSL) de clientes de saída (`mstsc`/`msrdcw`) e agentes externos (AnyDesk, TeamViewer, RustDesk, VNC, Chrome Remote Desktop, Parsec, etc.).

### 11. Rede e segurança
O que sai da máquina e por qual processo: conexões estabelecidas, sockets UDP e portas em escuta lidas direto da tabela TCP/UDP do kernel (`GetExtendedTcpTable`/`GetExtendedUdpTable` no Windows; `/proc/net` e sockets no Linux), com loopback separado do tráfego externo e destaque para portas em `0.0.0.0`/`::`. Ao lado, sinais de postura verificáveis apresentados sempre como evidência, nunca como rótulo de malware. Não substitui antivírus: nada é bloqueado ou removido automaticamente.

### 11. Áudio, Bandeja do Sistema (Tray Flyout) e Trilha de Auditoria
Flyout compacto na bandeja do sistema com **até 4 gráficos personalizáveis** escolhidos de qualquer telemetria do hardware (CPU, RAM, GPU, rede, disco I/O, temperaturas, ventoinhas RPM, clocks ou watts), barras duplas de cotas de IA, **Mixer de Volume por aplicativo** (Core Audio COM no Windows; PipeWire/PulseAudio no Linux) com troca de dispositivo de entrada/saída, **Portas & Projetos Dev ativos** (Windows + WSL2 e Linux nativo) para abrir no navegador, copiar URL ou encerrar o processo, e módulo dinâmico de Debug/Auditoria local exportável (`%LOCALAPPDATA%\Pulse\logs` no Windows; `~/.local/share/Pulse/logs` no Linux).

### 12. Recursos Beta (opt-in, desligados por padrão)
- **Bateria de periféricos conectados**: monitora a carga real de mouses, teclados, headsets e controles via USB 2.4GHz HID (Logitech HID++ 2.0, HyperX Wireless, Razer, SteelSeries, Corsair, DualSense/DS4), Bluetooth Classic/BLE, Xbox (XInput) e `/sys/class/power_supply` (Linux) / IOKit (macOS), com resumo opcional no Tray Flyout, na Barra Superior e botão de diagnóstico de hardware.
- **Pulse Shell estilo macOS (Top Bar + Dock em Win32 + Direct2D · Windows)**: barra superior nativa com menu Pulse/energia, widgets fixados reordenáveis por arrastar e soltar (CPU, RAM, GPU, rede, periféricos, cotas de IA, ícones de outros apps, volume e relógio), assim como os ícones da Dock (apps fixados e abertos) pelo botão esquerdo do mouse, com rótulos usando o nome oficial do produto (inclusive jogos com executável de nome técnico) + popups modulares dedicados (Painel de Áudio, Cotas de IA, Calendário/Data e Hora com atalho ao Windows e Periféricos com renomeação) + proxy user-space de ícones do System Tray (sem DLL injection), combinada à Dock flutuante transparente por pixel com reserva de área útil (`SHAppBarMessage`) no topo e na base, janela independente do **Explorador de Arquivos em Rust** com busca acelerada em memória e processo watchdog dedicado que restaura a barra nativa sem encerrar `explorer.exe`.
- **Pulse Runner (Launcher Rápido em Rust)**: busca instantânea acionada opcionalmente pelo toque isolado na tecla Windows (preservando `Win+E`, `Win+R`, `Win+V`, `Win+L`, `Win+D` e `Win+Shift+S`) para abrir aplicativos recentes, programas instalados, arquivos, módulos do Pulse, ações do sistema e calculadora embutida. O toque também revela a Dock quando ela está em auto-hide. Com um jogo protegido por anti-cheat aberto (ex.: Easy Anti-Cheat, BattlEye, Vanguard, FACEIT, Marvel Rivals), o Pulse desliga a interceptação da tecla e ela volta a abrir o Menu Iniciar nativo, retomando o runner sozinho quando o jogo fecha. Opcionalmente, com uma única permissão de administrador, a tecla funciona também sobre janelas abertas como administrador: uma tarefa agendada roda uma cópia protegida do Pulse em Arquivos de Programas, sem novo aviso ao abrir o app (só após uma atualização). Os resultados mostram os ícones reais dos aplicativos extraídos localmente do Windows (sem ícones fictícios).
- **Painel de áudio do Pulse**: mixer por aplicativo, troca de entrada/saída padrão e atalho para o painel clássico do Windows (`mmsys.cpl`); na Barra Superior, o widget próprio substitui o ícone de volume duplicado do Windows. Em jogos de tela cheia a Dock fica oculta e só aparece pela tecla Windows (inclusive junto do Menu Iniciar nativo, com o runner pausado), e seu menu de contexto (fixar, fechar janelas, nova janela) não a esconde.

## Como funciona

1. **Baixe o pacote do seu sistema** — instalador NSIS compacto para Windows 11 x64 (instalador NSIS de 4,2 MB), ou pacote `.deb` / `AppImage` para Linux x86_64.
2. **Instale ou execute em segundos** — no Windows, instalação no usuário atual com auto-update validado por SHA-256; no Linux, instale o `.deb` ou rode o `AppImage` direto (com auto-update atômico + SHA-256).
3. **Abra e leia o sistema** — o painel carrega a telemetria real da máquina imediatamente e fica disponível na bandeja do sistema.

## Requisitos

- **Windows 11 x64**: suporte principal (Windows-first), WebView2 Runtime já incluído nas versões atuais do sistema.
- **Linux x86_64**: [.deb (7,8 MB)](https://pulse.lagrimante.com.br/download/Pulse-latest-amd64.deb) · SHA-256 `71b86c79e43ca062cc4e554538ff23950a3a1bf471d87c51c163c122e44d4c4f` | [AppImage (82,0 MB)](https://pulse.lagrimante.com.br/download/Pulse-latest-amd64.AppImage) · SHA-256 `7da9bdcc98b9328e44b4a7cff183cb8ec92be0cd2cb096dd35c270bf29363b24`
- **macOS (em breve · experimental)**: base em preparação para macOS 11+ (Apple Silicon `.dmg`), ainda não validada em hardware.
- Pegada leve: < 80 MB de RAM em uso e ~0% de CPU em repouso.

## Privacidade por arquitetura

A privacidade do Pulse não é uma promessa de política: é uma consequência de como o aplicativo foi construído. Não existe servidor do Pulse para onde enviar dados, porque não existe servidor do Pulse.

- Sem conta e sem login.
- Sem telemetria: nenhuma leitura do sistema é transmitida para fora.
- Sem dados fictícios: sensor sem suporte aparece como Indisponível.
- Sem ações destrutivas: nada é apagado ou alterado sem confirmação explícita; a limpeza automática só roda se você a ativar.
- Rede transparente e sob seu controle: ao iniciar, o Pulse confere novas versões no manifesto oficial e valida o SHA-256 antes de atualizar (desativável a qualquer momento); Cotas de IA consultam direto os endpoints oficiais de cada provedor; os assistentes com IA mostram o comando antes de enviar qualquer resumo ao CLI escolhido.
- Código aberto sob licença MIT.

## Links

- Landing page: https://pulse.lagrimante.com.br/
- Manifesto de release (JSON): https://pulse.lagrimante.com.br/release.json
- Código-fonte (licença MIT): https://github.com/LucasLagrimante/pulse
- Releases (instaladores + SHA-256): https://github.com/LucasLagrimante/pulse/releases
- Autor: https://lagrimante.com.br/
