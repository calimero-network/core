#!/usr/bin/env bash
set -euo pipefail

# Verify state schema extraction from both build-time and WASM

ROOT="${ROOT:-$(git rev-parse --show-toplevel)}"
cd "$ROOT"

echo "=== State Schema Conformance Test ==="
echo ""

# Build the app
echo "1. Building state-schema-conformance..."
cargo build -p state-schema-conformance --target wasm32-unknown-unknown

# Check build-time generated schema
echo ""
echo "2. Checking build-time generated state schema..."
BUILD_TIME_SCHEMA="apps/state-schema-conformance/res/state-schema.json"
if [ ! -f "$BUILD_TIME_SCHEMA" ]; then
    echo "ERROR: Build-time state schema not found at $BUILD_TIME_SCHEMA"
    exit 1
fi

# Normalize build-time schema (extract only state_root and types)
python3 << 'PYTHON_SCRIPT'
import json
import sys

with open('apps/state-schema-conformance/res/state-schema.json') as f:
    schema = json.load(f)

normalized = {
    'state_root': schema.get('state_root'),
    'types': schema.get('types', {})
}

with open('/tmp/build-time-state-schema.json', 'w') as f:
    json.dump(normalized, f, indent=2, sort_keys=True)

print(f"✅ Build-time schema: state_root={normalized['state_root']}, types={len(normalized['types'])}")
PYTHON_SCRIPT

# Build the extractor if needed
EXTRACTOR="$ROOT/target/debug/mero-abi"
if [ ! -x "$EXTRACTOR" ]; then
    echo ""
    echo "3. Building calimero-abi extractor..."
    cargo build -p mero-abi
fi

# Extract from WASM
echo ""
echo "4. Extracting state schema from WASM..."
WASM_FILE="target/wasm32-unknown-unknown/debug/state_schema_conformance.wasm"
if [ ! -f "$WASM_FILE" ]; then
    echo "ERROR: WASM file not found at $WASM_FILE"
    exit 1
fi

"$EXTRACTOR" state "$WASM_FILE" -o /tmp/wasm-extracted-state-schema.json

# Normalize WASM-extracted schema
python3 << 'PYTHON_SCRIPT'
import json

with open('/tmp/wasm-extracted-state-schema.json') as f:
    schema = json.load(f)

normalized = {
    'state_root': schema.get('state_root'),
    'types': schema.get('types', {})
}

with open('/tmp/wasm-extracted-state-schema.json', 'w') as f:
    json.dump(normalized, f, indent=2, sort_keys=True)

print(f"✅ WASM-extracted schema: state_root={normalized['state_root']}, types={len(normalized['types'])}")
PYTHON_SCRIPT

# Compare with expected
echo ""
echo "5. Comparing with expected state schema..."
EXPECTED="apps/state-schema-conformance/state-schema.expected.json"

if [ ! -f "$EXPECTED" ]; then
    echo "WARNING: Expected state schema not found, creating from build-time schema..."
    cp /tmp/build-time-state-schema.json "$EXPECTED"
fi

# Compare build-time with expected
if ! diff -u "$EXPECTED" /tmp/build-time-state-schema.json > /tmp/build-time-diff.txt; then
    echo "ERROR: Build-time state schema differs from expected:"
    cat /tmp/build-time-diff.txt
    exit 1
fi
echo "✅ Build-time schema matches expected"

# Compare WASM-extracted with expected
if ! diff -u "$EXPECTED" /tmp/wasm-extracted-state-schema.json > /tmp/wasm-diff.txt; then
    echo "ERROR: WASM-extracted state schema differs from expected:"
    cat /tmp/wasm-diff.txt
    exit 1
fi
echo "✅ WASM-extracted schema matches expected"

# Compare build-time with WASM-extracted
if ! diff -u /tmp/build-time-state-schema.json /tmp/wasm-extracted-state-schema.json > /tmp/cross-diff.txt; then
    echo "ERROR: Build-time and WASM-extracted schemas differ:"
    cat /tmp/cross-diff.txt
    exit 1
fi
echo "✅ Build-time and WASM-extracted schemas are identical"

echo ""
echo "=== All tests passed! ==="

