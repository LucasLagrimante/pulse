#!/usr/bin/env bash
# ============================================================================
#  Build nativa do Pulse para Linux (.deb + .AppImage, ou .rpm)
# ============================================================================
#
#  USO (em Linux nativo ou WSL com toolchain Linux instalado):
#    npm run build:linux                       # frontend + .deb + .AppImage + sync landing
#    npm run build:linux -- --no-landing       # sem tocar em landing/
#    npm run build:linux -- --rust             # reaproveita dist/ atual
#    npm run build:linux -- --install          # instala o .deb gerado via apt
#    npm run build:linux:rpm                   # gera pacote .rpm (requer rpmbuild)
# ============================================================================

set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$(uname -s)" != "Linux" ]; then
  echo "build-linux.sh só pode ser executado no Linux (detectado: $(uname -s))." >&2
  exit 1
fi

SKIP_FRONTEND=0
RUN_INSTALL=0
SYNC_LANDING=1
BUNDLES="deb,appimage"

for arg in "$@"; do
  case "$arg" in
    --rust) SKIP_FRONTEND=1 ;;
    --install) RUN_INSTALL=1 ;;
    --no-landing) SYNC_LANDING=0 ;;
    --rpm) BUNDLES="rpm" ;;
    --all) BUNDLES="deb,appimage,rpm" ;;
    *) echo "argumento desconhecido: $arg" >&2; exit 2 ;;
  esac
done

exec 9>"${TMPDIR:-/tmp}/pulse-build-linux.lock"
if ! flock -n 9; then
  echo "outro build:linux já está em andamento; aguarde ele terminar" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERRO: cargo não encontrado no PATH do Linux." >&2
  echo "Instale o Rust stable: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  exit 1
fi

if command -v pkg-config >/dev/null 2>&1 && ! pkg-config --exists webkit2gtk-4.1; then
  echo "ERRO: libwebkit2gtk-4.1-dev não encontrada." >&2
  echo "Debian/Ubuntu/Mint: sudo apt install -y build-essential curl file patchelf libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libxdo-dev" >&2
  exit 1
fi

if [[ "$BUNDLES" == *rpm* ]] && ! command -v rpmbuild >/dev/null 2>&1; then
  echo "ERRO: rpmbuild não encontrado (instale o pacote 'rpm' para gerar .rpm)." >&2
  exit 1
fi

# linuxdeploy usa AppImage internamente; sem /dev/fuse ou libfuse2 precisa extrair.
if [ ! -e /dev/fuse ] || ! ldconfig -p 2>/dev/null | grep -q 'libfuse\.so\.2'; then
  export APPIMAGE_EXTRACT_AND_RUN=1
fi

node scripts/kill-running-app.mjs

if [ "$SKIP_FRONTEND" -eq 0 ]; then
  echo "==> 1/4  frontend (vite build)"
  npm run build
else
  echo "==> 1/4  frontend ignorado (--rust, reaproveitando dist/)"
fi

echo "==> 2/4  limpando pacotes de versões antigas"
rm -f src-tauri/target/release/bundle/deb/*.deb \
      src-tauri/target/release/bundle/appimage/*.AppImage \
      src-tauri/target/release/bundle/rpm/*.rpm \
      src-tauri/target/release/bundle/SHA256SUMS-linux.txt

echo "==> 3/4  tauri build (Linux: $BUNDLES)"
npx tauri build --bundles "$BUNDLES" --config '{"build":{"beforeBuildCommand":""}}' -- --locked

mkdir -p src-tauri/target/release/bundle
mapfile -t GENERATED < <(find src-tauri/target/release/bundle -maxdepth 2 -type f \( -name '*.deb' -o -name '*.AppImage' -o -name '*.rpm' \) | sort)
if [ "${#GENERATED[@]}" -eq 0 ]; then
  echo "ERRO: nenhum pacote Linux foi gerado em src-tauri/target/release/bundle/." >&2
  exit 1
fi

(
  cd src-tauri/target/release/bundle
  : > SHA256SUMS-linux.txt
  for file in deb/*.deb appimage/*.AppImage rpm/*.rpm; do
    [ -f "$file" ] || continue
    sha256sum "$file" | sed 's|  [^/]*/|  |' >> SHA256SUMS-linux.txt
  done
)

if [ "$SYNC_LANDING" -eq 1 ]; then
  echo "==> 4/4  sincronizando metadados com a Landing Page"
  node scripts/sync-landing.mjs
else
  echo "==> 4/4  sync da Landing Page ignorado (--no-landing)"
fi

echo
echo "==> artefatos Linux"
for file in "${GENERATED[@]}"; do
  ls -lh "$file"
done
echo "==> SHA-256 (src-tauri/target/release/bundle/SHA256SUMS-linux.txt)"
cat src-tauri/target/release/bundle/SHA256SUMS-linux.txt

if [ "$RUN_INSTALL" -eq 1 ]; then
  DEB_FILE="$(find src-tauri/target/release/bundle/deb -maxdepth 1 -name '*.deb' | head -n 1 || true)"
  if [ -n "$DEB_FILE" ] && command -v apt >/dev/null 2>&1; then
    echo "==> instalando $DEB_FILE via apt"
    sudo apt install -y "./$DEB_FILE"
  else
    echo "Aviso: --install requer um .deb gerado e apt disponível; execute o AppImage diretamente."
  fi
fi
