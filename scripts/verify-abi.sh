#!/usr/bin/env bash
set -euo pipefail

# Add WASM target if not present
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

PATH="$(scripts/setup-cargo-mero.sh):$PATH"

# Build the extractor if not present
EXTRACTOR="${ROOT:-$(git rev-parse --show-toplevel)}/target/debug/mero-abi"
if [ ! -x "$EXTRACTOR" ]; then
    echo "Building calimero-abi extractor..."
    cargo build --manifest-path tools/calimero-abi/Cargo.toml
fi

# Build an app through cargo mero (which embeds the ABI section), read that
# section back off the wasm, and diff it against the golden committed beside it.
verify_golden() {
    local app="$1"
    echo "Building $app..."
    cargo mero build --manifest-path "apps/$app/Cargo.toml"
    echo "Extracting $app ABI and comparing with its golden file..."
    "$EXTRACTOR" extract "apps/$app/res/$app.wasm" -o "/tmp/$app.abi.json"
    if ! diff -u <(jq . "apps/$app/abi.expected.json") <(jq . "/tmp/$app.abi.json"); then
        echo "ERROR: $app ABI output differs from its golden file"
        exit 1
    fi
}

verify_golden abi_conformance
# Aliases, macro-generated and re-exported types: only the compiler resolves
# these, so this golden is what proves they are described correctly.
verify_golden abi_conformance_resolved

WASM="apps/abi_conformance/res/abi_conformance.wasm"
OUT="/tmp/abi_conformance.abi.json"

# Spot checks with jq
echo "Running jq spot checks..."

# A #[app::view] method must surface intent=read_only, and a #[app::xcall]
# method must surface xcall_callable=true, in the emitted ABI.
if ! jq -e '.methods[] | select(.name=="view_constant").intent == "read_only"' "$OUT" >/dev/null; then
    echo "ERROR: view_constant (#[app::view]) missing intent=read_only"
    exit 1
fi
if ! jq -e '.methods[] | select(.name=="xcall_noop").xcall_callable == true' "$OUT" >/dev/null; then
    echo "ERROR: xcall_noop (#[app::xcall]) missing xcall_callable=true"
    exit 1
fi

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

# Exercise the identity-downgrade lint (the gate's L2 implementation) so a build
# break or panic in the diff path fails here too. A state schema diffed against
# itself must report NO unsafe downgrade (exit 0). The positive case — a real
# AuthoredMap->UnorderedMap downgrade IS caught — is gated on a real built
# pair in .github/workflows/app-migration-e2e.yml (schema-downgrade-guard).
echo "Exercising identity-downgrade lint (self-diff must be clean)..."
STATE="/tmp/abi_conformance.state.json"
"$EXTRACTOR" state "$WASM" -o "$STATE"
"$EXTRACTOR" diff "$STATE" "$STATE"

echo "ABI verify: OK" 