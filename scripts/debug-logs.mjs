#!/usr/bin/env node
// Lê a trilha de auditoria gravada pelo Pulse instalado
// (`%LOCALAPPDATA%\Pulse\logs\debug-AAAA-MM-DD.jsonl`, só com o modo Debug
// ligado) e imprime um resumo para diagnóstico. Funciona no WSL e no Windows.
//
// Uso: node scripts/debug-logs.mjs [--since 24h|7d|ISO] [--op system_overview]
//                                  [--errors] [--json] [--dir <pasta>]
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parseJsonl, parseSince, summarize } from "./lib/debug-log-summary.mjs";

function argValue(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

function localAppData() {
  if (process.platform === "win32") return process.env.LOCALAPPDATA;
  // WSL: pergunta ao Windows e converte para /mnt/c/...
  const winPath = execFileSync("/mnt/c/Windows/System32/cmd.exe", ["/c", "echo %LOCALAPPDATA%"], {
    cwd: "/mnt/c",
    stdio: ["ignore", "pipe", "ignore"],
  }).toString().trim();
  if (!winPath || winPath.includes("%")) throw new Error("não foi possível descobrir %LOCALAPPDATA% do Windows; use --dir");
  return execFileSync("wslpath", ["-u", winPath]).toString().trim();
}

const pulseDir = argValue("--dir") ? null : join(localAppData(), "Pulse");
const logDir = argValue("--dir") ?? join(pulseDir, "logs");
const sinceMs = parseSince(argValue("--since"));
const operation = argValue("--op") ?? null;
const errorsOnly = process.argv.includes("--errors");
const asJson = process.argv.includes("--json");

const debugMode = pulseDir && existsSync(join(pulseDir, "debug-mode"))
  ? readFileSync(join(pulseDir, "debug-mode"), "utf8").trim() === "1"
  : null;

if (!existsSync(logDir)) {
  console.error(`[debug-logs] pasta inexistente: ${logDir}`);
  console.error(debugMode === false
    ? "[debug-logs] o modo Debug está DESLIGADO: peça ao usuário para ligá-lo em Debug e reproduzir o problema."
    : "[debug-logs] a versão instalada pode ser anterior à 1.6.0 (sem gravação em arquivo) ou nada foi registrado ainda.");
  process.exit(2);
}

const sinceDay = new Date(sinceMs).toISOString().slice(0, 10);
const files = readdirSync(logDir)
  .filter(name => /^debug-\d{4}-\d{2}-\d{2}\.jsonl$/.test(name))
  // Datas do nome são locais; um dia de folga cobre o fuso.
  .filter(name => name.slice(6, 16) >= new Date(sinceMs - 86_400_000).toISOString().slice(0, 10))
  .sort();

let invalidLines = 0;
const entries = [];
for (const name of files) {
  const { entries: parsed, invalid } = parseJsonl(readFileSync(join(logDir, name), "utf8"));
  entries.push(...parsed);
  invalidLines += invalid;
}

const summary = summarize(entries, { sinceMs, operation, errorsOnly });

if (asJson) {
  console.log(JSON.stringify({ logDir, debugMode, files, invalidLines, sinceMs, ...summary }, null, 2));
  process.exit(0);
}

const fmt = ms => new Date(ms).toLocaleString("pt-BR");
console.log(`Pasta: ${logDir}`);
console.log(`Modo Debug: ${debugMode === null ? "desconhecido" : debugMode ? "ligado" : "DESLIGADO (nada novo será gravado)"}`);
console.log(`Arquivos: ${files.join(", ") || "nenhum"} · janela desde ${fmt(sinceMs)} (${sinceDay})`);
console.log(`Eventos: ${summary.entryCount} · erros: ${summary.errorCount} · sessões: ${summary.sessionCount} · versões: ${JSON.stringify(summary.versions)}`);
if (invalidLines) console.log(`ATENÇÃO: ${invalidLines} linha(s) corrompida(s) ignorada(s)`);

console.log("\nOperações (mais erros primeiro):");
for (const op of summary.operations) {
  console.log(`  ${op.operation.padEnd(34)} total ${String(op.total).padStart(5)}  erros ${String(op.errors).padStart(5)} (${Math.round(op.errorRate * 100)}%)  p50 ${op.p50Ms ?? "-"}ms  p95 ${op.p95Ms ?? "-"}ms  último ${op.lastOutcome}`);
}

if (summary.errorGroups.length) {
  console.log("\nGrupos de erro:");
  for (const group of summary.errorGroups) {
    console.log(`\n  [${group.count}x] ${group.operation} · etapa ${group.stage ?? "-"} · versões ${group.versions.join(", ") || "-"}`);
    console.log(`    de ${fmt(group.firstAt)} até ${fmt(group.lastAt)}`);
    console.log(`    mensagem: ${group.message}`);
    if (group.detail) console.log(`    detalhe:  ${group.detail}`);
    if (group.context) console.log(`    contexto: ${group.context}`);
  }
} else {
  console.log("\nNenhum erro na janela.");
}
