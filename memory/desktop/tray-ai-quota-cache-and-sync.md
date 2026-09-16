---
name: tray-ai-quota-cache-and-sync
description: Retenção de cache prévio e sincronização em tempo real das cotas de IA entre Tray Icon e janela principal
module: desktop
metadata:
  type: project
---
**Why:** O flyout do tray não pode zerar cards nem piscar com placeholders ao abrir se já houver leituras anteriores, e alterações de cotas devem refletir simultaneamente na janela principal e no tray.
**How to apply:** No Rust, `start_background_scan` deve inicializar preservando os provedores existentes no cache de memória/disco em vez de usar placeholders, salvando em `%LOCALAPPDATA%\Pulse\subscription-cache.json` e emitindo o evento `pulse://subscription-usage-updated` a cada leitura. No frontend, tanto o `TrayFlyout` quanto o `SubscriptionUsage` escutam o evento, gravam no `localStorage` e inicializam a partir do cache local para renderização imediata sem sumiço de cards.
