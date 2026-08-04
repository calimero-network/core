#!/usr/bin/env bash
# Fail if an app wasm is too big for merod's module-size limit.
# Usage: check-wasm-size.sh [wasm ...]   (default: the apps the fuzzy suite installs)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

if [ "$#" -gt 0 ]; then
    wasms=("$@")
else
    wasms=(
        apps/kv-store/res/kv_store.wasm
        apps/kv-store-with-handlers/res/kv_store_with_handlers.wasm
        apps/scaffolding-e2e/res/scaffolding_e2e.wasm
    )
fi

# Read from the runtime's own default rather than restating it, so a bump there
# moves this gate with it.
LIMITS_RS="crates/runtime/src/logic.rs"
limit_mib="$(sed -n 's/^const DEFAULT_MAX_MODULE_SIZE_MIB: u64 = \([0-9]\{1,\}\);.*/\1/p' "$LIMITS_RS")"
if [ -z "$limit_mib" ]; then
    echo "ERROR: could not read DEFAULT_MAX_MODULE_SIZE_MIB from $LIMITS_RS" >&2
    exit 1
fi
limit=$((limit_mib * 1024 * 1024))

# Trip at 90% so growth surfaces here while there is still room to land a fix,
# not on the first byte past the cliff.
budget=$((limit * 9 / 10))

oversized=0
for wasm in "${wasms[@]}"; do
    if [ ! -f "$wasm" ]; then
        echo "ERROR: $wasm not found (build the apps first)" >&2
        exit 1
    fi
    size="$(wc -c <"$wasm" | tr -d '[:space:]')"
    pct=$((size * 100 / limit))
    if [ "$size" -le "$budget" ]; then
        printf 'ok:      %s (%s bytes, %s%% of the limit)\n' "$wasm" "$size" "$pct"
    else
        printf 'TOO BIG: %s (%s bytes, %s%% of the limit)\n' "$wasm" "$size" "$pct" >&2
        oversized=1
    fi
done

if [ "$oversized" -ne 0 ]; then
    echo "" >&2
    echo "FAIL: budget is $budget bytes, 90% of the runtime's ${limit}-byte" >&2
    echo "max_module_size (DEFAULT_MAX_MODULE_SIZE_MIB = $limit_mib in $LIMITS_RS)." >&2
    echo "merod refuses to create a context from a module past $limit bytes." >&2
    exit 1
fi

echo "All ${#wasms[@]} app wasm(s) within $budget bytes (90% of the ${limit}-byte max_module_size)."
