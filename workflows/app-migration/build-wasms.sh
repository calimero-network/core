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

echo ">>> All migration-suite fixtures built."
