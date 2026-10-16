// Executa o instalador NSIS gerado pelo `tauri build`.
// Uso: `npm run tauri:build:install`. Localiza o `*-setup.exe` mais recente em
// `src-tauri\target\release\bundle\nsis\` e o abre via `cmd /c start`
// (ShellExecute: processo desanexado e UAC acionado se o instalador exigir
// elevação — spawn direto usa CreateProcess e falha nesse caso).
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const CMD_WSL = "/mnt/c/Windows/System32/cmd.exe";
const isWin = process.platform === "win32";
const isWsl = !isWin && existsSync(CMD_WSL);

if (!isWin && !isWsl) {
  console.log("[run-installer] Instalador disponível apenas no Windows ou WSL.");
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
  console.error("[run-installer] Rode `npm run build:native` primeiro.");
  process.exit(1);
}

if (setups.length === 0) {
  console.error(`[run-installer] Nenhum *-setup.exe em ${bundleDir}.`);
  process.exit(1);
}

const installer = join(bundleDir, setups[0].file);
let winInstallerPath = installer;
const cmdBin = isWin ? "cmd.exe" : CMD_WSL;

if (isWsl) {
  // cmd.exe não aceita UNC (\\wsl.localhost\...) como cwd e alguns instaladores
  // NSIS falham se executados de caminho UNC. Copiamos para Temp em NTFS.
  const ntfsTempDir = "/mnt/c/Users/lucas/AppData/Local/Temp/pulse-target/release/bundle/nsis";
  mkdirSync(ntfsTempDir, { recursive: true });
  const ntfsInstaller = join(ntfsTempDir, setups[0].file);
  copyFileSync(installer, ntfsInstaller);
  winInstallerPath = execFileSync("wslpath", ["-w", ntfsInstaller], { encoding: "utf8" }).trim();
}

console.log(`[run-installer] Executando ${winInstallerPath}…`);
const result = spawnSync(cmdBin, ["/c", "start", "", winInstallerPath], {
  cwd: isWsl ? "/mnt/c/Windows/Temp" : undefined,
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

