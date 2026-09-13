# Pulse — análise de engenharia e evolução

Data: 11/09/2026. Escopo: checkout Windows atual, incluindo alterações ainda não commitadas sobre `d8f0236`. Após a revisão, foi implementado o pacote P1 de confiabilidade: CPU inicial com leitura real, VRAM e temperatura explicitamente indisponíveis quando a fonte WMI não é confiável, limites de I/O do TreeSize, impacto de boot apenas quando medido e estado StartupApproved preservando o comando original.

> Atualização de implementação e validação: o texto histórico abaixo registra os achados originais. Nesta continuação, RAM, disco e CPU passaram a aceitar ausência sem converter falha em `0`; capacidades de unidades também ficam ausentes quando o Windows não as informa. O inventário preserva entradas distintas de HKCU e HKLM, e a desativação grava o FILETIME exigido pelo `StartupApproved`. A análise de disco agora marca no cache resultados parciais por cancelamento, limite ou prazo, e exclui agrupamentos sintéticos do ranking de arquivos reais. Validação atual: `npm run verify`, 46 testes Vitest, 26 testes Rust e Clippy sem warnings.

## Parecer

**Recomendo um upgrade adicional, começando pela confiabilidade dos dados e pela coleta.** A identidade visual já oferece uma base consistente. A distância para a proposta de central de diagnóstico está principalmente no conteúdo: o Pulse mostra o estado atual, mas ainda explica pouco sobre sua origem, duração, impacto e mudanças ao longo do tempo.

O produto tem uma boa estrutura de navegação, ações identificáveis, tratamento de ausência em gráficos, recuperação de erros e ferramentas locais úteis. Ainda não há evidência suficiente para considerá-lo substituto funcional do Monitor de Recursos, Autoruns ou Visualizador de Eventos. Também não há medição suficiente para certificar a meta de CPU em repouso e Working Set inferior a 80 MB.

## Validação realizada e limites

| Verificação | Resultado |
| --- | --- |
| TypeScript, Vitest e bundle Vite | Execução final aprovada: 42 testes (30 de App e 12 de Charts), TypeScript e build; JS 291,61 kB / gzip 87,58 kB, CSS 56,22 kB / gzip 11,21 kB |
| `cargo test --manifest-path src-tauri/Cargo.toml` | 25 testes aprovados, incluindo compilação nativa MSVC |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | Reprovado: 15 diagnósticos na biblioteca, 17 no alvo com testes; não somar como 32 problemas independentes |
| `git diff --check` | Sem erros de whitespace; avisos de conversão LF/CRLF |
| Coleta nativa do checkout | Executadas duas leituras de overview e uma de hardware por um executável auxiliar temporário, depois removido |
| GPU independente | Comparação local com `nvidia-smi`, somente leitura |
| Interface | Navegação e capturas em navegador, com fixture explícita, em 1200 × 780 e 960 × 640; varredura simulada e filtro de maiores arquivos |
| Instalação e integração Windows | Não certificadas nesta rodada: NSIS, troca real de autostart, encerramento de processos, clique no tray, múltiplos monitores e suspensão/retomada |

O erro de renderização impresso pelo Vitest é provocado pelo teste de CrashBoundary. Os testes passaram; isso não equivale a uma execução sem saída em stderr. O teste Rust do tray contém `assert!(true)`, portanto seu sucesso não valida o comportamento da bandeja.

Houve atualizações de `App.tsx`, `Charts.tsx` e CSS durante a revisão, observadas pelo Vite, externas a este trabalho. As primeiras capturas foram descartadas e a validação web foi repetida. O resultado vale para os arquivos observados, não para futuras alterações no checkout.

### Evidência nativa

| Leitura | Resultado observado |
| --- | --- |
| Primeiro overview em processo novo | 1.347 ms; CPU 0%; RAM 26,9 / 31,9 GiB; disco 840,2 / 930,5 GiB; 15 processos; 27 entradas de boot |
| Segundo overview, imediatamente após o primeiro | Cache: duração arredondada para 0 ms; CPU 36,9% |
| Hardware | 570 ms; RTX 4070 SUPER; VRAM 4.293.918.720 bytes, isto é, 4.095 MiB; temperatura indisponível |
| `nvidia-smi --query-gpu=name,memory.total,temperature.gpu --format=csv,noheader` | RTX 4070 SUPER; 12.282 MiB; 67 °C |

São amostras pontuais em build de desenvolvimento, durante trabalho na máquina, sem validade como benchmark de repouso. A segunda amostra de CPU tem intervalo muito curto; serve para mostrar a transição do estado inicial, não como referência de carga sustentada. O processo instalado foi encontrado em outro caminho; seu consumo isolado não certifica este checkout nem inclui todos os processos WebView2.

## Correções prioritárias

P1: resolver antes de ampliar funcionalidades ou apresentar métricas como confiáveis. P2: corrigir na sequência para robustez e clareza. Os achados de código abaixo têm mecanismos identificáveis; nem todos foram reproduzidos por interação nativa.

### P1 — A primeira CPU e falhas de coleta podem virar zero

Em `src-tauri/src/system_overview.rs`, `OVERVIEW_SCRIPT` inicializa CPU/RAM/disco/boot com zero. `read_cpu_percent()` retorna `None` na primeira chamada, pois ainda precisa guardar a referência para calcular o delta. A sobreposição posterior preserva o zero do script. Isso foi reproduzido no probe nativo.

Falhas de memória e disco também preservam os placeholders. `DriveInfo`, em `disk_analyzer.rs`, usa `unwrap_or(0)` e cria unidades com capacidade zero no fallback. O frontend não consegue distinguir esses casos de uma medição real.

**Correção:** `Capability<T>`/`Option` por métrica, estado de aquecimento para CPU, motivo e instante de coleta. Preservar uma amostra anterior apenas com indicação de desatualização. Aceite: processo novo, falha de API e unidade sem mídia nunca produzem zero fictício.

### P1 — VRAM incorreta na máquina real

`src-tauri/src/performance.rs` lê `Win32_VideoController.AdapterRAM`. A Microsoft documenta o campo como `uint32`: ele não representa capacidades de 4 GiB ou superiores. O cast para `int64` no PowerShell não recupera informação que a origem não representa. A comparação local encontrou 4.095 MiB no Pulse e 12.282 MiB no driver.

**Correção:** enumerar adaptadores com DXGI e ler memória dedicada/compartilhada separadamente; identificar o adaptador escolhido. A estrutura `DXGI_ADAPTER_DESC` expõe `DedicatedVideoMemory` como `SIZE_T`. Fonte oficial: [Win32_VideoController](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-videocontroller) e [DXGI_ADAPTER_DESC](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_adapter_desc).

A temperatura não está entre as propriedades documentadas de `Win32_VideoController`. Consultar `CurrentTemperature` nessa classe não constitui uma integração confiável de sensores. A NVIDIA mostrou temperatura disponível em sua própria ferramenta; isso demonstra uma oportunidade de provedor adicional, não suporte universal. Preservar indisponibilidade honesta para drivers sem provedor suportado.

### P1 — TreeSize não aplica o orçamento de I/O do projeto

Em `disk_analyzer.rs`, `start_disk_scan()` permite profundidade de 1 a 10, com padrão 6; o frontend solicita 6. A recursão não tem quota global de 2.500 arquivos, deadline de 1,5 s por raiz nem `yield_now()` a cada 15 entradas. Usa `spawn_blocking`, o que protege o executor, mas não limita o trabalho de filesystem.

Os 40 filhos por pasta limitam a árvore final, depois de construir e ordenar `direct_children`. Isso não limita a memória transitória nem a quantidade de leituras. O risco de competição de I/O é sustentado pelo código; não foi feito um stress test no disco pessoal nem uma captura ETW de DPC.

**Correção:** orçamento compartilhado, interrupção cooperativa, contadores de itens ignorados e limite global de nós/bytes de IPC. Aplicar as regras atuais do AGENTS. Se o produto precisar de análise profunda completa, primeiro definir um contrato específico para uma varredura explicitamente solicitada, com orçamento mensurado e retomada; não remover silenciosamente a proteção existente.

### P1 — “Impacto no boot” mistura observação com palpite

`startup_inventory.rs::determine_impact()` marca Discord, Steam e outros como alto impacto pelo nome; programas desconhecidos recebem baixo impacto. A interface apresenta a classificação como impacto detectado e participa do contador de aplicativos mais lentos, mesmo quando `startup_time_ms` é `None`.

**Correção:** sem evento, mostrar “Não medido”. Se uma estimativa editorial for mantida, identificá-la como estimativa e separá-la dos rankings medidos. Associar evento, data, origem e sessão de boot. O parser atual extrai XML por linhas/substrings e casa nomes por `contains`; substituir por parsing estrutural e identidade mais precisa. Aceite: aplicativo desconhecido nunca recebe “baixo impacto” por ausência de evidência; eventos de boots diferentes não parecem uma única medição atual.

### P1 — Controle de autostart perde a identidade da origem

O inventário junta HKCU/HKLM em mapas indexados somente por nome e descarta duplicatas. `set_startup_item_enabled(name, enabled)` grava sempre em HKCU, embora entradas HKLM sejam apresentadas como editáveis. O resultado pode representar a preferência de outra origem e não comprova que o Windows aplicou a alteração pretendida.

O controle do próprio Pulse em `startup.rs` ainda executa `reg delete` para desativar, contrariando a regra explícita do repositório. `get_startup_registration()` verifica a presença em Run, sem considerar StartupApproved; portanto pode divergir de uma desativação feita pelo Gerenciador de Tarefas. O registro de desativação dos demais itens também usa timestamp zerado, em desacordo com o contrato do AGENTS.

**Correção:** identidade por origem, visão do registro e nome; capacidade de alteração por entrada; releitura após a operação; respeitar StartupApproved também para o Pulse. Validar reversibilidade em entradas de teste isoladas. Nenhuma chave real foi alterada nesta revisão.

### P2 — Cancelamento e dados parciais de disco parecem conclusão normal

A recursão retorna uma árvore parcial ao observar `CANCEL_FLAG`, mas a função externa publica “Varredura concluída” e resultado sem motivo de incompletude. Falhas de acesso viram nós de zero bytes. Além disso, os grupos sintéticos de “Outros itens” têm `is_dir: false`, e `extractTopFiles()` inclui todo nó não diretório: um agregado pode aparecer como arquivo gigante.

**Correção:** estados `running/completed/cancelled/partial/failed`, razão de truncamento e contagem de erros; tipo explícito `file/directory/aggregate`. Excluir agregados do ranking de arquivos reais. Construir o ranking global antes das podas, pois hoje ele só considera a árvore retida. Preservar seleção e expansão durante progresso: `applyScanResult()` redefine foco e expansão para a raiz em cada snapshot.

### P2 — Coleta duplicada, sem prazo e sem estado de frescor suficiente

`App` e `TrayFlyout` usam intervalos próprios de 5 s sem política de visibilidade nem trava de requisição em andamento. O cache de overview libera o mutex antes de coletar: duas chamadas frias podem executar a mesma coleta. Há chamadas síncronas de comandos Tauri que executam `reg.exe`, `wevtutil.exe` e PowerShell; seus `.output()` não têm timeout explícito.

**Correção:** coordenador no Rust, uma coleta em andamento por fonte, cache compartilhado e assinatura pelas janelas. Hardware estático deve ter cache longo; telemetria deve seguir demanda e estado da janela. Dar prazo aos subprocessos ou substituí-los por APIs nativas. Ao falhar, registrar lacuna/idade da última medição. Aceite: atraso maior que o intervalo não acumula processos de coleta; ocultar ambas as janelas reduz o trabalho conforme política documentada.

### P2 — Arquivos recentes são uma amostra enviesada e silenciosa

`recent_files.rs` interrompe ao encontrar 200 resultados antes de ordenar por data. Uma pasta inicial pode consumir o orçamento e impedir que arquivos mais novos em outras raízes apareçam. O deadline atual é único para todas as raízes, embora o AGENTS descreva 1,5 s por raiz. As pastas padrão são montadas por nomes abaixo de USERPROFILE e podem não acompanhar redirecionamento/OneDrive. Há ainda uma pasta pessoal fixa como semente de configuração.

**Correção:** distribuir o orçamento entre raízes, mostrar cobertura/incompletude, manter os mais recentes entre os itens visitados, resolver pastas conhecidas do Windows e validar pastas adicionadas. O cache de recentes é invalidado ao alterar pastas — esse ponto já está atendido. Não anunciar busca completa do computador.

## Profundidade por módulo

Avaliação qualitativa baseada no código e nos fluxos existentes; não é uma pontuação de mercado.

| Módulo | Estado atual | Evolução que mais agrega |
| --- | --- | --- |
| Visão geral | Bom resumo visual, pouca explicação temporal | Resumo de mudanças e acesso à evidência; fonte e idade da amostra |
| Atividade | Raso para diagnóstico: até 15 processos por RAM | CPU e I/O por processo, busca no inventário completo, árvore pai/filho, caminho e instante de criação |
| Desempenho | CPU/RAM úteis; GPU praticamente inventário | Disco ativo/latência, rede por interface, GPU/VRAM corretas e histórico com timestamps |
| Armazenamento | Módulo mais explorável; análise parcial pouco explícita | Cobertura, cancelamento correto, ranking real e comparação de snapshots |
| Arquivos recentes | Conveniente, mas não um histórico de atividade | Explicar “modificados recentemente”; cobertura de raízes e filtros consistentes |
| Inicialização e segurança | Inventário parcial de Run com toggles | Boot medido, origem inequívoca, pastas Startup e tarefas em leitura; evidências de assinatura sob demanda |
| Debug | Diagnóstico interno do Pulse, buffer de 200 entradas | Manter como ferramenta do app; criar visão separada de eventos relevantes do Windows |
| Tray | Boa proposta de acesso rápido | Coleta por demanda, estado desatualizado e posicionamento por área útil/origem do monitor |

O nome “Inicialização e segurança” promete mais que o inventário atual. Não há aqui uma verificação de assinatura ou mecanismo de inspeção suficiente para inferir segurança. Metadados `CompanyName` do executável não equivalem a assinatura validada. Ao evoluir, apresentar evidências e permissões, sem rotular arquivos arbitrariamente como malware.

## Direção criativa: investigar o que mudou

O diferencial mais promissor é permitir selecionar um período e responder: **“O que estava acontecendo quando meu computador ficou lento?”** Isso exige histórico curto confiável, correlação e navegação contextual.

Fluxo proposto:

1. A pessoa inicia “Investigar por 60 segundos”. O Pulse informa quais fontes consegue coletar e o custo esperado.
2. A linha do tempo registra CPU, memória, latência/atividade de disco e principais processos, com lacunas visíveis.
3. Ao selecionar um pico, a interface mostra quais processos cresceram e os eventos próximos, com fonte e horário.
4. Uma explicação local baseada em regras descreve a coincidência observada. Só usar linguagem causal quando houver evidência suficiente.
5. A pessoa abre o processo, a pasta ou o evento relacionado; pode exportar um relatório local, revisando caminhos e nomes incluídos.

Exemplos de evolução complementar: comparar ocupação de uma pasta entre duas análises; comparar boots medidos antes/depois; destacar um processo cuja memória cresceu durante a sessão. Alertas devem exigir duração mínima e evitar repetição. Não criar uma nota genérica de “saúde” que esconda fontes, nem sugerir limpeza de RAM ou desativação automática de serviços.

## Arquitetura recomendada e ordem de entrega

### Etapa 1 — Verdade e previsibilidade

Corrigir métricas opcionais, VRAM, boot heurístico, autostart e limites/cancelamento de disco. Criar testes dos casos negativos realmente encontrados, incluindo ausência de API, início da CPU, nomes iguais em HKCU/HKLM e nó sintético no ranking. Esta etapa deve preceder novos painéis.

### Etapa 2 — Uma coleta compartilhada e Atividade útil

Separar shell, módulos, contratos e formatadores. Hoje `App.tsx` concentra UI, estado, polling e ações, e Charts importa `metric` de App enquanto App importa Charts. Resolver essa dependência e evitar colocar todo histórico no estado do shell. Os modelos `domain::Snapshot/Capability` existem, mas não são o contrato comum dos coletores; aproveitar a estrutura depois de unificá-la.

Criar coordenador de coleta com deduplicação, timeout e cadência por consumidor. Adicionar CPU por processo e I/O, identidade PID + instante de criação para evitar agir sobre PID reutilizado. Inventário completo pode ser consultado sob demanda, sem tornar cada atualização uma varredura de assinaturas ou versões de executáveis.

### Etapa 3 — Histórico local e investigação guiada

Persistência limitada por tempo/espaço, agregação e retenção explícita. O `Repository` atual abre SQLite em memória e cria apenas a estrutura de migração; não há histórico persistente de telemetria ligado aos coletores. Portanto esse recurso precisa ser construído, não apenas habilitado.

Começar por janela curta em memória e exportação manual. Depois incluir snapshots e comparação. Evitar gravar todo processo a cada segundo indefinidamente. As integrações de disco/rede/GPU precisam de protótipos de fonte e custo antes de prometer suporte universal.

### Etapa 4 — Cobertura de Windows e acabamento

Ampliar boot para outras origens em modo leitura, acrescentar eventos relevantes do Windows, validar assinatura sob demanda e explorar múltiplos monitores/DPI. No tray, o cálculo atual usa tamanho do monitor, mas não sua origem e área útil; validar monitor secundário e barra de tarefas antes de considerar o posicionamento nativo concluído.

Não há evidência nesta análise para justificar uma atualização indiscriminada de React, Vite ou Tauri. O build atual passa. Revisão de versões, advisories e dependências transitivas seria uma atividade própria, com consulta atual e testes de compatibilidade.

## Tema e usabilidade

Preservar grafite, âmbar e azul mineral, ícones discretos e estrutura lateral. A janela mínima mantém títulos, navegação e ações legíveis nas capturas. Há suporte a foco visível e movimento reduzido no CSS. A rolagem interna funciona no smoke test; em armazenamento, documento e viewport ficaram ambos em 960 × 640, sem overflow global.

O próximo ganho visual deve ser densidade informativa. No armazenamento a barra de controles ocupa quase toda a área visível na janela mínima; agrupar opções secundárias deixaria o resultado aparecer mais cedo. Em desempenho, CPU/RAM ocupam a primeira dobra e hardware fica abaixo; é aceitável, mas uma faixa compacta de resumo ajudaria a comparar recursos.

A árvore e o treemap usam elementos clicáveis genéricos: completar navegação por teclado e semântica de árvore antes de certificar acessibilidade. Não foi feito laudo de contraste, leitor de tela ou DPI. Algumas regras de scroll em `identity.css` substituem as de `App.css`; consolidar estilos reduz regressões de layout. Os textos misturam “Tree Window”, “Treemap” e português; uniformizar nomes conforme o público.

Capturas com dados artificiais de teste, sem valor de medição do hardware:

- [Visão geral — 1200 × 780](overview-1200.png)
- [Desempenho — 960 × 640](performance-960.png)
- [Armazenamento — 960 × 640](storage-960.png)

## Critérios para liberar o próximo upgrade

- Testes frontend/Rust e build aprovados; Clippy estrito saneado ou exceções justificadas individualmente.
- Contratos distinguem medido, aquecendo, indisponível e desatualizado. Zero válido continua sendo exibido.
- CPU inicial, falha de hardware, erro de acesso, cancelamento e limites de scan têm testes de comportamento.
- Autostart validado com entradas descartáveis, origens duplicadas e releitura; não usar aplicações pessoais como cobaia.
- Benchmark em release: 10 minutos com janela aberta, 10 minutos oculto, flyout aberto/fechado e scan limitado; contabilizar Pulse, WebView2 e subprocessos, com CPU normalizada e memória identificada por tipo.
- Registrar latência das coletas e ações, crescimento de memória, número de subprocessos e comportamento após suspensão/retomada. Para atribuir travamentos a DPC, usar evidência apropriada de tracing; `yield_now()` sozinho não prova sua ausência.
- Testar 960 × 640, 1200 × 780, DPI 125/150/200%, teclado, múltiplos monitores e lista volumosa.
- Smoke test do instalador e das integrações nativas da mesma revisão que foi compilada. Instalação anterior e screenshot web não substituem essa validação.

**Decisão recomendada:** preservar o tema, entregar primeiro um ciclo de confiabilidade e coleta, depois aprofundar Atividade/Desempenho e construir investigação temporal. Essa sequência aproxima o Pulse da proposta de explicar o Windows sem comprometer sua leveza e transparência.
