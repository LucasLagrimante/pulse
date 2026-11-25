---
name: auditoria-rdp-diferenciar-wslg-e-porta-customizada
description: Auditoria de acesso remoto deve ler a porta real do RDP no Registro/TermService PID e classificar msrdc.exe dentro do WSL como WSLg interno (sem falso alarme).
module: seguranca
metadata:
  type: project
---

**Why:** O WSLg executa `C:\Program Files\WSL\msrdc.exe` localmente para janelas gráficas Linux; sinalizá-lo como acesso remoto gera falso alarme. Além disso, checar apenas a porta `3389` perde RDP configurado em porta customizada (`WinStations\RDP-Tcp\PortNumber`).

**How to apply:**
- Em `src-tauri/src/security_audit.rs`, cruzar `GetExtendedTcpTable` com `PortNumber` do Registro e com o PID real do `TermService` (SCM), além de portas remotas conhecidas.
- Classificar `msrdc.exe` cujo caminho contenha `\wsl\` ou `windowssubsystemforlinux` como `wslg_internal`, separado de `rdp_client` (`mstsc`/`msrdcw`) e `remote_agent` (AnyDesk, TeamViewer, RustDesk, VNC, etc.).
- Em `src-tauri/src/custom_dock/tray_proxy.rs`, filtrar `msrdc.exe` dentro da pasta do WSL (`is_wslg_msrdc_path`) na Barra Superior: o Windows 11 esconde esses ícones no overflow (`^`), o menu deles quebra buscando `msrdcw.exe`, e exibi-los assusta o usuário.
- Se qualquer fonte (Registro, `qwinsta`, tabela TCP, processos) falhar, devolver veredito `partial` com `unavailableReason` — nunca `clean`.
