// Utilitários puros para artefatos multiplataforma de release do Pulse
// (Windows NSIS, Linux .deb/.AppImage e macOS .dmg experimental).
// Compartilhado por sync-landing.mjs, publish-release.mjs e release.mjs.

export const REPO = "LucasLagrimante/pulse";
export const RELEASES_URL = `https://github.com/${REPO}/releases`;
export const SITE_URL = "https://pulse.lagrimante.com.br";

export const DOWNLOAD_ALIASES = {
  windows: "Pulse-latest-x64-setup.exe",
  linuxDeb: "Pulse-latest-amd64.deb",
  linuxAppImage: "Pulse-latest-amd64.AppImage",
  macosDmg: "Pulse-latest-macos.dmg",
};

export const SUPPORTED_PLATFORMS = [
  "Windows 11 x64",
  "Linux x86_64 (.deb, AppImage)",
  "macOS Apple Silicon (experimental)",
];

const SHA256_RE = /^[a-f0-9]{64}$/;

export function formatSizeMb(sizeBytes) {
  const mb = sizeBytes / (1024 * 1024);
  return `${mb.toFixed(1).replace(".", ",")} MB`;
}

export function isValidSha256(value) {
  return typeof value === "string" && SHA256_RE.test(value.trim().toLowerCase());
}

/**
 * Identifica os instaladores da versão `version` dentro de uma lista de nomes
 * de arquivo (seja de `gh release view` ou de pastas locais de bundle).
 */
export function matchReleaseAssets(filenames, version) {
  const ver = String(version).trim().replace(/^v/i, "");
  const expectedWin = `Pulse_${ver}_x64-setup.exe`;
  const hasVersion = name => name.includes(`_${ver}_`) || name.includes(`-${ver}-`);

  const windows = filenames.find(name => name === expectedWin) ?? null;
  const linuxDeb =
    filenames.find(name => hasVersion(name) && /\.deb$/i.test(name) && /amd64|x86_64/i.test(name)) ??
    filenames.find(name => hasVersion(name) && /\.deb$/i.test(name)) ??
    null;
  const linuxAppImage =
    filenames.find(name => hasVersion(name) && /\.AppImage$/i.test(name) && /amd64|x86_64/i.test(name)) ??
    filenames.find(name => hasVersion(name) && /\.AppImage$/i.test(name)) ??
    null;
  const macosDmg =
    filenames.find(name => hasVersion(name) && /\.dmg$/i.test(name) && /aarch64|arm64/i.test(name)) ??
    filenames.find(name => hasVersion(name) && /\.dmg$/i.test(name)) ??
    null;

  return { windows, linuxDeb, linuxAppImage, macosDmg };
}

function buildAssetEntry(aliasFilename, version, asset) {
  if (!asset) return null;
  return {
    filename: aliasFilename,
    url: `${SITE_URL}/download/${aliasFilename}`,
    assetUrl: `${RELEASES_URL}/download/v${version}/${asset.assetName}`,
    sizeBytes: asset.sizeBytes,
    sizeFormatted: formatSizeMb(asset.sizeBytes),
    sha256: asset.sha256.toLowerCase(),
  };
}

/**
 * Monta o manifesto `landing/public/release.json` mantendo 100% de
 * compatibilidade com o auto-update do Windows (`installer`) e do Linux
 * (`linux.appimage` / `linux.deb`) em `src-tauri/src/self_update.rs`.
 * Nunca inventa entradas para plataformas sem artefato real.
 */
export function buildReleaseManifest({
  version,
  updatedAt,
  windows,
  linuxDeb = null,
  linuxAppImage = null,
  macosDmg = null,
}) {
  if (!windows) {
    throw new Error("buildReleaseManifest exige metadados reais do instalador Windows");
  }

  const manifest = {
    product: "Pulse",
    version,
    platform: "Windows 11 x64",
    platforms: [...SUPPORTED_PLATFORMS],
    installer: buildAssetEntry(DOWNLOAD_ALIASES.windows, version, windows),
  };

  const debEntry = buildAssetEntry(DOWNLOAD_ALIASES.linuxDeb, version, linuxDeb);
  const appImageEntry = buildAssetEntry(DOWNLOAD_ALIASES.linuxAppImage, version, linuxAppImage);
  if (debEntry || appImageEntry) {
    manifest.linux = {};
    if (debEntry) manifest.linux.deb = debEntry;
    if (appImageEntry) manifest.linux.appimage = appImageEntry;
  }

  const dmgEntry = buildAssetEntry(DOWNLOAD_ALIASES.macosDmg, version, macosDmg);
  if (dmgEntry) {
    manifest.macos = {
      experimental: true,
      dmg: dmgEntry,
    };
  }

  manifest.updatedAt = updatedAt;
  return manifest;
}

/**
 * Extrai o nome do asset real a partir de uma entrada existente do release.json
 * (para preservar metadados da mesma versão quando só uma plataforma é compilada).
 */
export function restoreAssetFromEntry(entry) {
  if (!entry || !isValidSha256(entry.sha256) || typeof entry.sizeBytes !== "number") {
    return null;
  }
  const assetName = typeof entry.assetUrl === "string" ? entry.assetUrl.split("/").pop() : null;
  if (!assetName) return null;
  return {
    assetName,
    sizeBytes: entry.sizeBytes,
    sha256: entry.sha256.toLowerCase(),
  };
}

/**
 * Valida um manifesto `release.json` (local ou no ar) contra os hashes reais
 * dos assets publicados na GitHub Release.
 */
export function verifyManifestHashes(manifest, version, expectedHashes, sourceLabel) {
  const problems = [];
  if (!manifest || manifest.version !== version) {
    problems.push(`${sourceLabel} não está na versão ${version} (encontrado: ${manifest?.version ?? "ausente"})`);
    return problems;
  }

  const pairs = [
    ["Windows (.exe)", manifest.installer?.sha256, expectedHashes.windows],
    ["Linux (.deb)", manifest.linux?.deb?.sha256, expectedHashes.linuxDeb],
    ["Linux (.AppImage)", manifest.linux?.appimage?.sha256, expectedHashes.linuxAppImage],
    ["macOS (.dmg)", manifest.macos?.dmg?.sha256, expectedHashes.macosDmg],
  ];

  for (const [label, actual, expected] of pairs) {
    if (!expected) continue;
    if (!actual) {
      problems.push(`${sourceLabel} não anuncia SHA-256 para ${label}`);
    } else if (actual.toLowerCase() !== expected.toLowerCase()) {
      problems.push(`${sourceLabel} com SHA-256 divergente em ${label} (${actual.slice(0, 12)}… vs ${expected.slice(0, 12)}…)`);
    }
  }

  return problems;
}
