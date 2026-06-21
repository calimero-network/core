#!/usr/bin/env bash
# Cross-repo ABI contract check (core breaks first).
#
# Every ABI that core emits today must be accepted by the downstream
# @calimero-network/abi-codegen tool (the mero-devtools-js repo). This is the
# "true" integration test for the ABI contract: the in-crate guards in
# crates/wasm-abi only prove core's Rust enums match core's *own* bundled JSON
# schema — they cannot see the tool. If a schema change here makes the published
# tool unable to parse a current-core ABI, THIS job fails, at the source, instead
# of the breakage shipping silently and surfacing only when someone regenerates
# the tool's vendored snapshot.
#
# It is the mirror image of the devtools-side seam: there, CALIMERO_CORE_DIR
# points the tool's tests at a core checkout; here, DEVTOOLS_DIR points core's
# test at the tool checkout.
#
# Usage:
#   scripts/abi-contract-check.sh
#   DEVTOOLS_DIR=/path/to/mero-devtools-js scripts/abi-contract-check.sh
#   ABI_CONTRACT_APPS="kv-store abi_conformance" scripts/abi-contract-check.sh
#
# Env:
#   DEVTOOLS_DIR        mero-devtools-js checkout (default: <core>/../mero-devtools-js)
#   ABI_CONTRACT_APPS   space-separated package names to check (default: the
#                       representative set from scripts/build-all-apps.sh)

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

DEVTOOLS_DIR="${DEVTOOLS_DIR:-$ROOT/../mero-devtools-js}"
CODEGEN_DIR="$DEVTOOLS_DIR/abi-codegen"
CLI="$CODEGEN_DIR/dist/cli.js"

if [ ! -d "$CODEGEN_DIR" ]; then
    echo "ERROR: abi-codegen not found at $CODEGEN_DIR"
    echo "Clone https://github.com/calimero-network/mero-devtools-js and/or set DEVTOOLS_DIR."
    exit 1
fi

command -v jq >/dev/null || { echo "ERROR: jq is required"; exit 1; }
command -v node >/dev/null || { echo "ERROR: node is required"; exit 1; }

rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

echo "==> Building ABI extractor (mero-abi)"
cargo build --manifest-path tools/calimero-abi/Cargo.toml
EXTRACTOR="$ROOT/target/debug/mero-abi"

echo "==> Ensuring abi-codegen is built ($CODEGEN_DIR)"
if [ ! -f "$CLI" ]; then
    (
        cd "$DEVTOOLS_DIR"
        corepack enable >/dev/null 2>&1 || true
        pnpm install --filter '@calimero-network/abi-codegen...'
        pnpm --filter '@calimero-network/abi-codegen' build
    )
fi
[ -f "$CLI" ] || { echo "ERROR: abi-codegen CLI missing at $CLI after build"; exit 1; }

# Corpus: the maintained representative app set. We reuse the exact app list from
# build-all-apps.sh so a new app added there is automatically covered here, and
# we deliberately skip the ~40 near-duplicate migration *scenario* apps (they add
# no ABI-surface diversity, only CI time).
if [ -n "${ABI_CONTRACT_APPS:-}" ]; then
    # Caller supplied package names directly.
    APP_DIRS=""
    PKGS="$ABI_CONTRACT_APPS"
else
    APP_DIRS="$(grep -oE 'apps/[^"]*/build\.sh' scripts/build-all-apps.sh | sed 's#/build\.sh##' | sort -u)"
    PKGS=""
fi

META="$(cargo metadata --no-deps --format-version 1)"

# Resolve app directories to their package names (skip dirs with no wasm app crate).
# CRDT-diverse apps that build-all-apps.sh omits but that exercise the CRDT
# types (sorted_map / sorted_set / shared_storage) whose absence was the original
# downstream breakage. Included so the corpus touches all 11 CrdtCollectionType
# values, not just the ones in the representative set.
EXTRA_APPS="sorted-kv-store sorted-set-store kv-store-with-shared-storage"

if [ -z "$PKGS" ]; then
    for dir in $APP_DIRS; do
        pkg="$(echo "$META" | jq -r --arg d "/$dir/Cargo.toml" \
            '.packages[] | select(.manifest_path | endswith($d)) | .name' | head -1)"
        if [ -z "$pkg" ]; then
            echo "WARN: no package found for $dir, skipping"
            continue
        fi
        PKGS="$PKGS $pkg"
    done
    for pkg in $EXTRA_APPS; do
        case " $PKGS " in
            *" $pkg "*) ;;
            *) PKGS="$PKGS $pkg" ;;
        esac
    done
fi

OUT_DIR="$(mktemp -d)"
trap 'rm -rf "$OUT_DIR"' EXIT

pass=0
fail=0
skip=0
FAILED=()

for pkg in $PKGS; do
    echo "==> [$pkg] build debug wasm (ABI custom section intact; no wasm-opt)"
    # Capture the exact produced .wasm path from cargo's artifact messages rather
    # than guessing the filename from the package name.
    wasm="$(cargo build -p "$pkg" --target wasm32-unknown-unknown --message-format=json 2>/dev/null \
        | jq -r 'select(.reason=="compiler-artifact") | .filenames[]?' \
        | grep '\.wasm$' | head -1 || true)"
    if [ -z "$wasm" ] || [ ! -f "$wasm" ]; then
        echo "    no wasm artifact produced — skipping"
        skip=$((skip + 1))
        continue
    fi

    abi="$OUT_DIR/$pkg.json"
    if ! "$EXTRACTOR" extract "$wasm" -o "$abi" >/dev/null 2>&1; then
        echo "    no calimero_abi_v1 section — app emits no ABI, skipping"
        skip=$((skip + 1))
        continue
    fi

    if node "$CLI" --validate -i "$abi" >"$OUT_DIR/$pkg.log" 2>&1; then
        echo "    OK  ($(jq -r '"\(.methods|length) methods, \(.events|length) events, \(.types|length) types"' "$abi"))"
        pass=$((pass + 1))
    else
        echo "    REJECTED by abi-codegen:"
        sed 's/^/      /' "$OUT_DIR/$pkg.log"
        FAILED+=("$pkg")
        fail=$((fail + 1))
    fi
done

# Real code generation smoke test on the full-surface conformance ABI: parsing is
# necessary but not sufficient — make sure a non-empty client with every method
# is actually emitted.
CONF_ABI="$OUT_DIR/abi_conformance.json"
if [ -f "$CONF_ABI" ]; then
    echo "==> codegen smoke test (abi_conformance)"
    GEN_DIR="$OUT_DIR/gen"
    if node "$CLI" -i "$CONF_ABI" -o "$GEN_DIR" >"$OUT_DIR/gen.log" 2>&1; then
        client="$(find "$GEN_DIR" -name '*.ts' | head -1)"
        if [ -n "$client" ] && [ -s "$client" ]; then
            echo "    OK  (generated $(basename "$client"), $(wc -l <"$client") lines)"
        else
            echo "    FAILED: generated client is empty/missing"
            FAILED+=("abi_conformance(codegen)")
            fail=$((fail + 1))
        fi
    else
        echo "    FAILED: codegen errored"
        sed 's/^/      /' "$OUT_DIR/gen.log"
        FAILED+=("abi_conformance(codegen)")
        fail=$((fail + 1))
    fi
fi

echo "==================================================================="
echo "ABI contract: $pass accepted, $fail rejected, $skip skipped"
if [ "$fail" -gt 0 ]; then
    echo "FAILED: ${FAILED[*]}"
    echo
    echo "Core emits ABIs that @calimero-network/abi-codegen cannot parse. Either:"
    echo "  - this is unintended schema drift in core — fix it here, or"
    echo "  - it is an intended ABI change — land the matching update in"
    echo "    mero-devtools-js (schema + model) FIRST, then bump the pin."
    exit 1
fi
if [ "$pass" -eq 0 ]; then
    echo "ERROR: no ABIs were validated — corpus resolved to nothing (build setup bug)"
    exit 1
fi
echo "OK"
