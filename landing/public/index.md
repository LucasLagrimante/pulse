# Pulse — seu Windows, em perspectiva

Central nativa de observabilidade, diagnóstico e controle local para Windows 11. O Pulse reúne em um único aplicativo o que antes exigia sete utilitários separados: monitor de recursos, gerenciador de tarefas, analisador de disco (TreeSize), inventário de inicialização, desinstalador com varredura de resíduos, console de serviços/atualizações e mixer de volume por aplicativo, além de monitorar localmente suas assinaturas de IA.

- **Gratuito**, código aberto e sem cadastro
- **100% local**: sem conta, sem telemetria, sem nuvem
- **Windows 11 x64**, instalador NSIS de 3,5 MB
- Versão atual: 1.0.0
- SHA-256: `d1c034853020fbd0663fab244f886456bd82c24aa8e6e53889af6c8c6002d06a`
- Site: https://pulse.lagrimante.com.br/
- Download direto: https://pulse.lagrimante.com.br/download/Pulse-latest-x64-setup.exe

## O que o Pulse faz (10 Módulos + Tray Flyout)

### 1. Visão geral da máquina
CPU, memória, disco por volume, GPU, rede e processos em uma tela unificada com cache nativo de 3,5 s e contagem de itens de inicialização.

### 2. Atividade (Processos agrupados)
Processos agrupados por aplicativo com consumo real de Memória, CPU e GPU, ordenação multi-coluna, identificação dedicada do host WSL (`vmmemWSL`) e encerramento seguro de árvore de processos.

### 3. Desempenho e telemetria em tempo real
Gráficos SVG vetoriais de CPU, RAM, GPU, download, upload e discos físicos, além de sensores térmicos verificados. Sensores sem suporte do hardware aparecem como **Indisponível** com justificativa técnica, nunca como zero fictício.

### 4. Análise de armazenamento (TreeSize + Treemap)
Mapa visual proporcional (Treemap), árvore agregada da ocupação do disco, arquivos acessados recentemente e varredura de pastas personalizadas em múltiplos volumes, com proteção anti-DPC (`spawn_blocking` + `yield_now` a cada 15 arquivos) para nunca travar o cursor do mouse.

### 5. Limpeza local controlada
Categorias seguras em whitelist (temporários, caches e relatórios conhecidos), com estimativa exata de tamanho antes da ação. O Pulse nunca apaga nada por conta própria.

### 6. Desinstalação com Ponto de Restauração e rastreamento de resíduos
Inventário deduplicado (`HKLM` 64/32 + `HKCU`) que cria Ponto de Restauração do Sistema, executa o desinstalador oficial do fabricante e faz uma varredura conservadora de pastas e chaves de registro órfãs para você revisar antes de remover.

### 7. Inicialização e segurança
Inventário completo do boot utilizando o padrão reversível `StartupApproved` do Windows (sem `reg delete` destrutivo), com medição de impacto real em milissegundos via Event Log (`Diagnostics-Performance` IDs 101/102) e detecção de anomalias verificáveis sem rotular executáveis arbitrariamente como malware.

### 8. Serviços e atualizações (SCM + Winget)
Inspeção nativa de serviços do Windows via Service Control Manager / WMI (classificados em *Sistema*, *Usuário* e *Revisão* com evidências técnicas) e conferência/instalação de atualizações pendentes de aplicativos via `winget`.

### 9. Cotas de assinaturas de IA (com histórico SQLite de 30 dias)
Acompanhe localmente limites de janela (5h, diária, semanal), saldo de créditos e histórico de 30 dias em SQLite (com pico e média diária) para **Claude Code**, **OpenAI Codex**, **Google Antigravity**, **OpenRouter** e **OpenCode Go**, protegendo chaves opcionais no Windows Credential Manager (`keyring`).

### 10. Áudio, Bandeja do Sistema (Tray Flyout) e Trilha de Auditoria
Flyout compacto na bandeja do sistema com telemetria instantânea, barras duplas de cotas de IA, **Mixer de Volume Core Audio por aplicativo** (`ISimpleAudioVolume`, mute individual e troca de dispositivo de entrada/saída) e módulo dinâmico de Debug/Auditoria local exportável.

## Como funciona

1. **Baixe o instalador** — um único executável NSIS compacto (instalador NSIS de 3,5 MB), configurado para instalação no usuário atual sem exigir elevação de administrador para todo o sistema.
2. **Instale em segundos** — a instalação é local, fecha instâncias anteriores automaticamente em upgrades e utiliza o WebView2 nativo do Windows 11.
3. **Abra e leia o sistema** — o painel carrega a telemetria da máquina imediatamente e fica disponível na bandeja do sistema.

## Requisitos

- Windows 11 x64.
- WebView2 Runtime (já incluído nas versões atuais do Windows 11).
- Pegada leve: < 80 MB de RAM em uso e ~0% de CPU em repouso.

## Privacidade por arquitetura

A privacidade do Pulse não é uma promessa de política: é uma consequência de como o aplicativo foi construído. Não existe servidor para onde enviar dados, porque não existe servidor.

- Sem conta e sem login.
- Sem telemetria: nenhuma leitura do sistema é transmitida para fora.
- Sem dados fictícios: sensor sem suporte aparece como Indisponível.
- Sem ações destrutivas: nada é apagado ou alterado sem confirmação explícita.

## Links

- Landing page: https://pulse.lagrimante.com.br/
- Manifesto de release (JSON): https://pulse.lagrimante.com.br/release.json
- Código-fonte: https://github.com/LucasLagrimante/pulse
- Autor: https://lucaslagrimante.github.io/
