#!/usr/bin/env bash
# Convenience wrapper that builds every migration-suite WASM fixture
# used by the workflows in this directory. Each suite has its own
# build.sh; this script just invokes them in order so CI and local
# devs have one entry-point.
#
# Add new suites here as later PRs introduce them.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

SUITES=(
    # Original v1..v5 chain (each vN migrates from vN-1).
    "apps/migrations/migration-suite-v1"
    "apps/migrations/migration-suite-v2-add-field"
    "apps/migrations/migration-suite-v3-remove-field"
    "apps/migrations/migration-suite-v4-rename-field"
    "apps/migrations/migration-suite-v5-change-type"
    # Per-scenario v1+v2 pairs (each pair is self-contained, no chain).
    "apps/migrations/scenario-new-method-v1"
    "apps/migrations/scenario-new-method-v2"
    "apps/migrations/scenario-new-enum-variant-v1"
    "apps/migrations/scenario-new-enum-variant-v2"
    "apps/migrations/scenario-pure-bugfix-v1"
    "apps/migrations/scenario-pure-bugfix-v2"
    "apps/migrations/scenario-crdt-native-v1"
    "apps/migrations/scenario-crdt-native-v2"
    "apps/migrations/scenario-struct-to-enum-v1"
    "apps/migrations/scenario-struct-to-enum-v2"
    "apps/migrations/scenario-field-split-v1"
    "apps/migrations/scenario-field-split-v2"
    "apps/migrations/scenario-field-remove-archive-v1"
    "apps/migrations/scenario-field-remove-archive-v2"
    "apps/migrations/scenario-invariant-reshuffle-v1"
    "apps/migrations/scenario-invariant-reshuffle-v2"
    "apps/migrations/scenario-authored-map-v1"
    "apps/migrations/scenario-authored-map-v2"
    "apps/migrations/scenario-authored-migrate-ux-v1"
    "apps/migrations/scenario-authored-migrate-ux-v2"
    "apps/migrations/scenario-user-storage-v1"
    "apps/migrations/scenario-user-storage-v2"
    "apps/migrations/scenario-frozen-storage-v1"
    "apps/migrations/scenario-frozen-storage-v2"
    "apps/migrations/scenario-shared-storage-v1"
    "apps/migrations/scenario-shared-storage-v2"
    "apps/migrations/scenario-authored-vector-v1"
    "apps/migrations/scenario-authored-vector-v2"
    "apps/migrations/scenario-unordered-set-v1"
    "apps/migrations/scenario-unordered-set-v2"
    "apps/migrations/scenario-identity-downgrade-v1"
    "apps/migrations/scenario-identity-downgrade-v2"
    # migration_check pass/fail pairs (PR-6d task 6d.6): the v2 fixtures export
    # #[app::migration_check] — the PASS pair carries items faithfully (check
    # accepts), the FAIL pair drops one item (check rejects → logical abort).
    "apps/migrations/scenario-migration-check-pass-v1"
    "apps/migrations/scenario-migration-check-pass-v2"
    "apps/migrations/scenario-migration-check-fail-v1"
    "apps/migrations/scenario-migration-check-fail-v2"
)

for suite in "${SUITES[@]}"; do
    if [ ! -d "$suite" ]; then
        echo "ERROR: $suite not found" >&2
        exit 1
    fi
    if [ ! -x "$suite/build.sh" ]; then
        echo "ERROR: $suite/build.sh missing or not executable" >&2
        exit 1
    fi
    echo ">>> Building $suite"
    bash "$suite/build.sh"
done

# Embed each fixture's state schema as the wasm's `calimero_abi_v1` section
# (AFTER build.sh so wasm-opt cannot strip it). The node reads this embedded
# form for the upgrade decision table (state_version + migration edges) and
# the identity-downgrade gate — without it both are fail-open/unresolvable.
echo ">>> Building mero-abi (embed tool)"
cargo build -p mero-abi --release
ABI_TOOL="${CARGO_TARGET_DIR:-target}/release/mero-abi"
for suite in "${SUITES[@]}"; do
    dir_name="$(basename "$suite")"
    wasm_file="${dir_name//-/_}.wasm"
    "$ABI_TOOL" embed "$suite/res/$wasm_file" "$suite/res/state-schema.json"
done

# Wrap every fixture into a signed single-service `.mpk`. v1/v2 of a pair
# share the package name, so they install under the SAME ApplicationId
# (hash(package, signer)) — the realistic upgrade shape, where only the
# bytecode blob changes between versions. The workflows install these
# bundles (not the raw wasms) so the same-id propagation path is exercised.
for suite in "${SUITES[@]}"; do
    dir_name="$(basename "$suite")"
    wasm_file="${dir_name//-/_}.wasm"
    # `migration-suite-v3-remove-field` → base `migration-suite`, version 3.
    # `scenario-user-storage-v2`       → base `scenario-user-storage`, version 2.
    base="${dir_name%%-v[0-9]*}"
    v_num="$(printf '%s' "$dir_name" | sed -E 's/.*-v([0-9]+).*/\1/')"
    echo ">>> Bundling $suite (com.calimero.${base} @ ${v_num}.0.0)"
    bash apps/migrations/bundle-wasm.sh \
        "$suite" "$wasm_file" "com.calimero.${base}" "${v_num}.0.0"
done

echo ">>> All migration-suite fixtures built."
