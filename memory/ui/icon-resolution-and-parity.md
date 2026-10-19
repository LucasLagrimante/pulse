---
name: icon-resolution-and-parity
description: Resolução de ícones nativos do Windows nas telas de desinstalação, serviços e atualizações
module: ui
metadata:
  type: project
---
**Why:** Telas de inventário (desinstalação, serviços, atualizações) precisam exibir os ícones autênticos dos binários para paridade visual com o shell do Windows 11 e consistência com a aba de Inicialização.
**How to apply:** Em desinstalação, resolva `DisplayIcon` (sanitizando vírgulas de índice), `UninstallString` e `InstallLocation` via `app_icons::icon_for_command`. Em atualizações, correlacione o pacote do winget com o inventário instalado por nome/id. Na UI, envolva o ícone em contêiner neumórfico sunken com fallback gracioso para monograma ou SVG do tema.
