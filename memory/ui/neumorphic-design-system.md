---
name: neumorphic-design-system
description: Sistema de design Neumorphism (Soft UI) com sombras duplas, suporte a temas Claro/Escuro e ícone de bandeja otimizado
module: ui
metadata:
  type: project
---
**Why:** O usuário solicitou a adoção completa da técnica de Neumorphism (Soft UI com relevos e sombras suaves) inspirada em referências visuais táteis, preservando a identidade Pulse com opção de temas claro e escuro e atenção crucial ao tray icon e painel rápido.
**How to apply:** Utilize variáveis CSS no `:root` e `[data-theme]` com pares de sombras opostas (`--neu-shadow-raised` e `--neu-shadow-inset`), switches táteis com `appearance: none` preservando semântica `<input type="checkbox">`, e gere a cadeia completa de ícones (`.ico`, `.png` 16/32/128/512) via `npx tauri icon` com pesos de traço calibrados para legibilidade na bandeja do Windows 11.
