#!/usr/bin/env bash
set -euo pipefail

# Add WASM target if not present
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

# Build the conformance app
echo "Building abi_conformance..."
cargo build -p abi_conformance --target wasm32-unknown-unknown

# Build the extractor if not present
EXTRACTOR="${ROOT:-$(git rev-parse --show-toplevel)}/target/debug/calimero-abi"
if [ ! -x "$EXTRACTOR" ]; then
    echo "Building calimero-abi extractor..."
    cargo build --manifest-path tools/calimero-abi/Cargo.toml
fi

# Extract ABI to temporary file
OUT="/tmp/abi_conformance.json"
echo "Extracting ABI..."
"$EXTRACTOR" extract target/wasm32-unknown-unknown/debug/abi_conformance.wasm -o "$OUT"

# Compare with golden file
echo "Comparing with golden file..."
if ! diff -u apps/abi_conformance/abi.expected.json "$OUT"; then
    echo "ERROR: ABI output differs from golden file"
    exit 1
fi

# Spot checks with jq
echo "Running jq spot checks..."

# Check nullable on opt methods
if ! jq -e '.methods[] | select(.name=="opt_u32").params[0].nullable == true' "$OUT" >/dev/null; then
    echo "ERROR: opt_u32 method parameter missing nullable=true"
    exit 1
fi
if ! jq -e '.methods[] | select(.name=="opt_u32").returns_nullable == true' "$OUT" >/dev/null; then
    echo "ERROR: opt_u32 method return missing returns_nullable=true"
    exit 1
fi

# Check events use payload (not type)
if ! jq -e '.events | all(.[]; (has("payload") or .payload==null))' "$OUT" >/dev/null; then
    echo "ERROR: Events should use 'payload' key, not 'type'"
    exit 1
fi

# Check bytes size rule (no size=0 for variable bytes)
if ! jq -e '.types | to_entries | all(.[]; (.value.kind!="bytes") or (.value.size == null or .value.size > 0))' "$OUT" >/dev/null; then
    echo "ERROR: Variable bytes should not have size=0"
    exit 1
fi

# Check map key form (string only)
if ! jq -e '.types | to_entries | all(.[]; (.value.kind!="map") or (.value.key=="string"))' "$OUT" >/dev/null; then
    echo "ERROR: Map keys must be 'string'"
    exit 1
fi

echo "ABI verify: OK" 