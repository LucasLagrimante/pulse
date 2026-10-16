// Encerra a instância do Pulse em execução antes do `tauri build`.
// O Windows trava executáveis em uso; com o app aberto (inclusive minimizado
// no tray), o cargo não consegue remover `target\release\pulse-windows.exe`
// e o link falha com `Access is denied (os error 5)`.
// Uso: `npm run tauri:build`. Saída 0 sempre — nunca bloqueia o build.
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";

const TASKKILL_WSL = "/mnt/c/Windows/System32/taskkill.exe";
const taskkillBin =
  process.platform === "win32"
    ? "taskkill"
    : existsSync(TASKKILL_WSL)
      ? TASKKILL_WSL
      : null;

if (!taskkillBin) {
  process.exit(0);
}

try {
  execFileSync(taskkillBin, ["/IM", "pulse-windows.exe", "/F"], { stdio: "ignore" });
  console.log("[kill-running-app] Instância anterior do Pulse encerrada.");
} catch {
  // Exit 128 = nenhum processo encontrado: nada a fazer, segue o build.
}

