---
name: desativar-provedores-ia
description: Desativar provedores de IA pausa a coleta no backend e remove o card do tray icon
module: telemetria
metadata:
  type: project
---

**Why:** Desativar um provedor precisa significar "parar de atualizar" (não apenas esconder na UI) e o tray é uma janela separada que não lê `get_subscription_settings`; por isso o estado precisa viajar dentro do próprio relatório.

**How to apply:**
1. Persistir a lista em `%LOCALAPPDATA%\Pulse\subscription-preferences.json` (`disabledProviders`), exposta em `SubscriptionSettings.disabledProviders`.
2. Marcar `ProviderUsage.disabled` no backend e pular a consulta no `start_background_scan` para ids desativados, mantendo a última leitura.
3. Comando `set_provider_enabled(providerId, enabled)` atualiza o cache/notify imediatamente (tray reage) e invalida o scan.
4. No frontend, `isProviderDisabled` combina `provider.disabled` com `disabledProviders`; card fica esmaecido com botão Reativar e o `TrayFlyout` filtra por `provider.disabled !== true`.
5. Desativados são sempre empurrados ao final (backend e tela), e o cabeçalho do card usa três elementos à direita — desativar/reativar e chave como ícones (`quota-icon-btn`) + pill de status, nunca como texto empilhado.
