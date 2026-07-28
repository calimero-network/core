#!/bin/bash
set -e

cd "$(dirname $0)"

# Hand-rolls a two-service manifest from one crate, which a workspace-level
# services table cannot express, so it stays outside the tool.
PATH="$(../../scripts/setup-cargo-mero.sh):$PATH"
cargo mero build --manifest-path Cargo.toml

mkdir -p res/multi-bundle-temp

# Both services use the same WASM - this is what we want to test:
# the multi-service bundle install + service_name selection path in merod.
cp res/scaffolding_e2e.wasm res/multi-bundle-temp/store-a.wasm
cp res/scaffolding_e2e.wasm res/multi-bundle-temp/store-b.wasm

ABI_ARGS=""
if [ -f res/abi.json ]; then
    cp res/abi.json res/multi-bundle-temp/store-a-abi.json
    cp res/abi.json res/multi-bundle-temp/store-b-abi.json
    ABI_ARGS="store-a-abi.json store-b-abi.json"
fi

sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }

WASM_SIZE=$(stat -f%z res/scaffolding_e2e.wasm 2>/dev/null || stat -c%s res/scaffolding_e2e.wasm)
WASM_HASH=$(sha256 res/scaffolding_e2e.wasm)
# Declared only when the file was actually staged: a required hash cannot
# describe an artifact that is not in the archive.
abi_block() {
    [ -f res/abi.json ] || return 0
    cat <<ABI
,
      "abi": {
        "path": "$1-abi.json",
        "size": $(stat -f%z res/abi.json 2>/dev/null || stat -c%s res/abi.json),
        "hash": "$(sha256 res/abi.json)"
      }
ABI
}

service_block() {
    cat <<SVC
    {
      "name": "$1",
      "wasm": {
        "path": "$1.wasm",
        "size": ${WASM_SIZE},
        "hash": "${WASM_HASH}"
      }$(abi_block "$1")
    }
SVC
}

cat > res/multi-bundle-temp/manifest.json <<EOF
{
  "version": "1.0",
  "package": "com.calimero.scaffolding-e2e-multi",
  "appVersion": "0.1.0",
  "minRuntimeVersion": "0.0.0",
  "services": [
$(service_block store-a),
$(service_block store-b)
  ],
  "migrations": []
}
EOF

cargo mero sign res/multi-bundle-temp/manifest.json --dev

cd res/multi-bundle-temp
tar -czf ../scaffolding-e2e-multi-0.1.0.mpk manifest.json store-a.wasm store-b.wasm ${ABI_ARGS} 2>/dev/null || \
tar -czf ../scaffolding-e2e-multi-0.1.0.mpk manifest.json store-a.wasm store-b.wasm 2>/dev/null

cd ..
rm -rf multi-bundle-temp

echo "Multi-service bundle created: res/scaffolding-e2e-multi-0.1.0.mpk"
