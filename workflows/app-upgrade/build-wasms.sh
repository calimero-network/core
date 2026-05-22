#!/bin/bash
# Build all migration-suite WASM fixtures consumed by the app-upgrade workflows.
#
# Convenience wrapper around each apps/migrations/migration-suite-*/build.sh.
# Each fixture writes its .wasm into its own apps/migrations/.../res/ — the
# workflows reference those paths directly, matching the pattern used by
# workflows/sync-tests/ (which builds apps/kv-store-with-handlers/ directly).
#
# Usage:
#   bash workflows/app-upgrade/build-wasms.sh
#
# CI runs the equivalent inline; this is for local dev iteration.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"

for build in apps/migrations/migration-suite-*/build.sh; do
    suite_dir="$(dirname "$build")"
    suite_name="$(basename "$suite_dir")"
    echo "==> Building ${suite_name}"
    bash "$build"
done

echo
echo "Built fixtures:"
for wasm in apps/migrations/migration-suite-*/res/*.wasm; do
    printf "  %-72s %s\n" "$wasm" "$(ls -lh "$wasm" | awk '{print $5}')"
done
