---
name: icones-apps-startup
description: Ícones por item de inicialização extraídos nativamente (SHGetFileInfoW + GDI → PNG data URI) e cacheados
module: inicializacao
metadata:
  type: project
---
**Why:** A tela de Inicialização precisava do ícone de cada app. O `StartupItem` (Rust/TS) não tinha campo de ícone. Extrair por PowerShell seria subprocesso pesado; a extração nativa evita isso e respeita a regra de não exibir dado fictício (ausência vira fallback, nunca ícone falso).
**How to apply:** Use `crate::app_icons::icon_for_command(command)`. O módulo resolve o exe do comando `Run` (aspas, caminhos com espaço via extensão, expansão `%VAR%`, busca no `PATH`), lê o ícone com `SHGetFileInfoW(SHGFI_ICON|SHGFI_LARGEICON)`, converte HICON→RGBA via `GetIconInfo`+`GetDIBits` (máscara AND quando não há alfa) e codifica PNG (`png` crate) para data URI. Cacheia por caminho, inclusive falhas. Features `windows` necessárias: `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging`, `Win32_Graphics_Gdi`, `Win32_Storage_FileSystem`, `Win32_Foundation`.
