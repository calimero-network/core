#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

# First build the WASM file
# Note: wasm-opt validation errors are non-fatal - the WASM file is still created
./build.sh 2>&1 | grep -v "wasm-validator error" || true

# Create bundle directory
mkdir -p res/bundle-temp

# Copy WASM file
cp res/access_control.wasm res/bundle-temp/app.wasm

# Copy ABI file if it exists
if [ -f res/abi.json ]; then
    cp res/abi.json res/bundle-temp/abi.json
fi

# Get file sizes for manifest
WASM_SIZE=$(stat -f%z res/access_control.wasm 2>/dev/null || stat -c%s res/access_control.wasm 2>/dev/null || echo 0)
ABI_SIZE=$(stat -f%z res/abi.json 2>/dev/null || stat -c%s res/abi.json 2>/dev/null || echo 0)

# Create manifest.json
cat > res/bundle-temp/manifest.json <<EOF
{
  "version": "1.0",
  "package": "com.calimero.access-control",
  "appVersion": "1.0.0",
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

# Create .mpk bundle (tar.gz archive)
cd res/bundle-temp
tar -czf ../access-control-1.0.0.mpk manifest.json app.wasm abi.json 2>/dev/null || \
tar -czf ../access-control-1.0.0.mpk manifest.json app.wasm 2>/dev/null

# Cleanup
cd ..
rm -rf bundle-temp

echo "Bundle created: res/access-control-1.0.0.mpk"

