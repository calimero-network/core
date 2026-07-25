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
    "apps/private_data/Cargo.toml"
    "apps/state-schema-conformance/Cargo.toml"
    "apps/team-metrics-custom/Cargo.toml"
    "apps/team-metrics-macro/Cargo.toml"
    "apps/xcall-example/Cargo.toml"
)

# Build the toolchain once: `cargo run` per app re-checks the whole workspace on
# every iteration.
cargo build -q -p cargo-mero
CARGO_MERO="${CARGO_TARGET_DIR:-target}/debug/cargo-mero"

for manifest in "${APPS[@]}"; do
    "$CARGO_MERO" mero build --manifest-path "$manifest"
done

# Apps that also ship an installable, signed .mpk bundle.
"$CARGO_MERO" mero bundle --dev --manifest-path apps/kv-store/Cargo.toml

# nested-crdt-test returns a tuple, which the ABI emitter cannot express, so it
# cannot go through `cargo mero build`. Compile it here anyway to keep it from
# rotting; the artifact stays in target/ and never reaches a res/ directory, so
# the embedded-ABI guard's "everything in res/ carries an ABI" rule still holds.
cargo build -q -p nested-crdt-test --target wasm32-unknown-unknown --profile app-release
