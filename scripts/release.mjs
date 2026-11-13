#!/usr/bin/env node
// Release padronizado do Pulse. Fluxo completo e regras de decisão na skill
// `.agents/skills/lancar-versao/SKILL.md`; este script faz as partes mecânicas
// e checáveis, sem commitar nem dar push (isso é da skill `commit`).
//
//   node scripts/release.mjs check [patch|minor|major]   pré-condições + tipo sugerido + prévia das notas
//   node scripts/release.mjs bump <patch|minor|major> [--force-type]
//                                                        troca a versão nos 6 arquivos e grava as notas
//   node scripts/release.mjs notes                       imprime as notas desde a última tag
//   node scripts/release.mjs verify [versão]             confere Release, asset, landing e release.json no ar
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  RELEASE_TYPES,
  VERSION_FILES,
  bumpAll,
  bumpVersion,
  readVersions,
  renderReleaseNotes,
  suggestReleaseType,
} from "./lib/release-version.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = "LucasLagrimante/pulse";
const LIVE_RELEASE_JSON = "https://pulse.lagrimante.com.br/release.json";
const ORDER = { patch: 0, minor: 1, major: 2 };

const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
const fail = message => {
  console.error(`[release] ERRO: ${message}`);
  process.exit(1);
};

function readContents() {
  return Object.fromEntries(VERSION_FILES.map(file => [file, readFileSync(join(root, file), "utf8")]));
}

function currentVersion() {
  const versions = readVersions(readContents());
  const distinct = new Set(Object.values(versions));
  if (distinct.size !== 1) {
    fail(`versões divergentes entre os arquivos:\n${JSON.stringify(versions, null, 2)}`);
  }
  return [...distinct][0];
}

function lastTag() {
  try {
    return git("describe", "--tags", "--abbrev=0", "--match", "v[0-9]*");
  } catch {
    return null;
  }
}

function commitsSince(tag) {
  const range = tag ? `${tag}..HEAD` : "HEAD";
  const raw = git("log", range, "--format=%s%x1f%b%x1e");
  return raw
    .split("\x1e")
    .map(chunk => chunk.trim())
    .filter(Boolean)
    .map(chunk => {
      const [subject, body = ""] = chunk.split("\x1f");
      return { subject: subject.trim(), body };
    });
}

function preconditions({ requireClean }) {
  const problems = [];
  const branch = git("rev-parse", "--abbrev-ref", "HEAD");
  if (branch !== "main") problems.push(`branch atual é ${branch}; release só sai da main`);
  const status = git("status", "--porcelain");
  if (requireClean && status) problems.push(`árvore com alterações não commitadas:\n${status}`);
  try {
    git("fetch", "--quiet", "origin", "main", "--tags");
    const behind = Number(git("rev-list", "--count", "HEAD..origin/main"));
    if (behind > 0) problems.push(`main está ${behind} commit(s) atrás de origin/main; faça pull antes`);
  } catch (error) {
    problems.push(`não foi possível consultar origin: ${error.message.split("\n")[0]}`);
  }
  try {
    execFileSync("gh", ["auth", "status"], { stdio: "ignore" });
  } catch {
    problems.push("gh sem autenticação (gh auth login)");
  }
  return problems;
}

function report(type) {
  const version = currentVersion();
  const tag = lastTag();
  const commits = commitsSince(tag);
  const suggested = suggestReleaseType(commits);
  console.log(`Versão atual: ${version} · última tag: ${tag ?? "nenhuma"} · ${commits.length} commit(s) desde então`);
  if (tag && tag !== `v${version}`) {
    console.log(`ATENÇÃO: última tag ${tag} difere da versão dos arquivos (${version})`);
  }
  console.log(`Tipo sugerido pelos commits: ${suggested} → ${bumpVersion(version, suggested)}`);
  if (type) console.log(`Tipo pedido: ${type} → ${bumpVersion(version, type)}`);
  return { version, tag, commits, suggested };
}

function writeNotes(version, commits) {
  const dir = join(root, "src-tauri", "target", "release-notes");
  mkdirSync(dir, { recursive: true });
  const path = join(dir, `v${version}.md`);
  writeFileSync(path, renderReleaseNotes(version, commits));
  return path;
}

const [command, arg] = process.argv.slice(2);

switch (command) {
  case "check": {
    if (arg && !RELEASE_TYPES.includes(arg)) fail(`tipo inválido: ${arg}`);
    const problems = preconditions({ requireClean: false });
    const { commits } = report(arg);
    console.log(`\n--- prévia das notas ---\n${renderReleaseNotes("X.Y.Z", commits)}`);
    if (git("status", "--porcelain")) {
      console.log("Aviso: há alterações não commitadas; commite-as (skill commit) antes do bump.");
    }
    if (problems.length) fail(problems.join("\n"));
    console.log("Pré-condições OK.");
    break;
  }

  case "bump": {
    if (!RELEASE_TYPES.includes(arg)) fail(`informe o tipo: ${RELEASE_TYPES.join(" | ")}`);
    const problems = preconditions({ requireClean: true });
    if (problems.length) fail(problems.join("\n"));
    const { version, commits, suggested } = report(arg);
    if (!commits.length) fail("nenhum commit desde a última tag; não há o que lançar");
    if (ORDER[arg] < ORDER[suggested] && !process.argv.includes("--force-type")) {
      fail(`tipo ${arg} é menor que o sugerido (${suggested}) pelos commits. Revise os commits ou use --force-type com justificativa.`);
    }
    const next = bumpVersion(version, arg);
    if (git("tag", "--list", `v${next}`)) fail(`a tag v${next} já existe`);
    const bumped = bumpAll(readContents(), version, next);
    for (const [file, text] of Object.entries(bumped)) writeFileSync(join(root, file), text);
    const notesPath = writeNotes(next, commits);
    console.log(`\nVersão ${version} → ${next} gravada em: ${VERSION_FILES.join(", ")}`);
    console.log(`Notas: ${notesPath}`);
    console.log(`\nPróximo: commit "chore(release): versão ${next}" (skill commit), depois`);
    console.log(`  git tag -a v${next} -F ${notesPath} && git push origin main v${next}`);
    break;
  }

  case "notes": {
    const tag = lastTag();
    process.stdout.write(renderReleaseNotes(arg ?? "X.Y.Z", commitsSince(tag)));
    break;
  }

  case "verify": {
    const version = arg ?? currentVersion();
    const tag = `v${version}`;
    const asset = `Pulse_${version}_x64-setup.exe`;
    const problems = [];

    const release = JSON.parse(execFileSync("gh", ["release", "view", tag, "--repo", REPO, "--json", "assets,isDraft,isPrerelease,body"], { encoding: "utf8" }));
    if (release.isDraft || release.isPrerelease) problems.push(`Release ${tag} está como rascunho/pré-release`);
    if (!release.assets.some(a => a.name === asset)) problems.push(`Release ${tag} sem o asset ${asset}`);

    const downloadDir = mkdtempSync(join(tmpdir(), "pulse-verify-"));
    let assetSha;
    try {
      execFileSync("gh", ["release", "download", tag, "--repo", REPO, "--pattern", asset, "--dir", downloadDir], { stdio: "ignore" });
      assetSha = createHash("sha256").update(readFileSync(join(downloadDir, asset))).digest("hex");
    } finally {
      rmSync(downloadDir, { recursive: true, force: true });
    }

    const local = JSON.parse(readFileSync(join(root, "landing", "public", "release.json"), "utf8"));
    const localSha = JSON.stringify(local).match(/[a-f0-9]{64}/)?.[0];
    if (!JSON.stringify(local).includes(version)) problems.push(`landing/public/release.json não está na versão ${version}`);
    if (localSha !== assetSha) problems.push(`SHA-256 da landing (${localSha}) difere do asset publicado (${assetSha}); rode npm run lp:sync:release`);

    let live = null;
    try {
      const response = await fetch(`${LIVE_RELEASE_JSON}?v=${Date.now()}`, { headers: { "cache-control": "no-cache" } });
      live = await response.json();
    } catch (error) {
      problems.push(`não foi possível ler ${LIVE_RELEASE_JSON}: ${error.message}`);
    }
    if (live) {
      const liveText = JSON.stringify(live);
      if (!liveText.includes(version)) problems.push(`${LIVE_RELEASE_JSON} ainda não anuncia ${version} (deploy do pulse-lp pendente?)`);
      if (!liveText.includes(assetSha)) problems.push(`${LIVE_RELEASE_JSON} com SHA-256 diferente do asset: o auto-update vai recusar o instalador`);
    }

    console.log(`Release ${tag}: asset ${asset} · SHA-256 ${assetSha}`);
    if (problems.length) fail(problems.join("\n"));
    console.log("Verificação OK: Release, landing local e release.json no ar consistentes.");
    break;
  }

  default:
    console.error("uso: node scripts/release.mjs <check [tipo] | bump <tipo> [--force-type] | notes | verify [versão]>");
    process.exit(2);
}
