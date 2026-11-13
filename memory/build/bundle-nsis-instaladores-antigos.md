---
name: bundle-nsis-instaladores-antigos
description: O tauri build não limpa bundle/nsis; o build-native deixa só Pulse_<versão>_x64-setup.exe e o run-installer escolhe pela versão, nunca pela data
module: build
metadata:
  type: project
---
Em 2026-11-13, depois da renumeração para 1.2.0, o `build:native -- --install` listou também `Pulse_1.4.0` e `Pulse_1.5.0`, que tinham sobrado de builds antigos na pasta `bundle/nsis`. O `cp -rf` de volta para o WSL dava a mesma data a todos, e o `run-installer` escolhia o mais recente pela data, então podia abrir um instalador antigo.
**Why:** o Tauri não apaga instaladores de versões anteriores. Comparar pela data não funciona depois de uma cópia que reescreve o mtime.
**How to apply:** o `build-native.sh` apaga `*-setup.exe` antes do build (no NTFS) e copia só o instalador da versão do `tauri.conf.json`. O `run-installer.mjs` abre `Pulse_<versão>_x64-setup.exe` e falha se ele não existir. Nunca volte a escolher artefato pela data.
