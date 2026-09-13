# Revisão visual do Pulse

## Direção

Grafite e branco quente formam a base. Azul mineral identifica informação e âmbar marca seleção e ação. O símbolo combina uma moldura aberta com um sinal contínuo: é usado no shell, no loading, no painel rápido, nos ícones Windows e nas artes NSIS. Ícones vetoriais de traço substituem números de navegação e emojis de arquivos.

A animação desenha o sinal durante a primeira coleta. Não há espera artificial, porcentagem fictícia de inicialização ou dependência de rede. A pessoa pode continuar para o painel enquanto a coleta está pendente. Movimento reduzido desliga animações e transições.

## Correções verificadas

- Navegação lateral com estilos isolados: as abas de armazenamento não absorvem mais o espaço livre.
- Gráficos com dimensões intrínsecas e rolagem quando necessária, sem sobrepor rodapés ou indicadores.
- Nome completo da GPU pode quebrar linha.
- Barra de disco e configuração de pastas não herdam a altura integral dos painéis.
- Unidades, caminho, presets e botão de varredura se reorganizam sem sobreposição.
- Tabela extensa tem rolagem horizontal; o modo dividido empilha árvore e mapa em janelas menores.
- Painel rápido: ações inferiores inteiramente dentro dos 380 x 490; posição inferior medida em 473 px.
- Gráficos sem dados não desenham zero; lacunas são preservadas. A barra fixa de VRAM em 60% foi removida.

## Evidências

Capturas em navegador com dados de teste explícitos. Esses dados não integram o bundle do aplicativo.

- [Desempenho, 1200 x 780](performance-check.png)
- [Armazenamento / mapa](storage-check.png)
- [Visão geral, 960 x 640](overview-check.png)
- [Pastas e arquivos recentes, 960 x 640](recent-check.png)
- [Painel rápido, 380 x 490](tray-check.png)
- [Abertura](loading-check.png)
- [Debug](debug-check.png)

`npm run verify`: 35 testes aprovados, TypeScript e Vite aprovados. O teste existente de CrashBoundary provoca deliberadamente um erro de renderização para validar a recuperação. `git diff --check`: aprovado. Rust/MSVC e empacotamento NSIS compilados.

A navegação e a disposição dos componentes foram verificadas no navegador. A instalação interativa, os sensores reais e a interação com a bandeja do Windows não foram automatizados nesta revisão.

## Reprodução dos assets

1. `npx tauri icon public/pulse.svg`
2. `./scripts/installer-art.ps1`
3. `npx tauri build --bundles nsis`

O instalador é gerado em `src-tauri/target/release/bundle/nsis/Pulse_0.1.0_x64-setup.exe`.
