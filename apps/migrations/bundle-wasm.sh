#!/usr/bin/env bash
# Wrap one migration-fixture wasm into a signed single-service `.mpk` bundle.
#
#   bundle-wasm.sh <suite-dir> <wasm-file> <package> <app-version>
#
# v1/v2 of a scenario pair MUST share <package>: a bundle's ApplicationId is
# hash(package, signer), so the same package yields the SAME id across
# versions — the realistic shape for app upgrades (the version delta lives in
# the bytecode blob). Output: <suite-dir>/res/<package-leaf>-<app-version>.mpk
set -euo pipefail

SUITE_DIR="$1"
WASM_FILE="$2"
PACKAGE="$3"
APP_VERSION="$4"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

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
WASM_SIZE=$(size res/bundle-temp/app.wasm)
ABI_SIZE=$( [ -f res/bundle-temp/abi.json ] && size res/bundle-temp/abi.json || echo 0)

cat > res/bundle-temp/manifest.json <<EOF
{
  "version": "1.0",
  "package": "${PACKAGE}",
  "appVersion": "${APP_VERSION}",
  "minRuntimeVersion": "0.1.0",
  "wasm": {
    "path": "app.wasm",
    "size": ${WASM_SIZE},
    "hash": null
  },
  "abi": {
    "path": "abi.json",
    "size": ${ABI_SIZE},
    "hash": null
  },
  "migrations": []
}
EOF

cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -p mero-sign --quiet -- \
    sign res/bundle-temp/manifest.json \
    --key "$REPO_ROOT/scripts/test-signing-key/test-key.json"

LEAF="${PACKAGE##*.}"
OUT="${LEAF}-${APP_VERSION}.mpk"

cd res/bundle-temp
tar -czf "../$OUT" manifest.json app.wasm abi.json 2>/dev/null ||
    tar -czf "../$OUT" manifest.json app.wasm
cd ..
rm -rf bundle-temp

echo "Bundle created: $SUITE_DIR/res/$OUT"
