# AGENTS.md — Landing Page do Pulse (`landing/` / `pulse-lp`)

Orientações para agentes de IA trabalhando na Landing Page oficial do **Pulse** (`https://pulse.lagrimante.com.br/`).

## Arquitetura Unificada

A Landing Page reside de forma unificada em `landing/` (neste repositório) e é espelhada automaticamente em `../pulse-lp` pelo script `scripts/sync-landing.mjs`.

- **Aplicação (`pulse`)**: `~/projects/pulse` (WSL/Linux/Windows). App nativo Windows 11 e Linux (macOS experimental) em Rust + Tauri 2 + React 19.
- **Build & Sincronização Automática**: Rodar `npm run build:native`, `npm run build:linux` ou `npm run lp:sync` lê os instaladores compilados disponíveis (`nsis/*.exe`, `deb/*.deb`, `appimage/*.AppImage`, `dmg/*.dmg`), recalcula tamanho em MB e `SHA-256`, atualiza `index.html`, `public/index.md`, `public/llms.txt` e `public/release.json`, compila `dist/`, executa `check-seo.mjs` e sincroniza `../pulse-lp`. Após release do CI, use `npm run lp:sync:release` para gravar os SHA-256 oficiais de todas as plataformas publicadas na GitHub Release.
- **Regra de Paridade**: Qualquer nova funcionalidade, plataforma ou módulo do aplicativo Pulse deve ser documentado e refletido no simulador e nas seções de recursos e download da Landing Page.

## Identidade Visual (Herdada do App)

A landing page utiliza os tokens do sistema neumórfico mineral (`src/identity.css` do app):
- Superfícies: Porcelain (`#e6edf5`) no claro, Obsidian (`#181b20`) no escuro.
- Acento âmbar: `#d97706` (claro) / `#f59e0b` (escuro).
- Texto sobre âmbar no tema escuro é sempre escuro (`#141619`).
- Contraste WCAG AA obrigatório em ambos os temas. Proibido usar gradiente em texto.

## Diretrizes Obrigatórias

1. **Idioma**: português brasileiro em respostas, comentários e textos da interface.
2. **SEO + Descoberta por IA**: `robots.txt`, `llms.txt`, `sitemap.xml`, `index.md`, `release.json`, `og-image.png` (1200×630) e JSON-LD (`WebSite` + `SoftwareApplication` + `FAQPage`) são obrigatórios e verificados por `scripts/check-seo.mjs`.
3. **Download Multiplataforma**: aponta para `/download/Pulse-latest-x64-setup.exe` (Windows), `/download/Pulse-latest-amd64.deb` e `/download/Pulse-latest-amd64.AppImage` (Linux), e `/download/Pulse-latest-macos.dmg` (macOS experimental quando publicado). O `worker.js` redireciona (302) para o asset correspondente da GitHub Release fixado em `release.json`. Nunca versionar binários neste repositório e nunca inventar SHA-256/tamanhos para plataformas ainda não publicadas.
4. **Temas**: alternância claro/escuro persistida em `localStorage` (`pulse-theme`) e aplicada no `<head>` antes do primeiro paint.
5. **Honestidade Visual**: o simulador interativo da interface é esquemático (`figcaption` explícito) e demonstra fielmente os 9 módulos da janela principal mais o Tray Flyout com Mixer de Áudio.
