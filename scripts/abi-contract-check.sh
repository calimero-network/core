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

# The corpus builds through `cargo mero`, which is what emits the ABI at all.
echo "==> Building the app toolchain (cargo-mero)"
PATH="$(scripts/setup-cargo-mero.sh):$PATH"

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
# Use arrays throughout so package names / paths are never word-split.
PKGS=()
APP_DIRS=()
if [ -n "${ABI_CONTRACT_APPS:-}" ]; then
    # Caller supplied package names directly (space-separated).
    read -ra PKGS <<<"$ABI_CONTRACT_APPS"
else
    # `while read` (not mapfile) for bash 3.2 portability (macOS default shell).
    while IFS= read -r dir; do
        APP_DIRS+=("$dir")
    done < <(
        grep -oE 'apps/[^" ]*/Cargo\.toml' scripts/build-all-apps.sh \
            | sed 's#/Cargo\.toml##' | sort -u
    )
fi

META="$(cargo metadata --no-deps --format-version 1)"

# Resolve app directories to their package names (skip dirs with no wasm app crate).
# CRDT-diverse apps that build-all-apps.sh omits but that exercise the CRDT
# types (sorted_map / sorted_set / shared_storage) whose absence was the original
# downstream breakage. Included so the corpus touches all 11 CrdtCollectionType
# values, not just the ones in the representative set.
EXTRA_APPS=(sorted-kv-store sorted-set-store kv-store-with-shared-storage)

if [ "${#PKGS[@]}" -eq 0 ]; then
    for dir in "${APP_DIRS[@]}"; do
        pkg="$(echo "$META" | jq -r --arg d "/$dir/Cargo.toml" \
            '.packages[] | select(.manifest_path | endswith($d)) | .name' | head -1)"
        if [ -z "$pkg" ]; then
            echo "WARN: no package found for $dir, skipping"
            continue
        fi
        PKGS+=("$pkg")
    done
    for extra in "${EXTRA_APPS[@]}"; do
        already=0
        for pkg in "${PKGS[@]}"; do
            [ "$pkg" = "$extra" ] && already=1 && break
        done
        [ "$already" -eq 0 ] && PKGS+=("$extra")
    done
fi

OUT_DIR="$(mktemp -d)"
# Extracted ABIs live in their own subdir so they never collide with, or get
# globbed alongside, the per-package cargo/build/extract logs in $OUT_DIR.
ABI_DIR="$OUT_DIR/abis"
mkdir -p "$ABI_DIR"
trap 'rm -rf "$OUT_DIR"' EXIT

pass=0
fail=0
skip=0
FAILED=()

for pkg in "${PKGS[@]}"; do
    echo "==> [$pkg] build wasm through cargo mero (--profiling: no wasm-opt)"
    # A build failure is a hard error, never a skip: a broken app must not drop
    # out of the coverage gate while the job still passes. --profiling only skips
    # wasm-opt to save CI time; the embed runs after it either way.
    build_log="$OUT_DIR/$pkg.build.log"
    if ! cargo mero build -p "$pkg" --profiling >"$build_log" 2>&1; then
        echo "    BUILD FAILED:"
        sed 's/^/      /' "$build_log"
        FAILED+=("$pkg(build)")
        fail=$((fail + 1))
        continue
    fi
    # cargo mero writes res/<underscored>.wasm beside the crate manifest.
    crate_dir="$(echo "$META" | jq -r --arg p "$pkg" \
        '.packages[] | select(.name==$p) | .manifest_path' | head -1)"
    crate_dir="$(dirname "$crate_dir")"
    wasm="$crate_dir/res/$(echo "$pkg" | tr '-' '_').wasm"
    # A missing wasm after a successful build means the tool and this script
    # disagree on the name - a fault, not an app without a cdylib.
    if [ ! -f "$wasm" ]; then
        echo "    BUILD OK BUT NO WASM AT $wasm"
        FAILED+=("$pkg(wasm-path)")
        fail=$((fail + 1))
        continue
    fi

    # The extractor resolves the abi.json beside the wasm. Only "No abi.json file
    # found" is a legitimate skip; any other error is a fault and must fail.
    abi="$ABI_DIR/$pkg.json"
    extract_log="$OUT_DIR/$pkg.extract.log"
    if ! "$EXTRACTOR" extract "$wasm" -o "$abi" >"$extract_log" 2>&1; then
        if grep -q "No abi.json file found" "$extract_log"; then
            echo "    no abi.json emitted — app has no ABI, skipping"
            skip=$((skip + 1))
            continue
        fi
        echo "    EXTRACTION FAILED:"
        sed 's/^/      /' "$extract_log"
        FAILED+=("$pkg(extract)")
        fail=$((fail + 1))
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
CONF_ABI="$ABI_DIR/abi_conformance.json"
if [ -f "$CONF_ABI" ]; then
    echo "==> codegen smoke test (abi_conformance)"
    GEN_DIR="$OUT_DIR/gen"
    if node "$CLI" -i "$CONF_ABI" -o "$GEN_DIR" >"$OUT_DIR/gen.log" 2>&1; then
        gen_ts=()
        while IFS= read -r ts; do gen_ts+=("$ts"); done < <(find "$GEN_DIR" -name '*.ts')
        empty=0
        for ts in "${gen_ts[@]}"; do [ -s "$ts" ] || empty=1; done
        if [ "${#gen_ts[@]}" -gt 0 ] && [ "$empty" -eq 0 ]; then
            echo "    OK  (generated ${#gen_ts[@]} non-empty file(s))"
        else
            echo "    FAILED: codegen produced no client, or an empty file"
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

# Coverage gate (strict — no allowlist). Per-app validation only sees the CRDT
# types some app happens to emit; a tool regression on a type that NO app
# exercises would slip through. So assert that EVERY CrdtType the core schema
# declares is exercised by at least one corpus ABI. The expected set is read from
# core's own wasm-abi.schema.json — the same file the in-crate tests pin to the
# Rust enum, so this needs no second hand-maintained list.
#
# There is deliberately no escape hatch: adding a CrdtType to core obliges you to
# add (or extend) an app that emits it — apps/abi_conformance is the canonical
# place, it locks every collection marker.
cov_fail=0
SCHEMA="crates/wasm-abi/wasm-abi.schema.json"
shopt -s nullglob
abi_files=("$ABI_DIR"/*.json)
shopt -u nullglob
if [ "${#abi_files[@]}" -gt 0 ] && [ -f "$SCHEMA" ]; then
    echo "==> CRDT coverage gate (every schema-declared type must be exercised)"
    exp_f="$OUT_DIR/_expected"
    cov_f="$OUT_DIR/_covered"
    jq -r '.definitions.CrdtType.enum[]' "$SCHEMA" | sort -u >"$exp_f"
    jq -s -r '[.[] | .. | objects | .crdt_type? // empty] | unique[]' "${abi_files[@]}" | sort -u >"$cov_f"

    # Declared but exercised by no app.
    missing="$(comm -23 "$exp_f" "$cov_f" || true)"
    # Emitted by an app but absent from the schema enum (drift the schema missed).
    extra="$(comm -13 "$exp_f" "$cov_f" || true)"

    echo "    declared:  $(tr '\n' ' ' <"$exp_f")"
    echo "    exercised: $(tr '\n' ' ' <"$cov_f")"
    if [ -n "$missing" ]; then
        echo "    COVERAGE GAP — declared CrdtType(s) no corpus app exercises:"
        echo "$missing" | sed 's/^/      - /'
        echo "      Add an app (or a field in apps/abi_conformance) that emits it."
        cov_fail=1
    fi
    if [ -n "$extra" ]; then
        echo "    DRIFT — corpus emitted CrdtType(s) the core schema does not declare:"
        echo "$extra" | sed 's/^/      - /'
        cov_fail=1
    fi
    [ "$cov_fail" -eq 0 ] && echo "    OK  (all $(wc -l <"$exp_f" | tr -d ' ') declared CrdtTypes exercised)"
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
if [ "$cov_fail" -ne 0 ]; then
    echo "FAILED: CRDT coverage gate (see above)."
    echo "  Every CrdtType the schema declares must be exercised by a corpus app."
    echo "  Add the type to apps/abi_conformance (or another corpus app) so its ABI"
    echo "  emits it. There is no allowlist."
    exit 1
fi
if [ "$pass" -eq 0 ]; then
    echo "ERROR: no ABIs were validated — corpus resolved to nothing (build setup bug)"
    exit 1
fi
echo "OK"
