#!/usr/bin/env bash
# Build + bundle the migration fixtures the workflows in this directory install.
#
# Each pair shares one `--package` so both versions resolve to the same
# ApplicationId, which is the upgrade shape under test. That also means the
# default dist/<package>.mpk path would collide, so each suite passes its own
# `-o`.
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
    # #[app::migration_check] - the PASS pair carries items faithfully (check
    # accepts), the FAIL pair drops one item (check rejects → logical abort).
    "apps/migrations/scenario-migration-check-pass-v1"
    "apps/migrations/scenario-migration-check-pass-v2"
    "apps/migrations/scenario-migration-check-fail-v1"
    "apps/migrations/scenario-migration-check-fail-v2"
)

mkdir -p dist

# Unembedded v2 variant for the missing-ABI refusal scenario
# (39-missing-abi-refused). `cargo mero bundle` always embeds the ABI (see
# scripts/check-embedded-abi.sh), so this one wasm is compiled by hand and
# bundled via bundle-wasm.sh BEFORE migration-suite-v2-add-field is built
# below - the loop below overwrites this same res/ wasm in place with an
# embedded copy. An upgrade whose target has no embedded ABI is refused (no
# migration evidence) unless forceCodeOnly is passed. Distinct package ->
# distinct ApplicationId, so it never collides with the embedded
# migration-suite bundles built later.
NOABI_SUITE="apps/migrations/migration-suite-v2-add-field"
NOABI_WASM="migration_suite_v2_add_field.wasm"
NOABI_TARGET="${CARGO_TARGET_DIR:-target}"
echo ">>> Building unembedded migration-suite-v2 (missing-ABI scenario fixture)"
cargo build --target wasm32-unknown-unknown --profile app-release -p migration-suite-v2-add-field
mkdir -p "$NOABI_SUITE/res"
cp "$NOABI_TARGET/wasm32-unknown-unknown/app-release/$NOABI_WASM" "$NOABI_SUITE/res/$NOABI_WASM"
if command -v wasm-opt > /dev/null; then
    wasm-opt -Oz --enable-bulk-memory "$NOABI_SUITE/res/$NOABI_WASM" -o "$NOABI_SUITE/res/$NOABI_WASM"
fi
echo ">>> Bundling unembedded migration-suite-v2 (com.calimero.migration-suite-noabi @ 2.0.0)"
bash apps/migrations/bundle-wasm.sh \
    "$NOABI_SUITE" "$NOABI_WASM" "com.calimero.migration-suite-noabi" "2.0.0"

for suite in "${SUITES[@]}"; do
    if [ ! -d "$suite" ]; then
        echo "ERROR: $suite not found" >&2
        exit 1
    fi
    dir_name="$(basename "$suite")"
    # `migration-suite-v3-remove-field` -> base `migration-suite`, version 3.
    # `scenario-user-storage-v2`       -> base `scenario-user-storage`, version 2.
    base="${dir_name%%-v[0-9]*}"
    v_num="$(printf '%s' "$dir_name" | sed -E 's/.*-v([0-9]+).*/\1/')"
    echo ">>> Bundling $suite (com.calimero.${base} @ ${v_num}.0.0) -> dist/${dir_name}.mpk"
    cargo run -q -p cargo-mero -- mero bundle --dev \
        --manifest-path "$suite/Cargo.toml" \
        --package "com.calimero.${base}" \
        --app-version "${v_num}.0.0" \
        -o "dist/${dir_name}.mpk"
done

echo ">>> All migration-suite fixtures built and bundled under dist/."
