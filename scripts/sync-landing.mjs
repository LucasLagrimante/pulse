// Sincroniza instaladores (Windows NSIS, Linux .deb/.AppImage e macOS .dmg
// experimental), metadados de versão/tamanho/SHA-256 e artefatos da Landing Page
// (em `landing/` e, se existir, no repo irmão `../pulse-lp`).
//
// Executado automaticamente ao final de `scripts/build-native.sh`,
// `scripts/build-linux.sh` ou via `npm run lp:sync`. Depois que o CI publica os
// instaladores na Release, use `npm run lp:sync:release` (lê os SHA-256 reais dos
// assets publicados na GitHub Release, nunca dos bundles locais).
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  REPO,
  buildReleaseManifest,
  matchReleaseAssets,
  restoreAssetFromEntry,
} from "./lib/release-assets.mjs";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const landingDir = join(rootDir, "landing");
const siblingLpDir = resolve(rootDir, "..", "pulse-lp");
const bundleRoot = join(rootDir, "src-tauri", "target", "release", "bundle");

const tauriConfPath = join(rootDir, "src-tauri", "tauri.conf.json");
const tauriConf = JSON.parse(readFileSync(tauriConfPath, "utf8"));
const version = tauriConf.version || "1.0.0";
const releaseJsonPath = join(landingDir, "public", "release.json");

// Remove cópias antigas de instaladores (landing local e espelho) para nenhum
// binário (.exe, .deb, .AppImage, .dmg) voltar ao Git.
for (const base of [landingDir, siblingLpDir]) {
  for (const sub of ["public", "dist"]) {
    rmSync(join(base, sub, "download"), { recursive: true, force: true });
  }
}
const trackedDownload = execFileSync("git", ["ls-files", "--", "landing/public/download"], {
  cwd: rootDir,
  encoding: "utf8",
}).trim();
if (trackedDownload) {
  console.error(
    `[sync-landing] ERRO: instalador ainda versionado no Git:\n${trackedDownload}\nRode: git rm --cached -r landing/public/download`,
  );
  process.exit(1);
}

function hashFile(filePath, assetName) {
  const buf = readFileSync(filePath);
  return {
    assetName,
    sizeBytes: buf.byteLength,
    sha256: createHash("sha256").update(buf).digest("hex"),
  };
}

function findLocalBundleFile(subDir, matcher) {
  const dir = join(bundleRoot, subDir);
  if (!existsSync(dir)) return null;
  const entries = readdirSync(dir);
  const matched = matcher(entries);
  if (!matched) return null;
  return hashFile(join(dir, matched), matched);
}

const previous = existsSync(releaseJsonPath) ? JSON.parse(readFileSync(releaseJsonPath, "utf8")) : null;
const previousSameVersion = previous?.version === version ? previous : null;

// --from-release: lê todos os instaladores já publicados na GitHub Release (build do CI)
// em vez dos bundles locais, que possuem outros SHA-256.
const fromRelease = process.argv.includes("--from-release");
let windows = null;
let linuxDeb = null;
let linuxAppImage = null;
let macosDmg = null;

if (fromRelease) {
  let assetNames = [];
  try {
    const info = JSON.parse(
      execFileSync("gh", ["release", "view", `v${version}`, "--repo", REPO, "--json", "assets"], {
        encoding: "utf8",
      }),
    );
    assetNames = (info.assets || []).map(a => a.name);
  } catch (error) {
    console.error(`[sync-landing] ERRO: não foi possível consultar a Release v${version}: ${error.message}`);
    process.exit(1);
  }

  const matched = matchReleaseAssets(assetNames, version);
  if (!matched.windows) {
    console.error(`[sync-landing] ERRO: Release v${version} sem o instalador Windows esperado (Pulse_${version}_x64-setup.exe).`);
    process.exit(1);
  }

  const toDownload = [
    ["windows", matched.windows],
    ["linuxDeb", matched.linuxDeb],
    ["linuxAppImage", matched.linuxAppImage],
    ["macosDmg", matched.macosDmg],
  ].filter(([, name]) => Boolean(name));

  const downloadDir = mkdtempSync(join(tmpdir(), "pulse-release-"));
  try {
    const results = {};
    for (const [key, assetName] of toDownload) {
      execFileSync(
        "gh",
        ["release", "download", `v${version}`, "--repo", REPO, "--pattern", assetName, "--dir", downloadDir],
        { stdio: "inherit" },
      );
      results[key] = hashFile(join(downloadDir, assetName), assetName);
    }
    windows = results.windows ?? null;
    linuxDeb = results.linuxDeb ?? null;
    linuxAppImage = results.linuxAppImage ?? null;
    macosDmg = results.macosDmg ?? null;
  } catch (error) {
    console.error(`[sync-landing] ERRO ao baixar assets da Release v${version}: ${error.message}`);
    process.exit(1);
  } finally {
    rmSync(downloadDir, { recursive: true, force: true });
  }
  console.log(
    `[sync-landing] Assets lidos da GitHub Release v${version}: ${toDownload.map(([, n]) => n).join(", ")}`,
  );
} else {
  windows =
    findLocalBundleFile("nsis", files => matchReleaseAssets(files, version).windows) ??
    restoreAssetFromEntry(previousSameVersion?.installer);
  linuxDeb =
    findLocalBundleFile("deb", files => matchReleaseAssets(files, version).linuxDeb) ??
    restoreAssetFromEntry(previousSameVersion?.linux?.deb);
  linuxAppImage =
    findLocalBundleFile("appimage", files => matchReleaseAssets(files, version).linuxAppImage) ??
    restoreAssetFromEntry(previousSameVersion?.linux?.appimage);
  macosDmg =
    findLocalBundleFile("dmg", files => matchReleaseAssets(files, version).macosDmg) ??
    restoreAssetFromEntry(previousSameVersion?.macos?.dmg);

  if (!windows) {
    console.error(
      `[sync-landing] ERRO: Pulse_${version}_x64-setup.exe não encontrado em src-tauri/target/release/bundle/nsis/ e release.json não é da versão ${version}. Rode npm run build:native.`,
    );
    process.exit(1);
  }

  const foundLabels = [
    windows && `Windows (${windows.assetName})`,
    linuxDeb && `Linux .deb (${linuxDeb.assetName})`,
    linuxAppImage && `Linux AppImage (${linuxAppImage.assetName})`,
    macosDmg && `macOS .dmg (${macosDmg.assetName})`,
  ].filter(Boolean);
  console.log(`[sync-landing] Metadados resolvidos para v${version}: ${foundLabels.join(" · ")}`);
}

const todayIso = new Date().toISOString().slice(0, 10);
const releaseManifest = buildReleaseManifest({
  version,
  updatedAt: todayIso,
  windows,
  linuxDeb,
  linuxAppImage,
  macosDmg,
});

const winSizeFormatted = releaseManifest.installer.sizeFormatted;
const winSha256 = releaseManifest.installer.sha256;
const debEntry = releaseManifest.linux?.deb ?? null;
const appImageEntry = releaseManifest.linux?.appimage ?? null;
const dmgEntry = releaseManifest.macos?.dmg ?? null;
const hasLinuxPublished = Boolean(debEntry || appImageEntry);
const hasMacosPublished = Boolean(dmgEntry);

// 1. Atualiza landing/public/release.json para consulta programática e auto-update
writeFileSync(releaseJsonPath, `${JSON.stringify(releaseManifest, null, 2)}\n`, "utf8");

// 2. Sincroniza versão no package.json da landing
const landingPkgPath = join(landingDir, "package.json");
if (existsSync(landingPkgPath)) {
  const landingPkg = JSON.parse(readFileSync(landingPkgPath, "utf8"));
  landingPkg.version = version;
  writeFileSync(landingPkgPath, `${JSON.stringify(landingPkg, null, 2)}\n`, "utf8");
}

function replaceTagText(html, attrName, value) {
  return html.replace(
    new RegExp(`(<[^>]*${attrName}[^>]*>)([^<]*)(</[^>]+>)`, "g"),
    `$1${value}$3`,
  );
}

function toggleBlockVisibility(html, attrName, visible) {
  return html.replace(new RegExp(`(<[^>]*${attrName}[^>]*?)(\\s+hidden)?(>)`, "g"), (_, start, _hidden, end) =>
    visible ? `${start}${end}` : `${start} hidden${end}`,
  );
}

// 3. Atualiza marcadores dinâmicos de versão, tamanho e SHA-256 no index.html
const indexHtmlPath = join(landingDir, "index.html");
if (existsSync(indexHtmlPath)) {
  let html = readFileSync(indexHtmlPath, "utf8");
  html = replaceTagText(html, "data-release-version(?!-)", `v${version}`);
  html = replaceTagText(html, "data-release-version-label", `Versão ${version}`);
  html = replaceTagText(html, "data-release-size", winSizeFormatted);
  html = replaceTagText(html, "data-release-installer-label", `Instalador NSIS · ${winSizeFormatted}`);
  html = replaceTagText(html, "data-release-sha256", winSha256);

  // Marcadores Linux (.deb e .AppImage)
  html = toggleBlockVisibility(html, "data-linux-published", hasLinuxPublished);
  html = toggleBlockVisibility(html, "data-linux-pending", !hasLinuxPublished);
  if (debEntry) {
    html = replaceTagText(html, "data-release-linux-deb-size", debEntry.sizeFormatted);
    html = replaceTagText(html, "data-release-linux-deb-sha256", debEntry.sha256);
    html = toggleBlockVisibility(html, "data-linux-deb-block", true);
  } else {
    html = toggleBlockVisibility(html, "data-linux-deb-block", false);
  }
  if (appImageEntry) {
    html = replaceTagText(html, "data-release-linux-appimage-size", appImageEntry.sizeFormatted);
    html = replaceTagText(html, "data-release-linux-appimage-sha256", appImageEntry.sha256);
    html = toggleBlockVisibility(html, "data-linux-appimage-block", true);
  } else {
    html = toggleBlockVisibility(html, "data-linux-appimage-block", false);
  }

  // Marcadores macOS (.dmg experimental)
  html = toggleBlockVisibility(html, "data-macos-published", hasMacosPublished);
  html = toggleBlockVisibility(html, "data-macos-pending", !hasMacosPublished);
  if (dmgEntry) {
    html = replaceTagText(html, "data-release-macos-dmg-size", dmgEntry.sizeFormatted);
    html = replaceTagText(html, "data-release-macos-dmg-sha256", dmgEntry.sha256);
  }

  html = html.replace(/"softwareVersion":\s*"[^"]*"/g, `"softwareVersion": "${version}"`);
  html = html.replace(/"fileSize":\s*"[^"]*"/g, `"fileSize": "${winSizeFormatted}"`);

  writeFileSync(indexHtmlPath, html, "utf8");
}

// 4. Atualiza index.md, llms.txt e sitemap.xml
const linuxSummaryLine = hasLinuxPublished
  ? `- **Linux x86_64**: ${[
      debEntry && `[.deb (${debEntry.sizeFormatted})](https://pulse.lagrimante.com.br/download/Pulse-latest-amd64.deb) · SHA-256 \`${debEntry.sha256}\``,
      appImageEntry && `[AppImage (${appImageEntry.sizeFormatted})](https://pulse.lagrimante.com.br/download/Pulse-latest-amd64.AppImage) · SHA-256 \`${appImageEntry.sha256}\``,
    ]
      .filter(Boolean)
      .join(" | ")}`
  : `- **Linux x86_64**: suporte nativo (.deb e AppImage) pronto no repositório; pacotes oficiais publicados a partir da próxima release em https://github.com/${REPO}/releases (ou compile localmente com \`npm run build:linux\`)`;

for (const docName of ["index.md", "llms.txt"]) {
  const docPath = join(landingDir, "public", docName);
  if (existsSync(docPath)) {
    let content = readFileSync(docPath, "utf8");
    content = content.replace(/instalador NSIS de ~?[0-9]+(?:,[0-9]+)? MB/gi, `instalador NSIS de ${winSizeFormatted}`);
    content = content.replace(
      /instalador NSIS \(aproximadamente [0-9]+(?:,[0-9]+)? MB\)/gi,
      `instalador NSIS (${winSizeFormatted})`,
    );
    content = content.replace(/instalador NSIS \([0-9]+(?:,[0-9]+)? MB\)/gi, `instalador NSIS (${winSizeFormatted})`);
    content = content.replace(/Versão atual:\s*[0-9.]+/g, `Versão atual: ${version}`);
    content = content.replace(/SHA-256(?:\s*\(Windows\))?:\s*`[a-f0-9]{64}`/gi, `SHA-256 (Windows): \`${winSha256}\``);
    content = content.replace(/^- \*\*Linux x86_64\*\*:.*$/gm, linuxSummaryLine);
    writeFileSync(docPath, content, "utf8");
  }
}

const sitemapPath = join(landingDir, "public", "sitemap.xml");
if (existsSync(sitemapPath)) {
  let sitemap = readFileSync(sitemapPath, "utf8");
  sitemap = sitemap.replace(/<lastmod>[0-9]{4}-[0-9]{2}-[0-9]{2}<\/lastmod>/g, `<lastmod>${todayIso}</lastmod>`);
  writeFileSync(sitemapPath, sitemap, "utf8");
}

// 5. Compila a Landing Page e valida SEO/integridade
console.log(
  `[sync-landing] Compilando landing page (v${version} · Win ${winSizeFormatted} · SHA-256 ${winSha256.slice(0, 12)}…)`,
);
execFileSync("npx", ["vite", "build", "landing", "--config", "landing/vite.config.js"], {
  cwd: rootDir,
  stdio: "inherit",
});
execFileSync("node", ["landing/scripts/check-seo.mjs", "--dir=landing/dist"], {
  cwd: rootDir,
  stdio: "inherit",
});

// 6. Espelha no repositório irmão ../pulse-lp (se existir) para manter paridade total de deploy
if (existsSync(siblingLpDir)) {
  for (const item of [
    "index.html",
    "package.json",
    "vite.config.js",
    "worker.js",
    "wrangler.jsonc",
    "README.md",
    "AGENTS.md",
  ]) {
    const srcItem = join(landingDir, item);
    if (existsSync(srcItem)) {
      copyFileSync(srcItem, join(siblingLpDir, item));
    }
  }
  for (const dirName of ["src", "public", "dist"]) {
    const srcSub = join(landingDir, dirName);
    if (existsSync(srcSub)) {
      cpSync(srcSub, join(siblingLpDir, dirName), { recursive: true, force: true });
    }
  }
  const checkSeoSrc = join(landingDir, "scripts", "check-seo.mjs");
  if (existsSync(checkSeoSrc)) {
    mkdirSync(join(siblingLpDir, "scripts"), { recursive: true });
    copyFileSync(checkSeoSrc, join(siblingLpDir, "scripts", "check-seo.mjs"));
  }
  console.log(`[sync-landing] Repositório irmão sincronizado em ${siblingLpDir}`);
}
