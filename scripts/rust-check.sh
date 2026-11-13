#!/usr/bin/env bash
# Gate do backend Rust a partir do WSL: espelha src-tauri/ na cópia NTFS do
# build nativo (%LOCALAPPDATA%\pulse-build\src) e roda o cargo do Windows lá,
# com o mesmo CARGO_TARGET_DIR do build:native (cache compartilhado).
#
# Uso: bash scripts/rust-check.sh [fmt] [clippy] [test] [--files a.rs,b.rs]
#   sem etapas      → fmt + clippy + test
#   fmt             → rustfmt --check só nos .rs alterados em relação ao HEAD
#                     (o repositório tem divergência de formatação antiga em
#                     outros arquivos; não misturar com a mudança atual)
#   --files         → lista explícita de .rs (relativos a src-tauri/src) para o fmt
#
# Os testes rodam com --test-threads=1: em paralelo a suíte derruba o processo
# com STATUS_ACCESS_VIOLATION de forma intermitente (pendência aberta).
# Testes conhecidos como dependentes da máquina (não bloqueiam release, ver
# memory/build/rust-check-testes-de-maquina.md):
#   services::tests::real_enumeration_reads_start_name_and_classifies
set -euo pipefail
cd "$(dirname "$0")/.."

CMD_EXE="/mnt/c/Windows/System32/cmd.exe"
MACHINE_TESTS=(services::tests::real_enumeration_reads_start_name_and_classifies)
STEPS=()
FILES=""
while [ $# -gt 0 ]; do
  case "$1" in
    fmt|clippy|test) STEPS+=("$1") ;;
    --files) FILES="$2"; shift ;;
    *) echo "argumento desconhecido: $1" >&2; exit 2 ;;
  esac
  shift
done
[ ${#STEPS[@]} -eq 0 ] && STEPS=(fmt clippy test)

WIN_BUILD_WIN="${PULSE_WIN_BUILD_DIR:-$("$CMD_EXE" /c 'echo %LOCALAPPDATA%\pulse-build' < /dev/null 2>/dev/null | tr -d '\r')}"
if [ -z "$WIN_BUILD_WIN" ] || [[ "$WIN_BUILD_WIN" == *%* ]]; then
  echo "não foi possível descobrir %LOCALAPPDATA% do Windows; defina PULSE_WIN_BUILD_DIR" >&2
  exit 1
fi
STAGING="$(wslpath -u "$WIN_BUILD_WIN\\src")"
CRATE_WIN="$WIN_BUILD_WIN\\src\\src-tauri"
TARGET_WIN="$WIN_BUILD_WIN\\target"

# Mesmo lock do build:native: os dois escrevem na mesma cópia NTFS.
exec 9>"${TMPDIR:-/tmp}/pulse-build-native.lock"
if ! flock -n 9; then
  echo "build:native em andamento; aguarde ele terminar" >&2
  exit 1
fi

echo "==> espelhando src-tauri/ em $STAGING/src-tauri"
mkdir -p "$STAGING/src-tauri" "$STAGING/dist"
rsync -a --delete --exclude target/ src-tauri/ "$STAGING/src-tauri/"
# O tauri-build exige o frontendDist; testes não precisam do bundle real.
[ -f "$STAGING/dist/index.html" ] || echo '<!doctype html>' > "$STAGING/dist/index.html"
node -e '
  const fs = require("fs"); const p = process.argv[1];
  const conf = JSON.parse(fs.readFileSync(p, "utf8"));
  conf.build = { ...conf.build, beforeBuildCommand: "" };
  fs.writeFileSync(p, JSON.stringify(conf, null, 2) + "\n");
' "$STAGING/src-tauri/tauri.conf.json"

run_cargo() {
  # cd no diretório NTFS antes do cmd.exe: com cwd no WSL ele cai em C:\Windows.
  (cd "$STAGING/src-tauri" && "$CMD_EXE" /c \
    "set CARGO_TARGET_DIR=$TARGET_WIN&& set PATH=%USERPROFILE%\\.cargo\\bin;%PATH%&& $*" < /dev/null)
}

for step in "${STEPS[@]}"; do
  case "$step" in
    fmt)
      if [ -z "$FILES" ]; then
        FILES="$( { git diff --name-only HEAD -- 'src-tauri/src/*.rs'; git ls-files --others --exclude-standard -- 'src-tauri/src/*.rs'; } \
          | sed 's#^src-tauri/src/##' | sort -u | paste -sd, -)"
      fi
      if [ -z "$FILES" ]; then
        echo "==> fmt: nenhum .rs alterado"
        continue
      fi
      WIN_FILES="$(echo "$FILES" | tr ',' '\n' | sed 's#/#\\#g; s#^#src\\#' | paste -sd' ' -)"
      echo "==> rustfmt --check: $FILES"
      run_cargo "rustfmt --check --edition 2021 $WIN_FILES"
      ;;
    clippy)
      echo "==> cargo clippy --lib --tests -D warnings"
      run_cargo "cargo clippy --lib --tests -q -- -D warnings"
      ;;
    test)
      # Em série: em paralelo a suíte cai com STATUS_ACCESS_VIOLATION de forma
      # intermitente (já no v1.5.0; ver memory/build/rust-check-testes-de-maquina.md).
      echo "==> cargo test --lib (em série)"
      SKIPS=""
      for t in "${MACHINE_TESTS[@]}"; do SKIPS="$SKIPS --skip $t"; done
      run_cargo "cargo test --lib -q -- --test-threads=1$SKIPS"
      # Testes que dependem do estado do Windows: rodam, mas só avisam.
      for t in "${MACHINE_TESTS[@]}"; do
        if run_cargo "cargo test --lib -q -- --exact $t" > /dev/null 2>&1; then
          echo "    ok (máquina): $t"
        else
          echo "    AVISO (máquina, não bloqueia): $t falhou — citar no relatório"
        fi
      done
      ;;
  esac
done
echo "==> rust-check OK (${STEPS[*]})"
