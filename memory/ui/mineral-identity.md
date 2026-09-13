# Pulse: identidade mineral e correções de layout

- Direção aprovada para execução em 2026-09-11: o usuário liberou a troca de toda a paleta e pediu consistência entre marca interna, ícone Windows, bandeja e instalador.
- Paleta: grafite `#141619`, superfície `#1c2025`, azul mineral `#8da9bf`, âmbar `#dda76d`, texto quente `#f0ede6`.
- Símbolo compartilhado: `src/Identity.tsx` e fonte vetorial `public/pulse.svg`. Gerar os ícones com `npx tauri icon public/pulse.svg`; depois executar `scripts/installer-art.ps1` para as artes NSIS.
- `nav` sem escopo transferia `flex: 1` e margem superior às abas de armazenamento. Usar `.sidebar nav`.
- `.panel` impõe `flex: 1` e `height: 100%`: barras de ferramentas e painéis de configuração precisam de `flex: 0 0 auto; height: auto` para não crescer ou cortar os controles.
- Gráficos detalhados não cabem em linhas de grid que encolhem a zero. Preservar tamanho intrínseco e oferecer rolagem no workspace.
- Tabelas de disco precisam de rolagem horizontal própria; a grade de oito colunas não cabe na metade de uma janela de 960 px.
- Loading de abertura acompanha a primeira coleta, tem saída manual e respeita movimento reduzido. Não adicionar espera artificial. A bandeja reutiliza o símbolo e os botões cabem em 380 x 490.
- Não preencher históricos com amostras repetidas nem apresentar VRAM em 60% fixos. Ausência de dados no gráfico deve produzir lacunas ou estado indisponível.
- Verificação: 35 testes frontend, TypeScript e Vite aprovados. Navegação visual em navegador a 960 x 640 e 1200 x 780 com fixture explícita, sem fixture importada pelo produto. Flyout checado em 380 x 490. Build Rust/MSVC e NSIS gerados. A instalação interativa e sensores no WebView nativo não foram exercitados nesta revisão.
