#!/usr/bin/env bash
# Build + bundle the migration fixtures the workflows in this directory install.
#
# Each pair shares one `--package` so both versions resolve to the same
# ApplicationId, the upgrade shape under test; the versioned output name keeps
# the two dist/ files from colliding.
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
PATH="$(scripts/setup-cargo-mero.sh):$PATH"

# The missing-ABI scenario needs one bundle with no embedded ABI. Its own
# package keeps the ApplicationId distinct from the embedded migration-suite-v2.
echo ">>> Bundling unembedded migration-suite-v2 (com.calimero.migration-suite-noabi @ 2.0.0)"
(cd apps/migrations/migration-suite-v2-add-field && cargo mero bundle --dev --no-abi --no-icon \
    --package com.calimero.migration-suite-noabi --app-version 2.0.0)

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
    echo ">>> Bundling $suite (com.calimero.${base} @ ${v_num}.0.0)"
    # cd, not --manifest-path: without it the output dir is the workspace root,
    # so bundles land in the top-level dist/ these scenarios install from.
    (cd "$suite" && cargo mero bundle --dev --no-icon \
        --package "com.calimero.${base}" \
        --app-version "${v_num}.0.0")
done

echo ">>> All migration-suite fixtures built and bundled under dist/."
