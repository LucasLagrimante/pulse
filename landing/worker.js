/**
 * pulse-lp · Worker de assets estáticos
 *
 * A landing page do Pulse é estática (Vite). O Worker existe para dois motivos:
 *  1. servir a variante Markdown (`/index.md`) quando o cliente pede
 *     `Accept: text/markdown` — descoberta por agentes e rastreadores de IA;
 *  2. devolver um 404 de verdade (e amigável a agente) para caminhos inexistentes,
 *     em vez do soft-404 que um catch-all de SPA produziria.
 *
 * O instalador (.exe) não é servido daqui: `/download/*.exe` redireciona para o
 * asset da GitHub Release fixado em `release.json` (`installer.assetUrl`), para
 * que o binário baixado seja sempre o do SHA-256 exibido na página.
 */

const MARKDOWN_ACCEPT = /(?:^|,)\s*text\/markdown(?:\s*[;,][^,]*)?/i;
const SITE = "https://pulse.lagrimante.com.br";
const RELEASES_URL = "https://github.com/LucasLagrimante/pulse/releases";

const NOT_FOUND_MARKDOWN = `# Pulse

## 404 — página não encontrada

O recurso solicitado não existe neste domínio.

- [Página inicial](${SITE}/)
- [Download para Windows](${SITE}/#download)
- [Sitemap](${SITE}/sitemap.xml)
- [llms.txt](${SITE}/llms.txt)
- [Versão em Markdown](${SITE}/index.md)
`;

// Resolve a URL do instalador pela versão publicada em release.json; se o
// manifesto faltar ou vier inválido, cai na página da última Release.
async function installerRedirect(request, env) {
  let target = `${RELEASES_URL}/latest`;
  try {
    const manifestResponse = await env.ASSETS.fetch(new Request(`${SITE}/release.json`, { method: "GET" }));
    if (manifestResponse.ok) {
      const assetUrl = (await manifestResponse.json())?.installer?.assetUrl;
      if (typeof assetUrl === "string" && assetUrl.startsWith(`${RELEASES_URL}/download/`)) {
        target = assetUrl;
      }
    }
  } catch {
    // Mantém o fallback para a página da última Release.
  }
  return new Response(null, {
    status: 302,
    headers: { Location: target, "Cache-Control": "public, max-age=300" },
  });
}

function markdownHeaders() {
  return {
    "Content-Type": "text/markdown; charset=utf-8",
    "Vary": "Accept, Accept-Encoding",
  };
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const accept = request.headers.get("Accept") || "";

    // Negociação de conteúdo: cliente que aceita Markdown recebe o index.md na raiz.
    if (url.pathname === "/" && MARKDOWN_ACCEPT.test(accept)) {
      const markdownResponse = await env.ASSETS.fetch(new Request(`${SITE}/index.md`, request));
      if (markdownResponse.ok) {
        const headers = new Headers(markdownResponse.headers);
        Object.entries(markdownHeaders()).forEach(([key, value]) => headers.set(key, value));
        headers.set("Link", `<${SITE}/index.md>; rel="alternate"; type="text/markdown"`);
        return new Response(markdownResponse.body, {
          status: 200,
          statusText: "OK",
          headers,
        });
      }
    }

    if (url.pathname.startsWith("/download/") && url.pathname.endsWith(".exe")) {
      return installerRedirect(request, env);
    }

    const response = await env.ASSETS.fetch(request);

    if (response.status === 404) {
      if (MARKDOWN_ACCEPT.test(accept)) {
        return new Response(NOT_FOUND_MARKDOWN, { status: 404, headers: markdownHeaders() });
      }
      return response;
    }

    // A raiz anuncia a variante Markdown para quem faz descoberta por header Link.
    if (url.pathname === "/") {
      const headers = new Headers(response.headers);
      headers.set("Link", `<${SITE}/index.md>; rel="alternate"; type="text/markdown"`);
      headers.set("Vary", "Accept, Accept-Encoding");
      return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
    }

    return response;
  },
};
