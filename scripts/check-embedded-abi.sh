#!/usr/bin/env bash
# Fail if any app wasm is missing the `calimero_abi_v1` custom section - the
# embedded full ABI the node reads for the xcall gate and the migration /
# identity-downgrade decisions. `cargo mero build` embeds it on every build;
# this guards against a regression that ships a wasm the node cannot introspect.
#
# Usage: check-embedded-abi.sh [wasm ...]
# With no args, checks every apps/**/res/*.wasm produced by a build.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

if [ "$#" -gt 0 ]; then
    wasms=("$@")
else
    # `find` (not a **/ glob) so this works on bash 3.2 (macOS) too; an unbuilt
    # tree yields an empty list and fails the "none found" check below.
    wasms=()
    while IFS= read -r w; do
        wasms+=("$w")
    done < <(find apps -type f -path '*/res/*.wasm' | sort)
fi

if [ "${#wasms[@]}" -eq 0 ]; then
    echo "ERROR: no wasm files to check (build the apps first)" >&2
    exit 1
fi

# Build the extractor once: `cargo run` per wasm re-checks the workspace each time.
cargo build -q -p mero-abi
MERO_ABI="${CARGO_TARGET_DIR:-target}/debug/mero-abi"

stderr_file="$(mktemp)"
trap 'rm -f "$stderr_file"' EXIT

missing=0
for wasm in "${wasms[@]}"; do
    # Capture the full inspect output before grepping. Piping straight into
    # `grep -q` lets grep close the pipe on the first match, killing `cargo run`
    # with SIGPIPE, which `set -o pipefail` then reports as a (false) failure.
    # A failing inspect means the guard could not run at all, so report it as a
    # tool error rather than silently calling the section missing.
    if ! report="$("$MERO_ABI" inspect "$wasm" 2>"$stderr_file")"; then
        echo "ERROR: could not run mero-abi inspect on $wasm" >&2
        sed 's/^/  /' "$stderr_file" >&2
        exit 1
    fi
    # Match the section-walk line (`CustomSection: 'calimero_abi_v1' (N bytes)`),
    # never a bare name: inspect's "section NOT found" advice also contains the
    # name, so a looser grep passes every wasm and the guard never fires.
    if printf '%s' "$report" | grep -q "CustomSection: 'calimero_abi_v1'"; then
        echo "ok:      $wasm"
    else
        echo "MISSING: $wasm has no calimero_abi_v1 section" >&2
        missing=1
    fi
done

if [ "$missing" -ne 0 ]; then
    echo "" >&2
    echo "FAIL: one or more app wasms lack the embedded ABI." >&2
    exit 1
fi

echo "All ${#wasms[@]} app wasm(s) carry the calimero_abi_v1 section."
