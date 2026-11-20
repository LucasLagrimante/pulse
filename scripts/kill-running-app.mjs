// Encerra a instância do Pulse em execução antes do build.
// No Windows o executável em uso trava o relink (`Access is denied / os error 5`);
// no Linux/macOS encerrar evita conflito de instância única durante testes locais.
// Saída 0 sempre — nunca bloqueia o build.
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";

const TASKKILL_WSL = "/mnt/c/Windows/System32/taskkill.exe";
const isWin = process.platform === "win32";
const isWsl = !isWin && existsSync(TASKKILL_WSL);

if (isWin || isWsl) {
  const taskkillBin = isWin ? "taskkill" : TASKKILL_WSL;
  try {
    execFileSync(taskkillBin, ["/IM", "pulse-windows.exe", "/F"], { stdio: "ignore" });
    console.log("[kill-running-app] Instância anterior do Pulse (Windows) encerrada.");
  } catch {
    // Exit 128 = nenhum processo encontrado: nada a fazer, segue o build.
  }
}

if (process.platform === "linux" || process.platform === "darwin") {
  try {
    execFileSync("pkill", ["-x", "pulse-windows"], { stdio: "ignore" });
    console.log("[kill-running-app] Instância anterior do Pulse encerrada.");
  } catch {
    // Exit 1 = nenhum processo encontrado: segue o build.
  }
}
