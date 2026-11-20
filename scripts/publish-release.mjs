// Publica instaladores locais (Windows NSIS, Linux .deb/.AppImage e/ou macOS .dmg)
// como assets da GitHub Release `v<versão>` do Pulse.
//
// A landing não hospeda binários: o Worker redireciona `/download/*` para a URL
// fixada em `landing/public/release.json`. Por isso este script só publica se os
// arquivos do bundle tiverem exatamente os SHA-256 que a landing anuncia — rode
// `npm run build:native` / `npm run build:linux` / `npm run lp:sync` antes.
//
// O caminho oficial é o workflow `.github/workflows/release.yml` (push da tag
// `v<versão>`), que compila Windows, Linux e macOS (experimental) nos runners
// do GitHub. Este script publica builds locais SEM assinatura e por isso exige
// `--unsigned` explícito.
//
// Uso: npm run release:publish -- --unsigned                         → publica todos os bundles locais encontrados
//      npm run release:publish -- --unsigned --clobber               → substitui assets numa Release existente
//      npm run release:publish -- --unsigned --platform linux        → publica só os bundles da plataforma escolhida
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { REPO, matchReleaseAssets } from "./lib/release-assets.mjs";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const bundleRoot = join(rootDir, "src-tauri", "target", "release", "bundle");
const clobber = process.argv.includes("--clobber");
const unsigned = process.argv.includes("--unsigned");
const platformIdx = process.argv.indexOf("--platform");
const platformFilter = platformIdx >= 0 ? process.argv[platformIdx + 1] : null;

const fail = message => {
  console.error(`[publish-release] ERRO: ${message}`);
  process.exit(1);
};

if (!unsigned) {
  fail(
    "o release oficial sai pelo CI (push da tag v<versão> → .github/workflows/release.yml). Para publicar um build local sem assinatura, passe --unsigned.",
  );
}

if (platformFilter && !["windows", "linux", "macos"].includes(platformFilter)) {
  fail(`--platform inválido: ${platformFilter} (use windows, linux ou macos)`);
}

const { version } = JSON.parse(readFileSync(join(rootDir, "src-tauri", "tauri.conf.json"), "utf8"));
const tag = `v${version}`;
const release = JSON.parse(readFileSync(join(rootDir, "landing", "public", "release.json"), "utf8"));
if (release.version !== version) {
  fail(`release.json está na versão ${release.version}, mas tauri.conf.json está em ${version}. Rode npm run lp:sync.`);
}

function findBundle(subDir, field) {
  const dir = join(bundleRoot, subDir);
  if (!existsSync(dir)) return null;
  const matched = matchReleaseAssets(readdirSync(dir), version)[field];
  if (!matched) return null;
  const fullPath = join(dir, matched);
  const sha256 = createHash("sha256").update(readFileSync(fullPath)).digest("hex");
  return { name: matched, path: fullPath, sha256 };
}

const include = p => !platformFilter || platformFilter === p;
const candidates = {
  windows: include("windows") ? findBundle("nsis", "windows") : null,
  linuxDeb: include("linux") ? findBundle("deb", "linuxDeb") : null,
  linuxAppImage: include("linux") ? findBundle("appimage", "linuxAppImage") : null,
  macosDmg: include("macos") ? findBundle("dmg", "macosDmg") : null,
};

const filesToUpload = [];
const shaLines = [];

const checks = [
  ["Windows (.exe)", candidates.windows, release.installer?.sha256],
  ["Linux (.deb)", candidates.linuxDeb, release.linux?.deb?.sha256],
  ["Linux (.AppImage)", candidates.linuxAppImage, release.linux?.appimage?.sha256],
  ["macOS (.dmg)", candidates.macosDmg, release.macos?.dmg?.sha256],
];

for (const [label, bundle, manifestSha] of checks) {
  if (!bundle) continue;
  if (!manifestSha || manifestSha.toLowerCase() !== bundle.sha256.toLowerCase()) {
    fail(
      `release.json não corresponde ao bundle local de ${label} (${bundle.sha256.slice(0, 12)}… vs ${manifestSha?.slice(0, 12) ?? "ausente"}). Rode npm run lp:sync.`,
    );
  }
  filesToUpload.push(bundle.path);
  shaLines.push(`- ${label}: \`${bundle.name}\` · SHA-256 \`${bundle.sha256}\``);
}

if (!filesToUpload.length) {
  fail(
    `nenhum instalador da versão ${version} encontrado em src-tauri/target/release/bundle/. Rode npm run build:native ou npm run build:linux antes.`,
  );
}

const tempDir = mkdtempSync(join(tmpdir(), "pulse-publish-"));
try {
  const linuxBundles = [candidates.linuxDeb, candidates.linuxAppImage].filter(Boolean);
  if (linuxBundles.length) {
    const sumsPath = join(tempDir, "SHA256SUMS-linux.txt");
    writeFileSync(sumsPath, linuxBundles.map(b => `${b.sha256}  ${b.name}`).join("\n") + "\n", "utf8");
    filesToUpload.push(sumsPath);
  }
  if (candidates.macosDmg) {
    const sumsPath = join(tempDir, "SHA256SUMS-macos.txt");
    writeFileSync(sumsPath, `${candidates.macosDmg.sha256}  ${candidates.macosDmg.name}\n`, "utf8");
    filesToUpload.push(sumsPath);
  }

  const gh = args => execFileSync("gh", args, { cwd: rootDir, stdio: "inherit" });
  let exists = true;
  try {
    execFileSync("gh", ["release", "view", tag, "--repo", REPO], { stdio: "ignore" });
  } catch {
    exists = false;
  }

  if (exists) {
    if (!clobber) fail(`a Release ${tag} já existe. Use --clobber para substituir/anexar os instaladores.`);
    gh(["release", "upload", tag, ...filesToUpload, "--repo", REPO, "--clobber"]);
  } else {
    const notes = `Instaladores do Pulse ${version} (build local sem assinatura de código).\n\n${shaLines.join("\n")}`;
    gh(["release", "create", tag, ...filesToUpload, "--repo", REPO, "--title", `Pulse ${version}`, "--notes", notes]);
  }
  console.log(`[publish-release] ${tag} publicado com ${filesToUpload.length} arquivo(s).`);
} finally {
  rmSync(tempDir, { recursive: true, force: true });
}
