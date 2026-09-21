// Executa o instalador NSIS gerado pelo `tauri build`.
// Uso: `npm run tauri:build:install`. Localiza o `*-setup.exe` mais recente em
// `src-tauri\target\release\bundle\nsis\` e o abre via `cmd /c start`
// (ShellExecute: processo desanexado e UAC acionado se o instalador exigir
// elevação — spawn direto usa CreateProcess e falha nesse caso).
import { spawnSync } from "node:child_process";
import { readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

if (process.platform !== "win32") {
  console.log("[run-installer] Instalador disponível apenas no Windows.");
  process.exit(0);
}

const bundleDir = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
  "target",
  "release",
  "bundle",
  "nsis",
);

let setups;
try {
  setups = readdirSync(bundleDir)
    .filter((file) => file.toLowerCase().endsWith("-setup.exe"))
    .map((file) => ({ file, mtime: statSync(join(bundleDir, file)).mtimeMs }))
    .sort((a, b) => b.mtime - a.mtime);
} catch {
  console.error(`[run-installer] Pasta não encontrada: ${bundleDir}`);
  console.error("[run-installer] Rode `npm run tauri:build` primeiro.");
  process.exit(1);
}

if (setups.length === 0) {
  console.error(`[run-installer] Nenhum *-setup.exe em ${bundleDir}.`);
  process.exit(1);
}

const installer = join(bundleDir, setups[0].file);
console.log(`[run-installer] Executando ${installer}…`);
// `start "" "<exe>"`: título vazio + caminho entre aspas; retorna na hora,
// sem travar o terminal. Erros de lançamento viram saída não-zero com motivo.
const result = spawnSync("cmd.exe", ["/c", "start", "", installer], {
  stdio: "ignore",
  windowsHide: true,
});
if (result.error) {
  console.error(`[run-installer] Falha ao abrir o instalador: ${result.error.message}`);
  process.exit(1);
}
if (result.status !== 0) {
  console.error(`[run-installer] cmd saiu com código ${result.status}.`);
  process.exit(1);
}
console.log("[run-installer] Instalador iniciado.");
