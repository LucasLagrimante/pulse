// Regras puras do release do Pulse: SemVer, sugestão de tipo a partir dos
// commits convencionais, notas da Release e troca de versão nos 6 arquivos.
// Usadas por scripts/release.mjs e cobertas por release-version.test.mjs.

export const RELEASE_TYPES = ["patch", "minor", "major"];

/** Arquivos que carregam a versão; todos precisam concordar antes e depois do bump. */
export const VERSION_FILES = [
  "package.json",
  "package-lock.json",
  "landing/package.json",
  "src-tauri/Cargo.toml",
  "src-tauri/Cargo.lock",
  "src-tauri/tauri.conf.json",
];

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;

export function parseVersion(version) {
  const match = SEMVER.exec(String(version).trim());
  if (!match) throw new Error(`versão fora do padrão MAJOR.MINOR.PATCH: ${version}`);
  return match.slice(1).map(Number);
}

export function bumpVersion(version, type) {
  const [major, minor, patch] = parseVersion(version);
  switch (type) {
    case "major": return `${major + 1}.0.0`;
    case "minor": return `${major}.${minor + 1}.0`;
    case "patch": return `${major}.${minor}.${patch + 1}`;
    default: throw new Error(`tipo de release inválido: ${type} (use ${RELEASE_TYPES.join(", ")})`);
  }
}

const CONVENTIONAL = /^(?<type>[a-z]+)(?:\((?<scope>[^)]+)\))?(?<breaking>!)?:\s*(?<description>.+)$/;

export function parseCommit({ subject, body = "" }) {
  const match = CONVENTIONAL.exec(subject.trim());
  if (!match) return { type: null, scope: null, breaking: /BREAKING CHANGE/.test(body), description: subject.trim() };
  const { type, scope, breaking, description } = match.groups;
  return {
    type,
    scope: scope ?? null,
    breaking: Boolean(breaking) || /BREAKING CHANGE/.test(body),
    description: description.trim(),
  };
}

/** Commits do próprio processo de release não entram nas notas nem na sugestão. */
export function isReleaseHousekeeping(commit) {
  return commit.type === "chore" && (commit.scope === "release" || commit.scope === "landing");
}

/** A partir de quantas áreas (escopos) com `feat` o conjunto conta como mudança grande. */
export const MINOR_FEAT_SCOPES = 3;

/**
 * Classifica o release pelo que mudou de fato (política de 2026-11-13: a
 * versão sobe devagar e reflete o tamanho real da mudança).
 *
 * - major: quebra declarada (`!` ou `BREAKING CHANGE`).
 * - minor (mudança grande): módulo novo no backend — arquivo `.rs` adicionado
 *   que expõe `#[tauri::command]` — ou novidades em MINOR_FEAT_SCOPES+ áreas.
 * - patch (mudança pequena): o resto, inclusive `feat` pontual numa área existente.
 *
 * `addedFiles`: [{ path, content }] adicionados desde a tag base.
 * Devolve { type, reasons } — as razões vão para o relatório ao usuário.
 */
export function classifyRelease(commits, addedFiles = []) {
  const relevant = commits.map(parseCommit).filter(c => !isReleaseHousekeeping(c));
  const breaking = relevant.filter(c => c.breaking);
  if (breaking.length) {
    return { type: "major", reasons: breaking.map(c => `quebra declarada: ${c.scope ? `${c.scope}: ` : ""}${c.description}`) };
  }
  const reasons = [];
  const newModules = addedFiles.filter(
    file => /^src-tauri\/src\/.+\.rs$/.test(file.path) && /#\[tauri::command\]/.test(file.content ?? ""),
  );
  for (const file of newModules) reasons.push(`módulo novo com comandos Tauri: ${file.path}`);
  const featScopes = [...new Set(relevant.filter(c => c.type === "feat").map(c => c.scope ?? "geral"))];
  if (featScopes.length >= MINOR_FEAT_SCOPES) {
    reasons.push(`novidades em ${featScopes.length} áreas: ${featScopes.join(", ")}`);
  }
  if (reasons.length) return { type: "minor", reasons };
  const counts = ["feat", "fix", "perf"].map(t => `${relevant.filter(c => c.type === t).length} ${t}`).join(", ");
  return {
    type: "patch",
    reasons: [`mudança pequena: ${counts}; nenhum módulo novo e novidades em menos de ${MINOR_FEAT_SCOPES} áreas`],
  };
}

export function suggestReleaseType(commits, addedFiles = []) {
  return classifyRelease(commits, addedFiles).type;
}

const SECTIONS = [
  { title: "Mudanças incompatíveis", match: c => c.breaking },
  { title: "Novidades", match: c => !c.breaking && c.type === "feat" },
  { title: "Correções", match: c => !c.breaking && c.type === "fix" },
  { title: "Desempenho", match: c => !c.breaking && c.type === "perf" },
];

function capitalize(text) {
  return text.charAt(0).toLocaleUpperCase("pt-BR") + text.slice(1);
}

/** Notas da Release em Markdown, só com o que o usuário do app percebe. */
export function renderReleaseNotes(version, commits) {
  const parsed = commits.map(parseCommit).filter(c => !isReleaseHousekeeping(c));
  const lines = [`## Pulse ${version}`, ""];
  let any = false;
  for (const section of SECTIONS) {
    const items = parsed.filter(section.match);
    if (!items.length) continue;
    any = true;
    lines.push(`### ${section.title}`, "");
    for (const item of items) {
      lines.push(`- ${item.scope ? `**${item.scope}**: ` : ""}${capitalize(item.description)}`);
    }
    lines.push("");
  }
  if (!any) lines.push("Manutenção interna, sem mudança visível no aplicativo.", "");
  return lines.join("\n").trimEnd() + "\n";
}

function replaceExactlyOnce(text, pattern, replacement, label) {
  const matches = text.match(new RegExp(pattern.source, pattern.flags.includes("g") ? pattern.flags : pattern.flags + "g"));
  if (!matches || matches.length !== 1) {
    throw new Error(`${label}: esperava exatamente 1 ocorrência da versão, achei ${matches?.length ?? 0}`);
  }
  return text.replace(pattern, replacement);
}

/** Troca só o primeiro `"version"` (o do topo), preservando a formatação do arquivo. */
export function bumpJsonTopLevel(text, from, to, label) {
  const parsed = JSON.parse(text);
  if (parsed.version !== from) throw new Error(`${label}: versão ${parsed.version}, esperava ${from}`);
  const next = text.replace(`"version": "${from}"`, `"version": "${to}"`);
  const check = JSON.parse(next);
  if (check.version !== to) throw new Error(`${label}: o primeiro "version" não é o do topo`);
  return next;
}

/** package-lock: versão da raiz e de `packages[""]`, nada mais. */
export function bumpPackageLock(text, from, to) {
  const parsed = JSON.parse(text);
  if (parsed.version !== from || parsed.packages?.[""]?.version !== from) {
    throw new Error(`package-lock.json: versões ${parsed.version}/${parsed.packages?.[""]?.version}, esperava ${from}`);
  }
  let count = 0;
  const next = text.replace(new RegExp(`"version": "${from.replace(/\./g, "\\.")}"`, "g"), match => {
    count += 1;
    return count <= 2 ? `"version": "${to}"` : match;
  });
  const check = JSON.parse(next);
  if (check.version !== to || check.packages[""].version !== to) {
    throw new Error("package-lock.json: as duas primeiras ocorrências não eram a raiz e packages[\"\"]");
  }
  return next;
}

export function bumpCargoToml(text, from, to) {
  return replaceExactlyOnce(
    text,
    new RegExp(`^(\\[package\\][\\s\\S]*?^version = )"${from.replace(/\./g, "\\.")}"`, "m"),
    `$1"${to}"`,
    "Cargo.toml",
  );
}

export function bumpCargoLock(text, from, to, crate = "pulse-windows") {
  return replaceExactlyOnce(
    text,
    new RegExp(`(name = "${crate}"\\r?\\nversion = )"${from.replace(/\./g, "\\.")}"`),
    `$1"${to}"`,
    "Cargo.lock",
  );
}

/** Lê a versão de cada arquivo (para checar consistência). */
export function readVersions(contents) {
  const json = name => JSON.parse(contents[name]);
  return {
    "package.json": json("package.json").version,
    "package-lock.json": json("package-lock.json").version,
    "package-lock.json#packages": json("package-lock.json").packages?.[""]?.version,
    "landing/package.json": json("landing/package.json").version,
    "src-tauri/Cargo.toml": /^\[package\][\s\S]*?^version = "([^"]+)"/m.exec(contents["src-tauri/Cargo.toml"])?.[1],
    "src-tauri/Cargo.lock": /name = "pulse-windows"\r?\nversion = "([^"]+)"/.exec(contents["src-tauri/Cargo.lock"])?.[1],
    "src-tauri/tauri.conf.json": json("src-tauri/tauri.conf.json").version,
  };
}

export function bumpAll(contents, from, to) {
  return {
    "package.json": bumpJsonTopLevel(contents["package.json"], from, to, "package.json"),
    "package-lock.json": bumpPackageLock(contents["package-lock.json"], from, to),
    "landing/package.json": bumpJsonTopLevel(contents["landing/package.json"], from, to, "landing/package.json"),
    "src-tauri/Cargo.toml": bumpCargoToml(contents["src-tauri/Cargo.toml"], from, to),
    "src-tauri/Cargo.lock": bumpCargoLock(contents["src-tauri/Cargo.lock"], from, to),
    "src-tauri/tauri.conf.json": bumpJsonTopLevel(contents["src-tauri/tauri.conf.json"], from, to, "tauri.conf.json"),
  };
}
