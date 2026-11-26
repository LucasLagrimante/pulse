---
name: runner-icones-reais
description: Pulse Runner deve exibir ícones reais dos apps via app_icons, não apenas o glifo genérico AppWindow
module: ux
metadata:
  type: project
---

**Why:** O runner mostrava o mesmo glifo azul (`AppWindow`) para todos os aplicativos porque `LauncherItem` só carregava `icon_hint`. O repositório já tinha extração nativa com cache em `app_icons` (SHGetFileInfo → PNG data URI), usada por startup/processos/mixer, mas o runner não a consumia.
**How to apply:** `indexer::icon_for_target()` resolve `.lnk`/`.exe` com `icon_for_path` e faz fallback para `icon_for_command`; `shell:AppsFolder\` não tem extração confiável e deve cair no glifo. O campo `icon: Option<String>` é serializado como `icon` com `skip_serializing_if`/`default`, e `LauncherWindow.tsx` renderiza `<img>` quando presente. Nunca inventar ícone: `None` = glifo semântico. Extração fica no `launcher_query` (que já roda em `spawn_blocking`) e o cache de `app_icons` evita repetir GDI.
**Atalhos `.url` (Steam):** `SHGetFileInfoW` resolve pela classe `internetshortcut` e nunca lê o arquivo, então mostrava documento genérico em vez do logo do jogo. `app_icons::parse_internet_shortcut` lê `IconFile=`/`IconIndex=` do `[InternetShortcut]` e `extract_icon_data_uri_at_index` usa `SHDefExtractIconW` no arquivo apontado (Steam guarda um `.ico` em `steam\games\`); sem `IconFile` válido, volta ao ícone do shell. Valide com `bash scripts/rust-check.sh` (espelho NTFS): o cargo do WSL não compila o código Windows.
