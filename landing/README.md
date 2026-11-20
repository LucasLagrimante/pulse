# Pulse — Landing Page

Landing page oficial do **Pulse**, central nativa de observabilidade e diagnóstico
para **Windows 11** e **Linux** (com port experimental para **macOS** em desenvolvimento).

**Site:** https://pulse.lagrimante.com.br/

## Sobre o produto

O Pulse reúne em um único aplicativo nativo o que antes exigia vários utilitários
separados: monitor de recursos, gerenciador de tarefas, analisador de disco
(TreeSize), limpeza segura, desinstalador com resíduos, inventário de inicialização,
serviços e atualizações, rede e segurança, mixer de áudio e cotas de IA.

Todo o processamento é local. Não há conta de usuário, não há telemetria para a
nuvem e nenhuma leitura do sistema é transmitida para fora. Métricas sem suporte
do hardware aparecem como **Indisponível**, nunca como zero fictício.

## Stack

- HTML estático + CSS (tokens de tema claro/escuro) + JavaScript sem framework.
- [Vite](https://vite.dev/) para o build.
- [Cloudflare Workers](https://developers.cloudflare.com/workers/) com assets
  estáticos para servir o site, a variante Markdown para agentes e os
  redirecionamentos `/download/*` (`.exe`, `.deb`, `.AppImage`, `.dmg`) para as
  GitHub Releases.

## Desenvolvimento

```bash
npm install
npm run dev       # servidor de desenvolvimento
npm run build     # gera ./dist
npm run preview   # prévia local do build
```

## Estrutura

| Caminho | Descrição |
| --- | --- |
| `index.html` | Landing completa: simulador, SEO, Open Graph, JSON-LD, download multiplataforma e FAQ |
| `src/base.css` | Tokens de tema (Porcelain/Obsidian) e reset |
| `src/components.css` | Cabeçalho, hero, simulador, cards, download multiplataforma, FAQ e rodapé |
| `src/main.js` | Alternância de tema, simulador, detecção de SO no download, cópia SHA-256 e FAQ |
| `worker.js` | Worker: Markdown para agentes, 404 real e redirect `/download/*` (.exe, .deb, .AppImage, .dmg) |
| `public/` | `robots.txt`, `llms.txt`, `sitemap.xml`, `index.md`, `og-image.png`, `404.html` |
| `public/release.json` | Versão, tamanho, SHA-256 e URLs dos instaladores na GitHub Release |

## Deploy

Cada push na branch `main` do repositório irmão `pulse-lp` dispara o GitHub Actions,
que faz o build e publica na Cloudflare (Worker `pulse-lp`) no domínio `pulse.lagrimante.com.br`.

O secret `CLOUDFLARE_API_TOKEN` precisa estar configurado no repositório `pulse-lp`.

## Autor

[Lucas Lagrimante](https://lagrimante.com.br/) ·
[GitHub](https://github.com/LucasLagrimante/pulse)
