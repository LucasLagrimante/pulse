# Pulse — seu Windows, em perspectiva

Central nativa de observabilidade, diagnóstico e controle local para Windows 11. O Pulse reúne em um único aplicativo o que antes exigia vários utilitários separados: monitor de recursos, gerenciador de tarefas, analisador de disco (TreeSize), inventário de inicialização, inspetor de conexões e portas em escuta, desinstalador com varredura de resíduos, console de serviços/atualizações e mixer de volume por aplicativo, além de monitorar localmente suas assinaturas de IA.

- **Gratuito**, código aberto (MIT) e sem cadastro
- **Local por padrão**: sem conta, sem telemetria e sem servidor do Pulse
- **Windows 11 x64**, instalador NSIS de 3,7 MB
- Versão atual: 1.3.0
- SHA-256: `9993861f16e9e21b218948e0f9dd5c0709b5ae941c18a7f1fc68211348a7bad5`
- Assinatura de código: o instalador ainda não é assinado, então o SmartScreen pode avisar ("Mais informações" → "Executar assim mesmo"); confira o SHA-256 publicado. Política: https://github.com/LucasLagrimante/pulse#política-de-assinatura-de-código
- Site: https://pulse.lagrimante.com.br/
- Download direto: https://pulse.lagrimante.com.br/download/Pulse-latest-x64-setup.exe

## O que o Pulse faz (10 Módulos + Tray Flyout)

### 1. Visão geral da máquina
CPU, memória, disco por volume, GPU, rede e processos em uma tela unificada com cache nativo de 3,5 s e contagem de itens de inicialização.

### 2. Atividade (Processos agrupados)
Processos agrupados por aplicativo com consumo real de Memória, CPU e GPU, ordenação multi-coluna, filtro por nome/editor, linha destacada com ação de fechar própria, nome, editor e ícone resolvidos também para processos elevados ou protegidos (como o Microsoft Defender), identificação dedicada do host WSL (`vmmemWSL`) e de componentes do kernel (Compressão de memória, Sistema) e encerramento seguro de árvore de processos.

### 3. Desempenho e telemetria em tempo real
Gráficos SVG vetoriais de CPU, RAM, GPU, download, upload e discos físicos, além de sensores térmicos verificados. Sensores sem suporte do hardware aparecem como **Indisponível** com justificativa técnica, nunca como zero fictício.

### 4. Análise de armazenamento (TreeSize + Treemap) e diagnóstico via IA CLI
Mapa visual proporcional (Treemap), árvore agregada da ocupação do disco, arquivos acessados recentemente e varredura de pastas personalizadas em múltiplos volumes, com proteção anti-DPC (`spawn_blocking` + `yield_now` a cada 15 arquivos) para nunca travar o cursor do mouse. Inclui integração com terminal de IA (Claude Code, Antigravity e Codex CLI; o botão mostra só os CLIs instalados, sem ligar o WSL) que gera relatórios estruturados das maiores pastas e pré-prompts prontos para encontrar e liberar espaço com segurança; o plano de limpeza que a IA grava aparece no próprio Pulse para abrir com um clique.

### 5. Limpeza local controlada
Categorias seguras em whitelist (temporários, caches e relatórios conhecidos), com estimativa exata de tamanho antes da ação. Inclui caches de shaders NVIDIA e de ferramentas de desenvolvimento (npm, uv, Gradle; só na limpeza manual). Os temporários das últimas 24 horas são sempre preservados. Limpeza automática opcional (desativada por padrão): uma vez ativada, a escolha fica salva e o Pulse limpa as categorias marcadas em segundo plano (diária, semanal ou mensal), só com o PC ocioso e na tomada, preservando arquivos dos últimos 7 dias e sem nunca esvaziar a Lixeira. A recuperação guiada detecta discos virtuais do WSL/Docker (.vhdx) e a WinSxS e mostra os comandos oficiais (compactar com diskpart, DISM) para você executar como administrador; o Pulse nunca apaga esses arquivos.

### 6. Desinstalação com Ponto de Restauração e rastreamento de resíduos
Inventário deduplicado (`HKLM` 64/32 + `HKCU`) com tamanho real medido no disco — pasta do programa (declarada ou deduzida e marcada como tal) mais os dados em AppData/ProgramData somados à parte, com medição completa sob demanda para pastas grandes (a estimativa do instalador fica só como fallback, marcada como tal); a medição segue até o fim em segundo plano mesmo ao trocar de módulo e a última varredura fica em cache local, então a próxima abertura é instantânea enquanto o Pulse confere o disco de novo, ordenação por tamanho (padrão: do maior para o menor), data de instalação ou nome e ação de desinstalar destacada na própria linha; cria Ponto de Restauração do Sistema, executa o desinstalador oficial do fabricante e faz uma varredura conservadora de pastas e chaves de registro órfãs para você revisar antes de remover.

### 7. Inicialização e segurança
Inventário completo do boot utilizando o padrão reversível `StartupApproved` do Windows (sem `reg delete` destrutivo), com medição de impacto real em milissegundos via Event Log (`Diagnostics-Performance` IDs 101/102) e detecção de anomalias verificáveis sem rotular executáveis arbitrariamente como malware.

### 8. Serviços e atualizações (SCM + Winget)
Inspeção nativa de serviços do Windows via Service Control Manager / WMI (classificados em *Sistema*, *Usuário* e *Revisão* com evidências técnicas) e conferência/instalação de atualizações pendentes de aplicativos via `winget`. Um botão de revisão de segurança com IA (Claude Code, Antigravity ou Codex CLI) gera um relatório dos serviços em execução fora das pastas do Windows e um pré-prompt para a IA procurar sinais de keylogger, spyware ou acesso indevido — só investigação, sem parar nem alterar nada; o parecer gravado pela IA abre direto no Pulse.

### 9. Cotas de assinaturas de IA (com histórico SQLite de 30 dias)
Acompanhe localmente limites de janela (5h, diária, semanal), saldo de créditos e histórico de 30 dias em SQLite (com pico e média diária) para **Claude Code**, **OpenAI Codex**, **Google Antigravity** (cotas separadas por modelo — Gemini, Claude + GPT e Gemini 4 Argon — numa matriz compacta modelo × janela), **OpenRouter** e **OpenCode Go**, protegendo chaves opcionais no Windows Credential Manager (`keyring`).

### 10. Rede e segurança
O que sai da máquina e por qual processo, lido direto da tabela TCP/UDP do kernel (`GetExtendedTcpTable`/`GetExtendedUdpTable`, coleta medida em ~1,1 ms): conexões estabelecidas, sockets UDP e portas em escuta por processo, conexões de loopback separadas das externas, destinos agrupados sem repetição (teto de 24 por processo) e destaque para portas que escutam em `0.0.0.0`/`::` e por isso são alcançáveis pela rede local. Nenhum processo recebe nome inventado: quando o executável é protegido (PPL ou elevado sem o Pulse elevado) o caminho aparece como indisponível, e uma tabela que não pôde ser lida aparece com o motivo, nunca como "nenhuma conexão". Ao lado, sinais de postura verificáveis com cache de 2 minutos: filtros de teclado e mouse (registro), Proxy do sistema, raízes de confiança adicionadas ao store de certificados, estado do Microsoft Defender (WMI) e visibilidade de auditoria do Windows. Cada sinal mostra a evidência e o que ela significa; o Pulse não rotula programa como malware e não substitui antivírus — nada é bloqueado ou removido automaticamente.

### 11. Áudio, Bandeja do Sistema (Tray Flyout) e Trilha de Auditoria
Flyout compacto na bandeja do sistema com telemetria instantânea, barras duplas de cotas de IA (clique abre direto a aba de Cotas; o hover mostra quanto falta para cada janela renovar, também no tooltip do ícone da bandeja), **Mixer de Volume Core Audio por aplicativo** (`ISimpleAudioVolume`, mute individual e troca de dispositivo de entrada/saída), **Portas & Projetos Dev ativos** (detecta servidores TCP em `LISTEN` no Windows nativo via `GetExtendedTcpTable` e no WSL2 sem acordar a VM parada, identifica projeto/runtime e permite abrir `http://localhost:porta`, copiar URL ou encerrar o processo tanto no painel rápido quanto no submenu do botão direito do tray) e módulo dinâmico de Debug/Auditoria local exportável (falhas com etapa, erro técnico e trecho do dado; com o modo Debug ligado, eventos gravados em `%LOCALAPPDATA%\Pulse\logs` por 7 dias, sem sair da máquina).

## Como funciona

1. **Baixe o instalador** — um único executável NSIS compacto (instalador NSIS de 3,7 MB), configurado para instalação no usuário atual sem exigir elevação de administrador para todo o sistema. O Pulse roda sem administrador; o indicador PERMISSÃO no topo mostra o modo atual, lista o que depende de elevação (pastas do sistema na limpeza, ponto de restauração, serviços do Windows) e permite reiniciar como administrador pelo UAC quando você quiser.
2. **Instale em segundos** — a instalação é local, fecha instâncias anteriores automaticamente em upgrades, utiliza o WebView2 nativo do Windows 11 e mantém o Pulse atualizado automaticamente ao iniciar com verificação de SHA-256 (com toggle para desativar no topo ou na bandeja).
3. **Abra e leia o sistema** — o painel carrega a telemetria da máquina imediatamente e fica disponível na bandeja do sistema.

## Requisitos

- Windows 11 x64.
- WebView2 Runtime (já incluído nas versões atuais do Windows 11).
- Pegada leve: < 80 MB de RAM em uso e ~0% de CPU em repouso.

## Privacidade por arquitetura

A privacidade do Pulse não é uma promessa de política: é uma consequência de como o aplicativo foi construído. Não existe servidor do Pulse para onde enviar dados, porque não existe servidor do Pulse.

- Sem conta e sem login.
- Sem telemetria: nenhuma leitura do sistema é transmitida para fora.
- Sem dados fictícios: sensor sem suporte aparece como Indisponível.
- Sem ações destrutivas: nada é apagado ou alterado sem confirmação explícita; a limpeza automática só roda se você a ativar.
- Rede transparente e sob seu controle: ao iniciar, o Pulse confere novas versões no manifesto oficial e atualiza silenciosamente após validar o SHA-256 (desativável a qualquer momento); Cotas de IA consultam direto os endpoints oficiais de cada provedor; o assistente de armazenamento envia ao CLI de IA escolhido um resumo com nomes e caminhos do scan e mostra o comando antes de executar.
- Código aberto sob licença MIT.

## Links

- Landing page: https://pulse.lagrimante.com.br/
- Manifesto de release (JSON): https://pulse.lagrimante.com.br/release.json
- Código-fonte (licença MIT): https://github.com/LucasLagrimante/pulse
- Releases (instalador + SHA-256): https://github.com/LucasLagrimante/pulse/releases
- Autor: https://lucaslagrimante.github.io/
