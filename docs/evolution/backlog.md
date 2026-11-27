# Backlog de evolução — Pulse

Fila única do ciclo diário (skill `pulse-evolution`, worktree `~/projects/pulse-auto`).

**Regras**
- Só o Lucas reordena, promove ou remove itens.
- O ciclo pega o primeiro item aberto do topo, entrega **uma etapa** por dia se o item for grande,
  marca `[x]` com o hash do commit e registra novos achados em "Descobertas".
- Item não executável hoje (depende de decisão) → registrar motivo e passar ao próximo.
- Toda fatia fecha com gate verde e commit próprio na branch `auto/pulse-evolution`.

## P0 — dívida estrutural (maior retorno)

- [ ] **P0-1 · App.tsx: 5.109 linhas → shell + views.**
  Extrair uma view por etapa (Visão geral, Atividade, Desempenho, Armazenamento, Inicialização,
  Debug), mantendo export público e testes verdes. Pronto quando: App.tsx < 1.500 linhas e
  `npm test` cobre as telas extraídas.
- [ ] **P0-2 · Cores hardcoded fora do design system.**
  `grep -rE '#[0-9a-fA-F]{3,8}' src --include=*.css --include=*.tsx` e substituir por tokens
  `--neu-*`/`--theme-*`. Violação direta do AGENTS.md §4.4. Pronto quando: zero hex em
  `background/color/border/stroke/fill` e os dois temas validados no QA visual.
- [ ] **P0-3 · App.css (6.034) + identity.css (4.917): separar por domínio.**
  Um arquivo por módulo (já existem `DataTable.css`, `PerformanceModule.css`, etc.);
  consolidar duplicatas e remover regras mortas. Pronto quando: nenhum CSS > 2.000 linhas e
  `npm run build` sem aviso de CSS novo.
- [ ] **P0-4 · FileExplorer.tsx (1.883 linhas) → split + testes.**
  Separar árvore, toolbar, preview e hooks; manter props públicas.
- [ ] **P0-5 · Rust: módulos > 2.500 linhas.**
  `subscription_usage.rs` (2.864), `custom_dock/dock_window.rs` (2.742),
  `peripheral_battery.rs` (2.668): extrair submódulos coesos, sem mudar comportamento.

- [ ] **P0-6 · Profundidade cortada: sombra clipada por container com `overflow`.**
  Regra em AGENTS.md §4.5. Runner já corrigido (janela opaca `680×520` recortada pelo DWM com
  bisel interno); flyout também. Falta varrer o resto: `.panel`/`.tall-panel` (App.css),
  `.fe-main`/`.fe-sidebar` (FileExplorer), `.dt-table` (DataTable), `.un-modal`,
  `.quota-modal-card`, `.disk-tree-section`, `.tray-flyout-container`.
  Pronto quando: nenhum filho com `--neu-shadow-raised*` estiver em container cujo padding seja
  menor que `|offset| + blur`, e as telas principais (Visão geral, Atividade, Armazenamento,
  Desempenho, Serviços) mostrarem a sombra terminando nos dois temas.

## P1 — qualidade e robustez

- [ ] **P1-1 · `unwrap()`/`expect()`/`panic!` em caminho de produção.**
  Inventariar por arquivo e converter comandos Tauri para `Result<_, String>` com erro real
  (`debug::record_failure`). Excluir testes e invariantes documentadas.
- [ ] **P1-2 · Cobertura de testes dos módulos críticos sem teste.**
  Prioridade: `disk_analyzer`, `services`, `startup_inventory`, `quota_history`, `tray`.
  Meta: um teste de comportamento por módulo/etapa, não cobertura cosmética.
- [ ] **P1-3 · Payload IPC.**
  Medir bytes de `get_system_overview` / `get_performance_details` / `start_disk_scan` e cortar
  campos não usados pelo frontend; memoizar seletores caros.
- [ ] **P1-4 · Estados de UI.**
  Inventariar as telas: loading (skeleton), vazio, erro, indisponível (com motivo), permissão
  negada. Padronizar cabeçalho, tabela e empty state com os componentes existentes.
- [ ] **P1-5 · Acessibilidade e teclado.**
  Foco visível, `Esc` em modais, ordem de tab, `aria-label` em ícones, contraste nos dois temas.

## P2 — performance e produto

- [ ] **P2-1 · Baseline de memória/CPU do app.**
  Medir WorkingSet e CPU em repouso (30 s) e com cada módulo aberto; registrar em
  `docs/evolution/metrics.jsonl`. Só medir com nenhuma instância aberta e sem elevação.
- [ ] **P2-2 · Tempo até primeiro paint.**
  Medir boot (instrumentação local, sem telemetria) e reduzir chamadas IPC escalonadas.
- [ ] **P2-3 · Bundle do frontend.**
  `du -sk dist` + análise de chunks; cortar imports pesados e código morto.
- [ ] **P2-4 · Densidade e despoluição visual.**
  Uma tela por rodada: remover ruído, agrupar ações, hierarquia tipográfica, alinhamento à grade.
- [ ] **P2-5 · Paridade de plataforma.**
  Auditar textos de `platformCopy()` e ações Windows-only (ocultar ou marcar indisponível).

## Descobertas

(itens novos entram aqui com data, evidência e impacto estimado; não promova sozinho)

- **2026-11-26 · P0-2 etapa 1 (4740bb0).** App.css linhas 1–208 (shell, sidebar, brand, nav,
  sidebar-foot, about-nav): 25 hex → tokens `--neu-*`. Contador `hex_hardcoded` 821 → 796.
  Achado: `identity.css` já sobrescreve `.app-shell`/`.sidebar`/`.brand`/`.nav-item`, mas
  `.nav-label`, `.nav-index`, `.brand-mark` e `.about-nav` ficavam só com a cor escura legada —
  eram ilegíveis no tema claro. Restam 632 hex em App.css (maioria em blocos de comando/shell);
  cada bloco de ~50 linhas fecha numa fatia.
- **2026-11-26 · QA visual.** `scripts/browser-fixture.js` não inclui `get_launcher_state`; a tela
  inicial "Continuar para o painel" precisa ser clicada para medir o shell real no headless.
- **2026-11-26 · Contraste no tema claro (reforça P1-5).** Sidebar light 1440: itens principais
  legíveis, mas subtítulo da marca, rótulo "MÓDULOS" e rodapé ("SEM NUVEM"/"SEM CONTA")
  ficaram com contraste abaixo de 4.5:1. Evidência: captura light-1440 do run smoke.
  Candidato: token de texto secundário mais escuro no tema claro + teste de contraste.
