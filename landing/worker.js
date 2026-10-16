/**
 * pulse-lp · Worker de assets estáticos
 *
 * A landing page do Pulse é estática (Vite). O Worker existe para dois motivos:
 *  1. servir a variante Markdown (`/index.md`) quando o cliente pede
 *     `Accept: text/markdown` — descoberta por agentes e rastreadores de IA;
 *  2. devolver um 404 de verdade (e amigável a agente) para caminhos inexistentes,
 *     em vez do soft-404 que um catch-all de SPA produziria.
 *
 * O instalador (.exe) é um arquivo estático servido pelo binding de assets.
 */

const MARKDOWN_ACCEPT = /(?:^|,)\s*text\/markdown(?:\s*[;,][^,]*)?/i;
const SITE = "https://pulse.lagrimante.com.br";

const NOT_FOUND_MARKDOWN = `# Pulse

## 404 — página não encontrada

O recurso solicitado não existe neste domínio.

- [Página inicial](${SITE}/)
- [Download para Windows](${SITE}/#download)
- [Sitemap](${SITE}/sitemap.xml)
- [llms.txt](${SITE}/llms.txt)
- [Versão em Markdown](${SITE}/index.md)
`;

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

    const response = await env.ASSETS.fetch(request);

    if (response.status === 404) {
      if (MARKDOWN_ACCEPT.test(accept)) {
        return new Response(NOT_FOUND_MARKDOWN, { status: 404, headers: markdownHeaders() });
      }
      return response;
    }

    // O instalador precisa de um Content-Type explícito: alguns clientes de
    // download interpretam application/octet-stream de forma inconsistente.
    if (url.pathname.startsWith("/download/") && url.pathname.endsWith(".exe")) {
      const headers = new Headers(response.headers);
      headers.set("Content-Type", "application/octet-stream");
      headers.set("Content-Disposition", `attachment; filename="${url.pathname.split("/").pop()}"`);
      headers.set("Cache-Control", "public, max-age=3600");
      return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
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
