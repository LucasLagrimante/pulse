#!/usr/bin/env bash
# ============================================================================
#  Build nativa do Pulse — frontend no WSL, backend Rust/MSVC no Windows
# ============================================================================
#
#  POR QUE EXISTE ESTE SCRIPT
#  O repositório vive no WSL (~/projects/pulse). O frontend (vite/tsc/vitest)
#  é cross-platform e roda nativo no WSL. O backend Rust precisa de toolchain
#  MSVC, do vcvars do Visual Studio e do link.exe — que só existem no Windows.
#
#  ARQUITETURA VALIDADA (o caminho que funciona):
#   1. Um executável Windows chamado de dentro do WSL resolve caminhos do WSL
#      pelo interop, então cargo.exe enxerga as fontes.
#   2. Processos Windows NÃO acessam \\wsl.localhost\... — PowerShell e cmd
#      falham com "UNC paths are not supported" e caem em C:\Windows. Por isso
#      o cmd.exe precisa de um cwd em NTFS.
#   3. Compilar direto sobre o filesystem do WSL é inviavel (9p/UNC é ordens
#      de magnitude mais lento), então:
#        - as fontes são copiadas para uma area de troca em NTFS;
#        - CARGO_TARGET_DIR tambem fica em NTFS;
#        - ao fim, .exe e instalador são copiados de volta para o WSL.
#   4. O bundler NSIS é o CLI npm (`npm run tauri:build`), NÃO um subcomando do
#      cargo — `cargo tauri` não existe.
#   5. Aninhar aspas de caminhos com espaço ("Program Files (x86)") dentro de
#      `cmd /c "..."` corrompe o comando. A solução é um wrapper .bat gravado
#      em NTFS, onde o quoting fica literal.
#
#  USO (de dentro do WSL, na raiz do projeto):
#    npm run build:native              # frontend + backend + instalador NSIS
#    npm run build:native -- --rust    # so backend + instalador
#    scripts/build-native.sh --rust
#    npm run build:native -- --no-landing  # não toca em landing/ (sem sync)
# ============================================================================

set -euo pipefail
cd "$(dirname "$0")/.."

SKIP_FRONTEND=0
RUN_INSTALLER=0
SYNC_LANDING=1
for arg in "$@"; do
  case "$arg" in
    --rust) SKIP_FRONTEND=1 ;;
    --install) RUN_INSTALLER=1 ;;
    --no-landing) SYNC_LANDING=0 ;;
    *) echo "argumento desconhecido: $arg" >&2; exit 2 ;;
  esac
done

CMD_EXE="/mnt/c/Windows/System32/cmd.exe"

# Dois builds simultâneos compartilham staging e target: um apaga as fontes do
# outro no meio da compilação ("couldn't find file build.rs" / Permission denied).
exec 9>"${TMPDIR:-/tmp}/pulse-build-native.lock"
if ! flock -n 9; then
  echo "outro build:native já está em andamento; aguarde ele terminar" >&2
  exit 1
fi

# Pastas de trabalho em %LOCALAPPDATA%\pulse-build do usuário Windows atual
# (sem caminho fixo de usuário). NÃO usar %TEMP%: limpezas por idade (plano de
# IA, Storage Sense, CCleaner) apagam arquivos antigos de dentro do cache do
# cargo — ex.: o bindgen.rs do libsqlite3-sys, copiado com mtime original — e o
# cargo não reroda o build script, quebrando todo build seguinte.
# PULSE_WIN_BUILD_DIR permite sobrescrever, ex.: 'D:\pulse-build'.
WIN_BUILD_WIN="${PULSE_WIN_BUILD_DIR:-$("$CMD_EXE" /c 'echo %LOCALAPPDATA%\pulse-build' < /dev/null 2>/dev/null | tr -d '\r')}"
if [ -z "$WIN_BUILD_WIN" ] || [[ "$WIN_BUILD_WIN" == *%* ]]; then
  echo "não foi possível descobrir %LOCALAPPDATA% do Windows; defina PULSE_WIN_BUILD_DIR" >&2
  exit 1
fi
STAGING_WIN="$WIN_BUILD_WIN\src"
TARGET_DIR_WIN="$WIN_BUILD_WIN\target"
STAGING="$(wslpath -u "$STAGING_WIN")"
TARGET_DIR="$(wslpath -u "$TARGET_DIR_WIN")"
export REPO_DIR="$PWD" STAGING

node scripts/kill-running-app.mjs

if [ "$SKIP_FRONTEND" -eq 0 ]; then
  echo "==> 1/5  frontend (vite build, nativo no WSL)"
  npm run build
fi

echo "==> 2/5  preparando copia de trabalho em NTFS"
rm -rf "$STAGING/src-tauri" "$STAGING/dist" "$STAGING/scripts" "$STAGING/package.json" "$STAGING/package-lock.json"
mkdir -p "$STAGING"
cp -r src-tauri "$STAGING/"
cp package.json package-lock.json "$STAGING/"
# O script tauri:build chama scripts/kill-running-app.mjs e
# scripts/run-installer.mjs; sem a pasta o bundler morre com MODULE_NOT_FOUND.
cp -r scripts "$STAGING/"
rm -rf "$STAGING/src-tauri/target"

# O tauri.conf.json aponta para ../dist; o bundler precisa do frontend gerado.
mkdir -p "$STAGING/dist"
cp -r dist/. "$STAGING/dist/"

# Provisiona o CLI nativo do Tauri para Windows em NTFS (sem depender de npm.cmd via interop)
if [ ! -f "$STAGING/node_modules/@tauri-apps/cli-win32-x64-msvc/cli.win32-x64-msvc.node" ]; then
  echo "    provisionando @tauri-apps/cli-win32-x64-msvc em NTFS"
  mkdir -p "$STAGING/node_modules/@tauri-apps/cli-win32-x64-msvc"
  cp -r node_modules/@tauri-apps/cli "$STAGING/node_modules/@tauri-apps/"
  npm pack @tauri-apps/cli-win32-x64-msvc@2.11.5 --pack-destination /tmp >/dev/null
  tar -xzf /tmp/tauri-apps-cli-win32-x64-msvc-2.11.5.tgz -C "$STAGING/node_modules/@tauri-apps/cli-win32-x64-msvc" --strip-components=1
  rm -f /tmp/tauri-apps-cli-win32-x64-msvc-2.11.5.tgz
fi

# O tauri.conf.json declara beforeBuildCommand "npm run build"; a cópia NTFS não
# precisa recompilar o frontend pois já foi gerado na etapa 1/5 no WSL.
python3 - <<'PYEOF'
import json, io, os
src = os.path.join(os.environ["REPO_DIR"], "src-tauri", "tauri.conf.json")
dst = os.path.join(os.environ["STAGING"], "src-tauri", "tauri.conf.json")
with io.open(src, "r", encoding="utf-8") as fh:
    conf = json.load(fh)
conf.setdefault("build", {})["beforeBuildCommand"] = ""
with io.open(dst, "w", encoding="utf-8") as fh:
    fh.write(json.dumps(conf, indent=2, ensure_ascii=False) + "\n")
print("    tauri.conf.json: beforeBuildCommand anulado (frontend já gerado)")
PYEOF

# Autocorreção: se algo apagou o bindgen.rs gerado pelo libsqlite3-sys, o
# cargo considera o build script em dia e falha sempre com "couldn't find file
# .../out/bindgen.rs". Remover a pasta força o build script a rodar de novo.
for out_dir in "$TARGET_DIR"/*/build/libsqlite3-sys-*/out; do
  [ -d "$out_dir" ] || continue
  if [ -f "$out_dir/libsqlite3.a" ] && [ ! -f "$out_dir/bindgen.rs" ]; then
    echo "    cache do libsqlite3-sys incompleto (bindgen.rs ausente); limpando $(dirname "$out_dir")"
    rm -rf "$(dirname "$out_dir")"
  fi
done

echo "==> 3/5  build Rust + instalador NSIS (Windows, via wrapper .bat)"
# Wrapper em NTFS: o quoting literal evita a corrupcao de aspas em caminhos
# com espaço ("Program Files (x86)") dentro de `cmd /c "..."`.
cat > "$STAGING/run-build.bat" <<BATEOF
@echo off
setlocal
taskkill /IM pulse-windows.exe /F >nul 2>&1
set "CARGO_TARGET_DIR=$TARGET_DIR_WIN"
rem PATH herdado do WSL pode vir sem .cargo\bin (ambiente da sessão WSL defasado)
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>&1 || (echo cargo nao encontrado no PATH do Windows & exit /b 4)
call "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 2
cd /d "$STAGING_WIN\src-tauri" || exit /b 3
echo === tauri build (Rust release + bundler NSIS) ===
chcp 65001 >nul
node "$STAGING_WIN\node_modules\@tauri-apps\cli\tauri.js" build --bundles nsis -- --locked
exit /b %errorlevel%
BATEOF

(cd "$STAGING" && "$CMD_EXE" /c run-build.bat < /dev/null)

echo "==> 4/5  copiando artefatos de volta para o WSL"
mkdir -p src-tauri/target/release
cp -f "$TARGET_DIR/release/pulse-windows.exe" src-tauri/target/release/ 2>/dev/null || true
cp -rf "$TARGET_DIR/release/bundle" src-tauri/target/release/ 2>/dev/null || true

if [ "$SYNC_LANDING" -eq 1 ]; then
  echo "==> 5/5  sincronizando instalador e metadados com a Landing Page"
  node scripts/sync-landing.mjs
else
  echo "==> 5/5  sync da Landing Page ignorado (--no-landing)"
fi

echo
echo "==> artefatos"
ls -la src-tauri/target/release/pulse-windows.exe 2>/dev/null || true
find src-tauri/target/release/bundle -name '*.exe' 2>/dev/null | sed 's/^/    /' || true
echo
echo "==> build nativa + landing page sincronizadas com sucesso"
echo "    release oficial (CI): git tag v<versao> && git push --tags; depois npm run lp:sync:release"

if [ "$RUN_INSTALLER" -eq 1 ]; then
  node scripts/run-installer.mjs
fi

