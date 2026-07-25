#!/bin/bash

# Build every in-repo app through `cargo mero build` (compile -> wasm-opt ->
# embed the full ABI as the wasm `calimero_abi_v1` section), then bundle the
# ones that ship an installable `.mpk`. Replaces the per-app build.sh /
# build-bundle.sh scripts.
set -ex

# Apps built to a wasm artifact (res/<name>.wasm + embedded ABI).
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
    # nested-crdt-test is intentionally omitted: its public API returns a tuple,
    # which the ABI emitter cannot express yet, so it cannot embed an ABI. Build
    # it directly with `cargo build --target wasm32-unknown-unknown -p nested-crdt-test`.
    "apps/private_data/Cargo.toml"
    "apps/state-schema-conformance/Cargo.toml"
    "apps/team-metrics-custom/Cargo.toml"
    "apps/team-metrics-macro/Cargo.toml"
    "apps/xcall-example/Cargo.toml"
)

for manifest in "${APPS[@]}"; do
    cargo run -q -p cargo-mero -- mero build --manifest-path "$manifest"
done

# Apps that also ship an installable, signed .mpk bundle.
cargo run -q -p cargo-mero -- mero bundle --dev --manifest-path apps/kv-store/Cargo.toml
