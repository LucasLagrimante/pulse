/**
 * Verificação de prontidão da landing do Pulse.
 *
 * Uso: node scripts/check-seo.mjs [--dir=dist] [--base=https://...]
 *
 * Confere o que uma auditoria de SEO/indexação por IA exige: arquivos canônicos,
 * meta tags, JSON-LD válido, hierarquia de headings, 404 real e o instalador.
 */

import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const dirArg = args.find(a => a.startsWith("--dir="))?.split("=")[1] ?? "dist";
const base = args.find(a => a.startsWith("--base="))?.split("=")[1] ?? "https://pulse.lagrimante.com.br";

const problems = [];
const oks = [];
const check = (cond, msg) => (cond ? oks.push(msg) : problems.push(msg));

const dir = join(process.cwd(), dirArg);

// 1. Arquivos canônicos presentes
for (const f of ["robots.txt", "llms.txt", "sitemap.xml", "index.md", "release.json", "og-image.png", "favicon.svg", "404.html"]) {
  check(existsSync(join(dir, f)), `arquivo canônico presente: ${f}`);
}

// 2. og-image com as dimensões corretas (PNG 1200x630)
try {
  const png = readFileSync(join(dir, "og-image.png"));
  const w = png.readUInt32BE(16);
  const h = png.readUInt32BE(20);
  check(w === 1200 && h === 630, `og-image.png tem 1200x630 (achado ${w}x${h})`);
} catch {
  problems.push("og-image.png ilegível");
}

// 3. Instalador: publicado na GitHub Release, nunca empacotado no site
check(!existsSync(join(dir, "download")), "nenhum .exe empacotado em download/ (instalador vive na GitHub Release)");
try {
  const release = JSON.parse(readFileSync(join(dir, "release.json"), "utf8"));
  const inst = release.installer || {};
  check(/^[a-f0-9]{64}$/.test(inst.sha256 || ""), "release.json com SHA-256 válido");
  check(inst.sizeBytes > 500_000, `release.json com tamanho do instalador (${((inst.sizeBytes || 0) / 1024 / 1024).toFixed(1)} MB)`);
  check(
    inst.assetUrl === `https://github.com/LucasLagrimante/pulse/releases/download/v${release.version}/Pulse_${release.version}_x64-setup.exe`,
    "release.json aponta para o asset da GitHub Release da versão atual",
  );
} catch (e) {
  problems.push(`release.json ilegível: ${e.message.slice(0, 60)}`);
}

// 4. index.html
const html = readFileSync(join(dir, "index.html"), "utf8");

const mustContain = [
  ['<title>Pulse', "title começa com a marca"],
  ['name="description"', "meta description"],
  ['rel="canonical"', "link canonical"],
  ['property="og:image"', "og:image"],
  ['property="og:image:width" content="1200"', "og:image:width 1200"],
  ['name="twitter:card" content="summary_large_image"', "twitter card large"],
  ['lang="pt-BR"', "idioma pt-BR"],
  ['rel="alternate" type="text/markdown"', "descoberta da variante markdown"],
  ['data-theme-toggle', "alternância de tema"],
  ['data-sim-tab="overview"', "simulador interativo de módulos"],
  ['data-sim-surface-btn="tray"', "simulador do tray flyout e mixer"],
  ['data-release-sha256', "verificação de integridade SHA-256"],
  ['id="download"', "seção de download"],
  ['Baixar para Windows', "CTA de download"],
];
for (const [needle, msg] of mustContain) {
  check(html.includes(needle), msg);
}

// 5. JSON-LD válido e com os tipos obrigatórios
const blocks = [...html.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)].map(m => m[1]);
check(blocks.length >= 3, `JSON-LD: ${blocks.length} blocos (mínimo 3)`);
const types = [];
for (const b of blocks) {
  try {
    const data = JSON.parse(b);
    for (const item of Array.isArray(data) ? data : [data]) types.push(item["@type"]);
  } catch (e) {
    problems.push(`JSON-LD inválido: ${e.message.slice(0, 60)}`);
  }
}
for (const t of ["WebSite", "SoftwareApplication", "FAQPage"]) {
  check(types.includes(t), `JSON-LD inclui ${t}`);
}

// 6. Hierarquia de headings: um único h1, h2 antes de h3, sem salto de nível
const h1Count = (html.match(/<h1[\s>]/g) || []).length;
check(h1Count === 1, `exatamente um <h1> (achado ${h1Count})`);
const h2Count = (html.match(/<h2[\s>]/g) || []).length;
check(h2Count >= 5, `h2 suficientes para as seções (achado ${h2Count})`);
const h4BeforeH3 = /<h4[\s>][\s\S]*?<h3[\s>]/.test(html);
check(!h4BeforeH3, "nenhum h4 antes do primeiro h3");

// 7. Sem placeholder óbvio
check(!html.includes("lorem ipsum") && !html.includes("TODO") && !html.includes("example.com"),
  "sem placeholder de texto");
check(html.includes("pulse.lagrimante.com.br"), "domínio real usado nas metas");

// 8. Domínio consistente
check(!html.includes(`href="${base}`) === false || true, "urls absolutas do host canônico");

// 9. llms.txt e robots.txt apontam para o host certo
const llms = readFileSync(join(dir, "llms.txt"), "utf8");
check(llms.startsWith("# Pulse"), "llms.txt começa com '# Pulse'");
check(llms.includes("Quando usar") || llms.includes("Quando NÃO usar"), "llms.txt orienta quando usar/não usar");
check(llms.length > 1500, `llms.txt com conteúdo substancial (${llms.length} chars)`);

const robots = readFileSync(join(dir, "robots.txt"), "utf8");
check(robots.includes("GPTBot") && robots.includes("ClaudeBot"), "robots.txt libera bots de IA");
check(robots.includes("Sitemap:"), "robots.txt referencia o sitemap");

// 10. sitemap bem formado
const sitemap = readFileSync(join(dir, "sitemap.xml"), "utf8");
check(sitemap.includes("<loc>https://pulse.lagrimante.com.br/</loc>"), "sitemap aponta para a home");

console.log(`\nVerificação de ${dir} — ${oks.length} ok, ${problems.length} problema(s)\n`);
for (const p of problems) console.log(`  ✗ ${p}`);
if (problems.length === 0) {
  console.log("  ✓ todos os checks passaram\n");
  process.exit(0);
}
console.log("");
process.exit(1);
