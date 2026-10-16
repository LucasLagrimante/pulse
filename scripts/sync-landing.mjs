// Sincroniza o instalador NSIS compilado, metadados de versão/tamanho/SHA-256
// e artefatos da Landing Page (em `landing/` e, se existir, no repo irmão `../pulse-lp`).
//
// Executado automaticamente ao final de `scripts/build-native.sh` ou via
// `npm run lp:sync`.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const landingDir = join(rootDir, "landing");
const siblingLpDir = resolve(rootDir, "..", "pulse-lp");
const bundleDir = join(rootDir, "src-tauri", "target", "release", "bundle", "nsis");

const tauriConfPath = join(rootDir, "src-tauri", "tauri.conf.json");
const tauriConf = JSON.parse(readFileSync(tauriConfPath, "utf8"));
const version = tauriConf.version || "1.0.0";

const downloadDir = join(landingDir, "public", "download");
mkdirSync(downloadDir, { recursive: true });
const targetInstallerPath = join(downloadDir, "Pulse-latest-x64-setup.exe");

if (existsSync(bundleDir)) {
  const setups = readdirSync(bundleDir)
    .filter(file => file.toLowerCase().endsWith("-setup.exe"))
    .map(file => ({ file, mtime: statSync(join(bundleDir, file)).mtimeMs }))
    .sort((a, b) => b.mtime - a.mtime);

  if (setups.length > 0) {
    const latestSetup = join(bundleDir, setups[0].file);
    copyFileSync(latestSetup, targetInstallerPath);
    console.log(`[sync-landing] Instalador copiado: ${setups[0].file} -> landing/public/download/Pulse-latest-x64-setup.exe`);
  }
}

if (!existsSync(targetInstallerPath)) {
  console.error("[sync-landing] ERRO: Pulse-latest-x64-setup.exe não encontrado em landing/public/download/.");
  process.exit(1);
}

const installerBuffer = readFileSync(targetInstallerPath);
const sizeBytes = installerBuffer.byteLength;
const sizeMbNumber = sizeBytes / (1024 * 1024);
const sizeMbFormatted = `${sizeMbNumber.toFixed(1).replace(".", ",")} MB`;
const sha256 = createHash("sha256").update(installerBuffer).digest("hex");
const todayIso = new Date().toISOString().slice(0, 10);

// 1. Atualiza landing/public/release.json para consulta programática
const releaseManifest = {
  product: "Pulse",
  version,
  platform: "Windows 11 x64",
  installer: {
    filename: "Pulse-latest-x64-setup.exe",
    url: "https://pulse.lagrimante.com.br/download/Pulse-latest-x64-setup.exe",
    sizeBytes,
    sizeFormatted: sizeMbFormatted,
    sha256,
  },
  updatedAt: todayIso,
};
writeFileSync(
  join(landingDir, "public", "release.json"),
  `${JSON.stringify(releaseManifest, null, 2)}\n`,
  "utf8",
);

// 2. Sincroniza versão no package.json da landing
const landingPkgPath = join(landingDir, "package.json");
if (existsSync(landingPkgPath)) {
  const landingPkg = JSON.parse(readFileSync(landingPkgPath, "utf8"));
  landingPkg.version = version;
  writeFileSync(landingPkgPath, `${JSON.stringify(landingPkg, null, 2)}\n`, "utf8");
}

// 3. Atualiza marcadores dinâmicos de versão, tamanho e SHA-256 no index.html
const indexHtmlPath = join(landingDir, "index.html");
if (existsSync(indexHtmlPath)) {
  let html = readFileSync(indexHtmlPath, "utf8");
  html = html.replace(
    /(<[^>]*data-release-version[^>]*>)([^<]*)(<\/[^>]+>)/g,
    `$1v${version}$3`,
  );
  html = html.replace(
    /(<[^>]*data-release-version-label[^>]*>)([^<]*)(<\/[^>]+>)/g,
    `$1Versão ${version}$3`,
  );
  html = html.replace(
    /(<[^>]*data-release-size[^>]*>)([^<]*)(<\/[^>]+>)/g,
    `$1${sizeMbFormatted}$3`,
  );
  html = html.replace(
    /(<[^>]*data-release-installer-label[^>]*>)([^<]*)(<\/[^>]+>)/g,
    `$1Instalador NSIS · ${sizeMbFormatted}$3`,
  );
  html = html.replace(
    /(<[^>]*data-release-sha256[^>]*>)([^<]*)(<\/[^>]+>)/g,
    `$1${sha256}$3`,
  );
  html = html.replace(
    /"softwareVersion":\s*"[^"]*"/g,
    `"softwareVersion": "${version}"`,
  );
  html = html.replace(
    /"fileSize":\s*"[^"]*"/g,
    `"fileSize": "${sizeMbFormatted}"`,
  );
  writeFileSync(indexHtmlPath, html, "utf8");
}

// 4. Atualiza index.md, llms.txt e sitemap.xml
for (const docName of ["index.md", "llms.txt"]) {
  const docPath = join(landingDir, "public", docName);
  if (existsSync(docPath)) {
    let content = readFileSync(docPath, "utf8");
    content = content.replace(/instalador NSIS de ~?[0-9]+(?:,[0-9]+)? MB/gi, `instalador NSIS de ${sizeMbFormatted}`);
    content = content.replace(/instalador NSIS \(aproximadamente [0-9]+(?:,[0-9]+)? MB\)/gi, `instalador NSIS (${sizeMbFormatted})`);
    content = content.replace(/Versão atual:\s*[0-9.]+/g, `Versão atual: ${version}`);
    content = content.replace(/SHA-256:\s*`[a-f0-9]{64}`/gi, `SHA-256: \`${sha256}\``);
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
console.log(`[sync-landing] Compilando landing page (v${version} · ${sizeMbFormatted} · SHA-256 ${sha256.slice(0, 12)}…)`);
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
