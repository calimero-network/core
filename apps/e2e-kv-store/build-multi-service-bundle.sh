#!/bin/bash
set -e

cd "$(dirname $0)"

# Build the WASM first
# Note: wasm-opt validation errors are non-fatal
./build.sh 2>&1 | grep -v "wasm-validator error" || true

mkdir -p res/multi-bundle-temp

# Both services use the same WASM — this is what we want to test:
# the multi-service bundle install + service_name selection path in merod.
cp res/e2e_kv_store.wasm res/multi-bundle-temp/store-a.wasm
cp res/e2e_kv_store.wasm res/multi-bundle-temp/store-b.wasm

ABI_ARGS=""
if [ -f res/abi.json ]; then
    cp res/abi.json res/multi-bundle-temp/store-a-abi.json
    cp res/abi.json res/multi-bundle-temp/store-b-abi.json
    ABI_ARGS="store-a-abi.json store-b-abi.json"
fi

WASM_SIZE=$(stat -f%z res/e2e_kv_store.wasm 2>/dev/null || stat -c%s res/e2e_kv_store.wasm 2>/dev/null || echo 0)
ABI_SIZE=0
if [ -f res/abi.json ]; then
    ABI_SIZE=$(stat -f%z res/abi.json 2>/dev/null || stat -c%s res/abi.json 2>/dev/null || echo 0)
fi

cat > res/multi-bundle-temp/manifest.json <<EOF
{
  "version": "1.0",
  "package": "com.calimero.e2e-kv-store-multi",
  "appVersion": "0.1.0",
  "services": [
    {
      "name": "store-a",
      "wasm": {
        "path": "store-a.wasm",
        "size": ${WASM_SIZE},
        "hash": null
      },
      "abi": {
        "path": "store-a-abi.json",
        "size": ${ABI_SIZE},
        "hash": null
      }
    },
    {
      "name": "store-b",
      "wasm": {
        "path": "store-b.wasm",
        "size": ${WASM_SIZE},
        "hash": null
      },
      "abi": {
        "path": "store-b-abi.json",
        "size": ${ABI_SIZE},
        "hash": null
      }
    }
  ],
  "migrations": []
}
EOF

cd res/multi-bundle-temp
tar -czf ../e2e-kv-store-multi-0.1.0.mpk manifest.json store-a.wasm store-b.wasm ${ABI_ARGS} 2>/dev/null || \
tar -czf ../e2e-kv-store-multi-0.1.0.mpk manifest.json store-a.wasm store-b.wasm 2>/dev/null

cd ..
rm -rf multi-bundle-temp

echo "Multi-service bundle created: res/e2e-kv-store-multi-0.1.0.mpk"
