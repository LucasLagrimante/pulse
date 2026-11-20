#!/usr/bin/env bash
# ============================================================================
#  Build nativa do Pulse para macOS (.app + .dmg — experimental, sem assinatura)
# ============================================================================
#
#  USO (em macOS 11+ com Xcode Command Line Tools + Rust + Node 22+):
#    npm run build:macos                       # frontend + .app + .dmg + sync landing
#    npm run build:macos -- --no-landing       # sem tocar em landing/
#    npm run build:macos -- --rust             # reaproveita dist/ atual
#    npm run build:macos -- --install          # abre o .dmg gerado no Finder
# ============================================================================

set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$(uname -s)" != "Darwin" ]; then
  echo "build-macos.sh só pode ser executado no macOS (detectado: $(uname -s))." >&2
  exit 1
fi

SKIP_FRONTEND=0
OPEN_DMG=0
SYNC_LANDING=1

for arg in "$@"; do
  case "$arg" in
    --rust) SKIP_FRONTEND=1 ;;
    --install) OPEN_DMG=1 ;;
    --no-landing) SYNC_LANDING=0 ;;
    *) echo "argumento desconhecido: $arg" >&2; exit 2 ;;
  esac
done

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERRO: cargo não encontrado no PATH do macOS." >&2
  echo "Instale o Rust stable: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  exit 1
fi

node scripts/kill-running-app.mjs

if [ "$SKIP_FRONTEND" -eq 0 ]; then
  echo "==> 1/4  frontend (vite build)"
  npm run build
else
  echo "==> 1/4  frontend ignorado (--rust, reaproveitando dist/)"
fi

echo "==> 2/4  limpando .dmg antigos"
rm -f src-tauri/target/release/bundle/dmg/*.dmg \
      src-tauri/target/release/bundle/SHA256SUMS-macos.txt

echo "==> 3/4  tauri build (macOS experimental: app, dmg)"
npx tauri build --bundles app,dmg --config '{"build":{"beforeBuildCommand":""}}' -- --locked

DMG_FILE="$(find src-tauri/target/release/bundle/dmg -maxdepth 1 -name '*.dmg' | head -n 1 || true)"
if [ -z "$DMG_FILE" ]; then
  echo "ERRO: nenhum .dmg foi gerado em src-tauri/target/release/bundle/dmg/." >&2
  exit 1
fi

(
  cd src-tauri/target/release/bundle/dmg
  shasum -a 256 *.dmg > ../SHA256SUMS-macos.txt
)

if [ "$SYNC_LANDING" -eq 1 ]; then
  echo "==> 4/4  sincronizando metadados com a Landing Page"
  node scripts/sync-landing.mjs
else
  echo "==> 4/4  sync da Landing Page ignorado (--no-landing)"
fi

echo
echo "==> artefato macOS (experimental, sem assinatura/notarização)"
ls -lh "$DMG_FILE"
cat src-tauri/target/release/bundle/SHA256SUMS-macos.txt
echo "    Nota Gatekeeper: clique com botão direito → Abrir, ou:"
echo "    xattr -dr com.apple.quarantine /Applications/Pulse.app"

if [ "$OPEN_DMG" -eq 1 ]; then
  open "$DMG_FILE"
fi
