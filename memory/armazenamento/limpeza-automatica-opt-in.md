---
name: limpeza-automatica-opt-in
description: Limpeza automática do Pulse é opt-in, persistida em auto-cleanup.json, roda só ocioso/na tomada, preserva arquivos < 7 dias e nunca esvazia a Lixeira; VHDX/WinSxS só como recuperação guiada
module: armazenamento
metadata:
  type: project
---
**Why:** O usuário pediu limpeza automática "opcional, mas marcada uma vez fica em cache"; o princípio de ações não destrutivas exige opt-in e salvaguardas.
**How to apply:**
- Estado em `%LOCALAPPDATA%\Pulse\auto-cleanup.json` (escrita atômica) via `get_auto_cleanup`/`set_auto_cleanup`; desligado por padrão, intervalo 1/7/30 dias.
- Thread `pulse-auto-cleanup` (`disk_cleanup::init`): espera 5 min, checa a cada 15 min, exige ociosidade ≥5 min (`GetLastInputInfo`) e não estar na bateria; `CleaningGuard` impede manual e automática simultâneas.
- Modo auto passa `cutoff` de 7 dias a `remove_entry` (pasta usa o mtime lido ANTES de apagar os filhos); `recycle-bin` e `dev-package-caches` ficam fora de `is_auto_eligible` (corte por idade deixaria cache Gradle/uv pela metade). Manual: TEMP (`user-temp`/`windows-temp`) preserva 24 h via `effective_cutoff`; Puppeteer fora (Chrome não é rebaixado sozinho).
- Entradas `pulse-*` no topo das raízes (staging/cache de build `pulse-src`/`pulse-target*`, relatórios `pulse-disk-*.md`) são puladas na análise e na limpeza (`is_pulse_owned`): apagar parte do cache corrompe o build (`bindgen.rs` do libsqlite3-sys).
- VHDX do WSL/Docker e WinSxS: só `get_cleanup_insights` (detecta + comando para copiar; `attach vdisk readonly`), nunca executados nem apagados pelo Pulse.
