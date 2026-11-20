# Pulse — seu sistema, em perspectiva

Central nativa de observabilidade, diagnóstico e controle local para **Windows 11** e **Linux** (com port experimental para **macOS** em desenvolvimento). O Pulse reúne em um único aplicativo o que antes exigia vários utilitários separados: monitor de recursos, gerenciador de tarefas, analisador de disco (TreeSize), inventário de inicialização, inspetor de conexões e portas em escuta, desinstalador com varredura de resíduos, console de serviços/atualizações e mixer de volume por aplicativo, além de monitorar localmente suas assinaturas de IA.

- **Gratuito**, código aberto (MIT) e sem cadastro
- **Local por padrão**: sem conta, sem telemetria e sem servidor do Pulse
- **Windows 11 x64**: instalador NSIS de 3,7 MB · https://pulse.lagrimante.com.br/download/Pulse-latest-x64-setup.exe
- Versão atual: 1.3.0
- SHA-256 (Windows): `9993861f16e9e21b218948e0f9dd5c0709b5ae941c18a7f1fc68211348a7bad5`
- **Linux x86_64**: suporte nativo (.deb e AppImage) pronto no repositório; pacotes oficiais publicados a partir da próxima release em https://github.com/LucasLagrimante/pulse/releases (ou compile localmente com `npm run build:linux`)
- **macOS (em breve · experimental)**: port em desenvolvimento para macOS 11+ (Apple Silicon `.dmg`, ainda não validado em hardware e sem assinatura/notarização)
- Assinatura de código: os instaladores ainda não são assinados, então o SmartScreen no Windows pode avisar ("Mais informações" → "Executar assim mesmo"); confira o SHA-256 publicado. Política: https://github.com/LucasLagrimante/pulse#política-de-assinatura-de-código
- Site: https://pulse.lagrimante.com.br/

## O que o Pulse faz (11 Módulos + Tray Flyout)

### 1. Visão geral da máquina
CPU, memória, disco por volume/ponto de montagem, GPU, rede e processos em uma tela unificada com cache nativo de 3,5 s e contagem de itens de inicialização.

### 2. Atividade (Processos agrupados)
Processos agrupados por aplicativo com consumo real de Memória, CPU e GPU, ordenação multi-coluna, filtro por nome/editor, linha destacada com ação de fechar própria, nome/editor/ícone resolvidos também para processos elevados ou protegidos (no Windows, com identificação dedicada de `vmmemWSL` e Compressão de memória do kernel; no Linux, via `/proc` e arquivos `.desktop`) e encerramento seguro de árvore de processos.

### 3. Desempenho e telemetria em tempo real
Gráficos SVG vetoriais de CPU, RAM, GPU, download, upload e discos físicos, além de sensores térmicos verificados (PDH/NVML/WMI no Windows; `/proc`, `/sys/class/hwmon`, DRM e NVML no Linux). Sensores sem suporte do hardware aparecem como **Indisponível** com justificativa técnica, nunca como zero fictício.

### 4. Análise de armazenamento (TreeSize + Treemap) e diagnóstico via IA CLI
Mapa visual proporcional (Treemap), árvore agregada da ocupação do disco, arquivos acessados recentemente e varredura de pastas personalizadas em múltiplos volumes/montagens, com proteção de responsividade (`spawn_blocking` + `yield_now` a cada 15 arquivos). Inclui integração com terminal de IA (Claude Code, Antigravity e Codex CLI) que gera relatórios estruturados das maiores pastas e pré-prompts prontos para liberar espaço com segurança; o plano de limpeza gravado pela IA abre direto no Pulse.

### 5. Limpeza local controlada
Categorias seguras em whitelist (temporários, caches de navegador, shaders, caches de ferramentas de desenvolvimento como npm/uv/Gradle e relatórios conhecidos), com estimativa exata antes da ação. Temporários das últimas 24 horas são preservados. Limpeza automática opcional (desativada por padrão) limpa em segundo plano só com o computador ocioso, preservando arquivos dos últimos 7 dias e sem nunca esvaziar a Lixeira. No Windows, a recuperação guiada também mostra comandos oficiais para compactar discos virtuais WSL/Docker e limpar a WinSxS via DISM.

### 6. Desinstalação com rastreamento de resíduos
No Windows: inventário deduplicado (`HKLM` 64/32 + `HKCU`) com tamanho real medido no disco, Ponto de Restauração do Sistema, desinstalador oficial do fabricante e varredura conservadora de pastas e chaves de registro órfãs. No Linux: inventário de pacotes do sistema (`apt`/Flatpak/Snap e `.desktop`), medição real de espaço, remoção com elevação via `pkexec` e revisão de resíduos XDG (`~/.config`, `~/.cache`, `~/.local/share`).

### 7. Inicialização e segurança
No Windows: padrão reversível `StartupApproved` (sem `reg delete` destrutivo) com impacto real em milissegundos via Event Log (`Diagnostics-Performance` IDs 101/102). No Linux: Autostart XDG (`~/.config/autostart` e `/etc/xdg/autostart`) e unidades `systemd` de usuário e sistema. Sempre apontando anomalias verificáveis sem rotular executáveis arbitrariamente como malware.

### 8. Serviços e atualizações
No Windows: inspeção nativa do Service Control Manager / WMI (classificados em *Sistema*, *Usuário* e *Revisão*) e atualizações via `winget`. No Linux: unidades `systemd` (com controle via `pkexec`) e atualizações pelos gerenciadores de pacotes instalados (`apt`, `dnf`, `pacman`, `flatpak`, `snap`). Inclui revisão de segurança dos serviços em execução via terminal de IA local (só investigação, sem alterar nada).

### 9. Cotas de assinaturas de IA (com histórico SQLite de 30 dias)
Acompanhe localmente limites de janela (5h, diária, semanal), saldo de créditos e histórico de 30 dias em SQLite para **Claude Code**, **OpenAI Codex**, **Google Antigravity**, **OpenRouter** e **OpenCode Go**, protegendo chaves opcionais no cofre nativo do sistema (Windows Credential Manager ou Secret Service no Linux).

### 10. Rede e segurança
O que sai da máquina e por qual processo: conexões estabelecidas, sockets UDP e portas em escuta lidas direto da tabela TCP/UDP do kernel (`GetExtendedTcpTable`/`GetExtendedUdpTable` no Windows; `/proc/net` e sockets no Linux), com loopback separado do tráfego externo e destaque para portas em `0.0.0.0`/`::`. Ao lado, sinais de postura verificáveis apresentados sempre como evidência, nunca como rótulo de malware. Não substitui antivírus: nada é bloqueado ou removido automaticamente.

### 11. Áudio, Bandeja do Sistema (Tray Flyout) e Trilha de Auditoria
Flyout compacto na bandeja do sistema com telemetria instantânea, barras duplas de cotas de IA, **Mixer de Volume por aplicativo** (Core Audio COM no Windows; PipeWire/PulseAudio no Linux) com troca de dispositivo de entrada/saída, **Portas & Projetos Dev ativos** (Windows + WSL2 e Linux nativo) para abrir no navegador, copiar URL ou encerrar o processo, e módulo dinâmico de Debug/Auditoria local exportável (`%LOCALAPPDATA%\Pulse\logs` no Windows; `~/.local/share/Pulse/logs` no Linux).

## Como funciona

1. **Baixe o pacote do seu sistema** — instalador NSIS compacto para Windows 11 x64 (instalador NSIS de 3,7 MB), ou pacote `.deb` / `AppImage` para Linux x86_64.
2. **Instale ou execute em segundos** — no Windows, instalação no usuário atual com auto-update validado por SHA-256; no Linux, instale o `.deb` ou rode o `AppImage` direto (com auto-update atômico + SHA-256).
3. **Abra e leia o sistema** — o painel carrega a telemetria real da máquina imediatamente e fica disponível na bandeja do sistema.

## Requisitos

- **Windows 11 x64**: suporte principal (Windows-first), WebView2 Runtime já incluído nas versões atuais do sistema.
- **Linux x86_64**: suporte nativo (.deb e AppImage) pronto no repositório; pacotes oficiais publicados a partir da próxima release em https://github.com/LucasLagrimante/pulse/releases (ou compile localmente com `npm run build:linux`)
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
