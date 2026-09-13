# Polling somente em primeiro plano

- A janela principal e o `tray-flyout` consultam `is_current_window_foreground` antes de iniciar qualquer coleta recorrente.
- O backend emite `pulse://foreground-changed` ao ganhar ou perder foco e também antes de ocultar uma janela explicitamente.
- Ao sair do primeiro plano, o frontend cancela `setTimeout` e `setInterval` da telemetria. Ao voltar, coleta imediatamente e então retoma o intervalo normal de 5 segundos.
- A inicialização com `--minimized-to-tray` não coleta dados até a janela principal ou o painel rápido receber foco.
