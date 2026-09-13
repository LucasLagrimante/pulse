# Build do Tauri no toolchain nativo do Windows

O Pulse deve ser compilado em um shell Windows com Rust MSVC, mesmo quando o repositório está armazenado em `C:\Users\...`.

Executar `npm run tauri dev` ou `npm run tauri build` a partir do WSL em `/mnt/c/...` faz o Tauri procurar um `cargo` Linux e pode resultar em `failed to run 'cargo metadata': No such file or directory (os error 2)`.

Use PowerShell, Prompt de Comando ou invoque explicitamente `powershell.exe` a partir do WSL. Antes do build, confirme `rustc --version` e `cargo --version` no mesmo shell que executará o Tauri.
