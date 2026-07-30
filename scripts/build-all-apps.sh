#!/bin/bash

# Build every in-repo app through `cargo mero build`, then bundle the ones that
# ship an installable `.mpk`.
set -ex

APPS=(
    "apps/abi_conformance/Cargo.toml"
    "apps/blobs/Cargo.toml"
    "apps/collaborative-editor/Cargo.toml"
    "apps/kv-store-init/Cargo.toml"
    "apps/kv-store-with-handlers/Cargo.toml"
    "apps/kv-store-with-user-and-frozen-storage/Cargo.toml"
    "apps/kv-store/Cargo.toml"
    "apps/migrations/migration-suite-v1/Cargo.toml"
    "apps/migrations/migration-suite-v2-add-field/Cargo.toml"
    "apps/migrations/migration-suite-v3-remove-field/Cargo.toml"
    "apps/migrations/migration-suite-v4-rename-field/Cargo.toml"
    "apps/migrations/migration-suite-v5-change-type/Cargo.toml"
    "apps/nested-crdt-test/Cargo.toml"
    "apps/private_data/Cargo.toml"
    "apps/state-schema-conformance/Cargo.toml"
    "apps/team-metrics-custom/Cargo.toml"
    "apps/team-metrics-macro/Cargo.toml"
    "apps/xcall-example/Cargo.toml"
)

PATH="$(scripts/setup-cargo-mero.sh):$PATH"

for manifest in "${APPS[@]}"; do
    cargo mero build --manifest-path "$manifest"
done

# Apps that also ship an installable, signed .mpk bundle.
cargo mero bundle --dev --manifest-path apps/kv-store/Cargo.toml
