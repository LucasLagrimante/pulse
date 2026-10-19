---
name: process-and-recent-file-icons
description: Ícones nativos para processos em execução (Visão Geral e Atividade) e previews/ícones para arquivos recentes
module: ui
metadata:
  type: project
---
**Why:** As listas de processos e arquivos recentes pareciam textuais e sem identidade visual comparadas a outras abas do Pulse (Inicialização, Desinstalação e Serviços).
**How to apply:** Em processos, exponha `path` e anexe data URIs de ícones via `app_icons::icon_for_path` (com fallback em `icon_for_command`). Em arquivos recentes, utilize `FileTypeIcon` por categoria semântica e carregue previews assíncronos (`get_image_preview`) para imagens, com fallbacks defensivos e contêineres neumórficos sunken.
