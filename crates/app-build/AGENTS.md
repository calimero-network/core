# calimero-app-build - Rust guest to `calimero:app` component

## Package Identity

- **Crate**: `calimero-app-build` (`publish = false`, prototype); lib plus bin `mero-app-build`
- **Entry**: `src/lib.rs`, `src/main.rs`
- **Spec**: `docs/specs/app-runtime-v0.14.md` (D45 build path)

## What This Crate Does

`build_component(crate_dir, target_dir)` runs `cargo build --release --target wasm32-wasip2`, lifts the `calimero:abi` custom section from the nested core module to the component's top level, and returns the component, its ABI JSON and its top-level imports. `abi_of` and `imports_of` read a component's top level only.

## Rules

- Section lifting is raw byte surgery on section boundaries (no re-encode of contents); keep it that way.
- Table `partition` refusal is not here; it is a compile error in the `#[table]` derive.
- `tests/guests/echo` is a standalone crate (own `[workspace]`), not a member of the core workspace.

## Verify

`cargo test -p calimero-app-build`
