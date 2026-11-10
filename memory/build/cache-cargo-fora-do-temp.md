---
name: cache-cargo-fora-do-temp
description: Staging e CARGO_TARGET_DIR do build nativo ficam em %LOCALAPPDATA%\pulse-build, nunca no %TEMP% — limpeza por idade quebrava todo build (bindgen.rs ausente)
module: build
metadata:
  type: project
  pin: true
---
Erro recorrente `couldn't find file ...\libsqlite3-sys-*\out/bindgen.rs`: o build script copia o bindgen pronto com `fs::copy`, que preserva o mtime antigo; um plano de limpeza da IA ("Temp, arquivos > 7 dias") apagou só esse arquivo dentro de `Temp\pulse-target`, e o cargo nunca reroda o build script (fingerprint em dia).
**Why:** qualquer limpeza por idade (Storage Sense, CCleaner, plano de IA) remove arquivos soltos do cache e deixa o build quebrado para sempre, sem causa aparente.
**How to apply:** `build-native.sh` e `run-installer.mjs` usam `%LOCALAPPDATA%\pulse-build\{src,target}` (`PULSE_WIN_BUILD_DIR` sobrescreve). O script apaga `build/libsqlite3-sys-*` quando há `libsqlite3.a` sem `bindgen.rs`. O prompt da IA de disco manda excluir `pulse-*` da TEMP e nunca apagar por idade dentro de caches de build. Correção manual: apagar a pasta `libsqlite3-sys-*` do `target\<perfil>\build`.
