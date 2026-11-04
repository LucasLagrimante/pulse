# Pulse — Landing Page

Landing page oficial do **Pulse**, central de observabilidade e diagnóstico para
Windows 11.

**Site:** https://pulse.lagrimante.com.br/

## Sobre o produto

O Pulse reúne em um único aplicativo nativo o que hoje exige cinco ferramentas
separadas: monitor de recursos, gerenciador de tarefas, analisador de disco,
inventário de inicialização e painel de diagnóstico.

Todo o processamento é local. Não há conta de usuário, não há telemetria para a
nuvem e nenhuma leitura do sistema é transmitida para fora. Métricas sem suporte
do hardware aparecem como **Indisponível**, nunca como zero fictício.

## Stack

- HTML estático + CSS (tokens de tema claro/escuro) + JavaScript sem framework.
- [Vite](https://vite.dev/) para o build.
- [Cloudflare Workers](https://developers.cloudflare.com/workers/) com assets
  estáticos para servir o site, incluindo a variante Markdown para agentes.

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
| `index.html` | Landing completa: SEO, Open Graph, JSON-LD e FAQ |
| `src/base.css` | Tokens de tema (Porcelain/Obsidian) e reset |
| `src/components.css` | Cabeçalho, hero, cards, FAQ e rodapé |
| `src/main.js` | Alternância de tema, FAQ e revelação ao rolar |
| `worker.js` | Worker: Markdown para agentes, 404 real, headers do instalador |
| `public/` | `robots.txt`, `llms.txt`, `sitemap.xml`, `index.md`, `og-image.png`, `404.html` |
| `public/release.json` | Versão, tamanho, SHA-256 e URL do instalador na GitHub Release |

## Deploy

Cada push na branch `main` dispara o GitHub Actions, que faz o build e publica
na Cloudflare (Worker `pulse-lp`) no domínio `pulse.lagrimante.com.br`.

O secret `CLOUDFLARE_API_TOKEN` precisa estar configurado no repositório.

## Autor

[Lucas Lagrimante](https://lucaslagrimante.github.io/) ·
[GitHub](https://github.com/LucasLagrimante/pulse)
