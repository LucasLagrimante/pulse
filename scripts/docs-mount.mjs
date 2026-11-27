#!/usr/bin/env node
// Monta o material interno (repo privado `../pulse-docs`) dentro do clone do Pulse.
// Cria apenas um symlink local `memory` — ignorado pelo Git, nunca versionado.
import { existsSync, lstatSync, readlinkSync, symlinkSync, unlinkSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const docsRoot = resolve(repoRoot, "..", "pulse-docs");
const docsRemote = "git@github.com:LucasLagrimante/pulse-docs.git"; // secret-scan:allow remoto SSH do repo privado, não e-mail
const linkPath = join(repoRoot, "memory");
const targetPath = join(docsRoot, "memory");

function run(cmd, args, cwd) {
  const r = spawnSync(cmd, args, { cwd, stdio: "inherit" });
  if (r.status !== 0) {
    console.error(`[docs:mount] falhou: ${cmd} ${args.join(" ")}`);
    process.exit(r.status ?? 1);
  }
}

if (!existsSync(docsRoot)) {
  console.log(`[docs:mount] clonando ${docsRemote} em ${docsRoot}`);
  run("git", ["clone", docsRemote, docsRoot]);
}

if (!existsSync(targetPath)) {
  console.error(`[docs:mount] ${targetPath} não existe. O repo pulse-docs está íntegro?`);
  process.exit(1);
}

if (lstatSync(linkPath, { throwIfNoEntry: false })) {
  const st = lstatSync(linkPath);
  if (st.isSymbolicLink()) {
    if (readlinkSync(linkPath) === targetPath) {
      console.log(`[docs:mount] já montado: memory -> ${targetPath}`);
      process.exit(0);
    }
    unlinkSync(linkPath);
  } else {
    console.error(`[docs:mount] ${linkPath} é um diretório real com conteúdo.`);
    console.error("[docs:mount] Mova o conteúdo para pulse-docs/memory e remova o diretório antes de montar.");
    process.exit(1);
  }
}

symlinkSync(targetPath, linkPath, process.platform === "win32" ? "junction" : "dir");
console.log(`[docs:mount] montado: memory -> ${targetPath}`);
