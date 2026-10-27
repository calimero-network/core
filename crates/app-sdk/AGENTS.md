# calimero-app-sdk - the Rust app SDK runtime

## Package Identity

- **Crate**: `calimero-app-sdk` (`publish = false`, prototype); builds for the host (tests) and `wasm32-wasip2` (apps)
- **Entry**: `src/lib.rs`; `encoding/{key,value}.rs`, `store.rs` (+ `store/{mem,host}.rs`), `table.rs`, `collections/{map,counter}.rs`, `env.rs` (+ `env/{wasm,mock}.rs`), `guards.rs`, `error.rs`, `bindings.rs` (wasm32 only)
- **Spec**: `docs/specs/app-runtime-v0.14.md` section 7, the SDK reference, D9, D10, D20, D47, D48, D50, D52

## What This Crate Does

The shared storage encoding (order-preserving tuple keys, deterministic CBOR values), the `Store` trait over the host's ordered key-value storage, tables with indexes, unique constraints and counts, `UnorderedMap`, `Counter`, `env` and the member/admin guards. The macros (`#[app::logic]`, ...) and the test harness are separate tasks.

## Rules

- Key layouts are the cross-language contract (the TypeScript SDK must produce the same bytes): change them only with the spec.
- Table counts and `Counter::add` use `Store::add`, never read-modify-write (D47).
- Keys over 1 KiB are refused with `StorageError::KeyTooLong` before any store call (D50).
- Queries read `limit + 1` entries through the cursor and no more.
- `bindings.rs` skips `handle`: the export belongs to the macro-generated code.
- `env::mock` (host target) is the stand-in for the host in tests.

## Verify

`cargo test -p calimero-app-sdk`, `cargo build -p calimero-app-sdk --target wasm32-wasip2`, and `cargo clippy -p calimero-app-sdk --all-targets -- -D warnings`
