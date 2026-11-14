# AGENTS.md — Landing Page do Pulse (`landing/` / `pulse-lp`)

Orientações para agentes de IA trabalhando na Landing Page oficial do **Pulse** (`https://pulse.lagrimante.com.br/`).

## Arquitetura Unificada

A Landing Page agora reside de forma unificada em `landing/` (neste repositório) e é espelhada automaticamente em `../pulse-lp` pelo script `scripts/sync-landing.mjs`.

- **Aplicação (`pulse`)**: `~/projects/pulse` (WSL). App nativo Windows 11 (Rust + Tauri 2 + React 19).
- **Build & Sincronização Automática**: Rodar `npm run build:native` (ou `npm run lp:sync` na raiz do `pulse`) lê o instalador NSIS do bundle (`src-tauri/target/release/bundle/nsis/Pulse_<versão>_x64-setup.exe`), recalcula tamanho em MB e hash `SHA-256`, atualiza `index.html`, `public/index.md`, `public/llms.txt` e `public/release.json` (com `installer.assetUrl` da GitHub Release), compila `dist/`, executa `check-seo.mjs` e sincroniza `../pulse-lp`.
- **Regra de Paridade**: Qualquer nova funcionalidade ou módulo criado no aplicativo Pulse deve ser documentado e refletido no simulador e nas seções de recursos da Landing Page.

## Identidade Visual (Herdada do App)

A landing page utiliza os tokens do sistema neumórfico mineral (`src/identity.css` do app):
- Superfícies: Porcelain (`#e6edf5`) no claro, Obsidian (`#181b20`) no escuro.
- Acento âmbar: `#d97706` (claro) / `#f59e0b` (escuro).
- Texto sobre âmbar no tema escuro é sempre escuro (`#141619`).
- Contraste WCAG AA obrigatório em ambos os temas. Proibido usar gradiente em texto.

## Diretrizes Obrigatórias

1. **Idioma**: português brasileiro em respostas, comentários e textos da interface.
2. **SEO + Descoberta por IA**: `robots.txt`, `llms.txt`, `sitemap.xml`, `index.md`, `release.json`, `og-image.png` (1200×630) e JSON-LD (`WebSite` + `SoftwareApplication` + `FAQPage`) são obrigatórios e verificados por `scripts/check-seo.mjs`.
3. **Download**: aponta para `/download/Pulse-latest-x64-setup.exe`, que o `worker.js` redireciona (302) para o asset da GitHub Release fixado em `release.json`; o checksum `SHA-256` exibido é o desse arquivo. Nunca versionar `.exe` neste repositório.
4. **Temas**: alternância claro/escuro persistida em `localStorage` (`pulse-theme`) e aplicada no `<head>` antes do primeiro paint.
5. **Honestidade Visual**: o simulador interativo da interface é esquemático (`figcaption` explícito) e demonstra fielmente os 9 módulos da janela principal mais o Tray Flyout com Mixer Core Audio.
