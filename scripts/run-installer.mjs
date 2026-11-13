// Executa o instalador NSIS gerado pelo `tauri build`.
// Uso: `npm run tauri:build:install`. Abre `Pulse_<versão>_x64-setup.exe` da versão
// atual do `tauri.conf.json` em `src-tauri\target\release\bundle\nsis\` via `cmd /c start`
// (ShellExecute: processo desanexado e UAC acionado se o instalador exigir
// elevação — spawn direto usa CreateProcess e falha nesse caso).
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
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

// Escolhe pela versão, nunca pela data: o cp de volta do WSL iguala as datas
// e instaladores de versões antigas podem ter sobrado na pasta.
const rootDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const version = JSON.parse(readFileSync(join(rootDir, "src-tauri", "tauri.conf.json"), "utf8")).version;
const setupName = `Pulse_${version}_x64-setup.exe`;
if (!existsSync(join(bundleDir, setupName))) {
  console.error(`[run-installer] ${setupName} não encontrado em ${bundleDir}.`);
  console.error("[run-installer] Rode `npm run build:native` primeiro.");
  process.exit(1);
}
const setups = [{ file: setupName }];

const installer = join(bundleDir, setups[0].file);
let winInstallerPath = installer;
const cmdBin = isWin ? "cmd.exe" : CMD_WSL;

if (isWsl) {
  // cmd.exe não aceita UNC (\\wsl.localhost\...) como cwd e alguns instaladores
  // NSIS falham se executados de caminho UNC. Copiamos para a pasta de build
  // em NTFS (%LOCALAPPDATA%\pulse-build, a mesma do build-native.sh).
  const winBuildDir =
    process.env.PULSE_WIN_BUILD_DIR ||
    execFileSync(CMD_WSL, ["/c", "echo %LOCALAPPDATA%\\pulse-build"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  const ntfsTempDir = join(
    execFileSync("wslpath", ["-u", winBuildDir], { encoding: "utf8" }).trim(),
    "target",
    "release",
    "bundle",
    "nsis",
  );
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

