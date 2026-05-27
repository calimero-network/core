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
    "apps/migrations/migration-suite-v1"
    "apps/migrations/migration-suite-v2-add-field"
    "apps/migrations/migration-suite-v3-remove-field"
    "apps/migrations/migration-suite-v4-rename-field"
    "apps/migrations/migration-suite-v5-change-type"
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
