// Publica o instalador NSIS como asset da GitHub Release `v<versão>` do Pulse.
//
// A landing não hospeda o .exe: o Worker redireciona `/download/*.exe` para a
// URL fixada em `landing/public/release.json` (`installer.assetUrl`). Por isso
// este script só publica se o arquivo do bundle tiver exatamente o SHA-256 que a
// landing anuncia — rode `npm run build:native` (ou `npm run lp:sync`) antes.
//
// Uso: npm run release:publish            → cria a Release (falha se já existir)
//      npm run release:publish -- --clobber → substitui o asset de uma Release existente
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = "LucasLagrimante/pulse";
const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const clobber = process.argv.includes("--clobber");

const fail = message => {
  console.error(`[publish-release] ERRO: ${message}`);
  process.exit(1);
};

const { version } = JSON.parse(readFileSync(join(rootDir, "src-tauri", "tauri.conf.json"), "utf8"));
const tag = `v${version}`;
const assetName = `Pulse_${version}_x64-setup.exe`;
const installerPath = join(rootDir, "src-tauri", "target", "release", "bundle", "nsis", assetName);
if (!existsSync(installerPath)) fail(`${assetName} não encontrado no bundle. Rode npm run build:native.`);

const sha256 = createHash("sha256").update(readFileSync(installerPath)).digest("hex");
const release = JSON.parse(readFileSync(join(rootDir, "landing", "public", "release.json"), "utf8"));
if (release.version !== version || release.installer?.sha256 !== sha256) {
  fail(`release.json (v${release.version}, ${release.installer?.sha256?.slice(0, 12)}…) não corresponde ao bundle (v${version}, ${sha256.slice(0, 12)}…). Rode npm run lp:sync.`);
}

const gh = args => execFileSync("gh", args, { cwd: rootDir, stdio: "inherit" });
let exists = true;
try {
  execFileSync("gh", ["release", "view", tag, "--repo", REPO], { stdio: "ignore" });
} catch {
  exists = false;
}

if (exists) {
  if (!clobber) fail(`a Release ${tag} já existe. Use --clobber para substituir o instalador.`);
  gh(["release", "upload", tag, installerPath, "--repo", REPO, "--clobber"]);
} else {
  const notes = `Instalador NSIS para Windows 11 x64.\n\nSHA-256 (\`${assetName}\`): \`${sha256}\``;
  gh(["release", "create", tag, installerPath, "--repo", REPO, "--title", `Pulse ${version}`, "--notes", notes]);
}
console.log(`[publish-release] ${tag} publicado: ${release.installer.assetUrl}`);
