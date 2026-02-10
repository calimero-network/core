#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../../target}"

./build.sh 2>&1 | grep -v "wasm-validator error" || true

mkdir -p res/bundle-temp

cp res/migration_suite_v1.wasm res/bundle-temp/app.wasm

if [ -f res/abi.json ]; then
    cp res/abi.json res/bundle-temp/abi.json
fi

WASM_SIZE=$(stat -f%z res/migration_suite_v1.wasm 2>/dev/null || stat -c%s res/migration_suite_v1.wasm 2>/dev/null || echo 0)
ABI_SIZE=$(stat -f%z res/abi.json 2>/dev/null || stat -c%s res/abi.json 2>/dev/null || echo 0)

cat > res/bundle-temp/manifest.json <<EOF
{
  "version": "1.0",
  "package": "com.calimero.migration-suite",
  "appVersion": "1.0.0",
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

cargo run -p mero-sign --quiet -- sign res/bundle-temp/manifest.json \
    --key ../../../scripts/test-signing-key/test-key.json

cd res/bundle-temp
tar -czf ../migration-suite-1.0.0.mpk manifest.json app.wasm abi.json 2>/dev/null || \
tar -czf ../migration-suite-1.0.0.mpk manifest.json app.wasm 2>/dev/null

cd ..
rm -rf bundle-temp

echo "Bundle created: res/migration-suite-1.0.0.mpk"
