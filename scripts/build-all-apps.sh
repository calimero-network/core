#!/bin/bash

# Build every in-repo app through `cargo mero build`, then bundle every app
# that ships an installable, signed `.mpk` under dist/.
set -ex

APPS=(
    "apps/abi_conformance/Cargo.toml"
    "apps/abi_conformance_resolved/Cargo.toml"
    "apps/blobs/Cargo.toml"
    "apps/collaborative-editor/Cargo.toml"
    "apps/components-demo/Cargo.toml"
    "apps/kv-store-init/Cargo.toml"
    "apps/kv-store-with-handlers/Cargo.toml"
    "apps/kv-store-with-shared-storage/Cargo.toml"
    "apps/kv-store-with-user-and-frozen-storage/Cargo.toml"
    "apps/kv-store/Cargo.toml"
    "apps/migrations/migration-suite-v1/Cargo.toml"
    "apps/migrations/migration-suite-v2-add-field/Cargo.toml"
    "apps/migrations/migration-suite-v3-remove-field/Cargo.toml"
    "apps/migrations/migration-suite-v4-rename-field/Cargo.toml"
    "apps/migrations/migration-suite-v5-change-type/Cargo.toml"
    "apps/nested-crdt-test/Cargo.toml"
    "apps/private_data/Cargo.toml"
    "apps/scaffolding-e2e/Cargo.toml"
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

PATH="$(scripts/setup-cargo-mero.sh):$PATH"

for manifest in "${APPS[@]}"; do
    cargo mero build --manifest-path "$manifest"
done

for manifest in "${APPS[@]}" "${BUNDLE_ONLY[@]}"; do
    for skip in "${BUILD_ONLY[@]}"; do
        [ "$manifest" = "$skip" ] && continue 2
    done
    # cd (not --manifest-path) so the bundle lands under the shared top-level
    # dist/, matching the migration fixtures' convention.
    (cd "$(dirname "$manifest")" && cargo mero bundle --dev --no-icon)
done
