#!/usr/bin/env bash
# Bundle one migration-fixture wasm WITHOUT embedding an ABI - the one thing
# `cargo mero bundle` cannot do, and what the missing-ABI-refusal scenario needs.
#
#   bundle-wasm.sh <suite-dir> <wasm-file> <package> <app-version>
#
# v1/v2 of a scenario pair MUST share <package>: a bundle's ApplicationId is
# hash(package, signer), so the same package yields the SAME id across
# versions - the realistic shape for app upgrades (the version delta lives in
# the bytecode blob). Output: <suite-dir>/res/<package-leaf>-<app-version>.mpk
set -euo pipefail

SUITE_DIR="$1"
WASM_FILE="$2"
PACKAGE="$3"
APP_VERSION="$4"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PATH="$("$REPO_ROOT/scripts/setup-cargo-mero.sh"):$PATH"

cd "$SUITE_DIR"

if [ ! -f "res/$WASM_FILE" ]; then
    echo "ERROR: res/$WASM_FILE not found (build the suite first)" >&2
    exit 1
fi

mkdir -p res/bundle-temp
rm -f res/bundle-temp/*

cp "res/$WASM_FILE" res/bundle-temp/app.wasm
if [ -f res/abi.json ]; then
    cp res/abi.json res/bundle-temp/abi.json
fi

size() { stat -f%z "$1" 2>/dev/null || stat -c%s "$1"; }
sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }

WASM_SIZE=$(size res/bundle-temp/app.wasm)
WASM_HASH=$(sha256 res/bundle-temp/app.wasm)

# Declared only when the file was actually staged: a required hash cannot
# describe an artifact that is not in the archive.
ABI_BLOCK=""
if [ -f res/bundle-temp/abi.json ]; then
    ABI_BLOCK=$(cat <<ABI
  "abi": {
    "path": "abi.json",
    "size": $(size res/bundle-temp/abi.json),
    "hash": "$(sha256 res/bundle-temp/abi.json)"
  },
ABI
)
fi

cat > res/bundle-temp/manifest.json <<EOF
{
  "version": "1.0",
  "package": "${PACKAGE}",
  "appVersion": "${APP_VERSION}",
  "minRuntimeVersion": "0.1.0",
  "wasm": {
    "path": "app.wasm",
    "size": ${WASM_SIZE},
    "hash": "${WASM_HASH}"
  },
${ABI_BLOCK}
  "migrations": []
}
EOF

cargo mero sign res/bundle-temp/manifest.json --dev

LEAF="${PACKAGE##*.}"
OUT="${LEAF}-${APP_VERSION}.mpk"

cd res/bundle-temp
tar -czf "../$OUT" manifest.json app.wasm abi.json 2>/dev/null ||
    tar -czf "../$OUT" manifest.json app.wasm
cd ..
rm -rf bundle-temp

echo "Bundle created: $SUITE_DIR/res/$OUT"
