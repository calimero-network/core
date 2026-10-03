#!/bin/bash

# Build every in-repo app with cargo-mero: bundle every app that ships an
# installable, signed `.mpk` under dist/, build the rest, then check that every
# wasm embeds its ABI. CI caches the result (.github/actions/wasm-apps), so a
# check added here also guards every restored set.
#
#   --list-bundled   print the app dirs bundled into dist/, one per line, and exit
#                    (lint-e2e-bundle-coverage.py reads what dist/ holds from it)
set -e

APPS=(
    "apps/abi_conformance/Cargo.toml"
    "apps/abi_conformance_resolved/Cargo.toml"
    "apps/authored-sorted-kv-store/Cargo.toml"
    "apps/blobs/Cargo.toml"
    "apps/collaborative-editor/Cargo.toml"
    "apps/components-demo/Cargo.toml"
    "apps/fugue-collab/Cargo.toml"
    "apps/indexed-forum/Cargo.toml"
    "apps/permissions-showcase/Cargo.toml"
    "apps/indexed-issue-tracker/Cargo.toml"
    "apps/kv-store-init/Cargo.toml"
    "apps/kv-store-with-handlers/Cargo.toml"
    "apps/kv-store-with-shared-storage/Cargo.toml"
    "apps/kv-store-with-user-and-frozen-storage/Cargo.toml"
    "apps/tee-dice/Cargo.toml"
    "apps/tee-cards/Cargo.toml"
    "apps/name-registry/Cargo.toml"
    "apps/name-registry-admin/Cargo.toml"
    "apps/kv-store/Cargo.toml"
    "apps/migrations/migration-suite-v1/Cargo.toml"
    "apps/migrations/migration-suite-v2-add-field/Cargo.toml"
    "apps/migrations/migration-suite-v3-remove-field/Cargo.toml"
    "apps/migrations/migration-suite-v4-rename-field/Cargo.toml"
    "apps/migrations/migration-suite-v5-change-type/Cargo.toml"
    "apps/nested-crdt-test/Cargo.toml"
    "apps/private_data/Cargo.toml"
    "apps/rich-collab/Cargo.toml"
    "apps/scaffolding-e2e/Cargo.toml"
    "apps/search-chat/Cargo.toml"
    "apps/state-schema-conformance/Cargo.toml"
    "apps/team-metrics-custom/Cargo.toml"
    "apps/team-metrics-macro/Cargo.toml"
    "apps/xcall-example/Cargo.toml"
)

# migration-suite v1..v5 bundle via workflows/app-migration/build-wasms.sh's
# --app-version ladder; bundling them here too would collide on dist/.
BUILD_ONLY=(
    "apps/migrations/migration-suite-v1/Cargo.toml"
    "apps/migrations/migration-suite-v2-add-field/Cargo.toml"
    "apps/migrations/migration-suite-v3-remove-field/Cargo.toml"
    "apps/migrations/migration-suite-v4-rename-field/Cargo.toml"
    "apps/migrations/migration-suite-v5-change-type/Cargo.toml"
)

# scaffolding-e2e-multi declares services backed by scaffolding-e2e's wasm and
# builds none of its own, so only `bundle` has anything to do for it.
BUNDLE_ONLY=(
    "apps/scaffolding-e2e-multi/Cargo.toml"
)

bundled() {
    local manifest skip
    for manifest in "${APPS[@]}" "${BUNDLE_ONLY[@]}"; do
        for skip in "${BUILD_ONLY[@]}"; do
            [ "$manifest" = "$skip" ] && continue 2
        done
        dirname "$manifest"
    done
}

if [ "${1:-}" = --list-bundled ]; then
    bundled
    exit 0
fi
[ "$#" -eq 0 ] || { echo "usage: $0 [--list-bundled]" >&2; exit 2; }
set -x

PATH="$(scripts/setup-cargo-mero.sh):$PATH"

for manifest in "${BUILD_ONLY[@]}"; do
    cargo mero build --manifest-path "$manifest"
done

# `cargo mero bundle` builds every service it packages, so a bundled app is not
# also built on its own first. cd (not --manifest-path) so the bundle lands under
# the shared top-level dist/, matching the migration fixtures' convention.
# Word-split on purpose: app dirs hold no spaces, and bash 3.2 (macOS) has no mapfile.
for dir in $(bundled); do
    (cd "$dir" && cargo mero bundle --dev --no-icon)
done

./scripts/check-embedded-abi.sh
