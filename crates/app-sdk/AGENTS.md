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

## Encoding contract (the TypeScript SDK must match)

- **Keys**: tag byte per part (none 0x00, bool 0x10, int 0x20, string 0x30, bytes 0x40); ints are sign-flipped big-endian 16 bytes; strings and bytes escape `0x00` as `0x00 0xFF` and end with `0x00 0x00`.
- **Key length (D50, ruling 24)**: the host refuses keys over 1,024 bytes. The SDK caps every key it stores (record, index, unique, count, map, counter) at 1,023 (`MAX_SDK_KEY_BYTES`), so derived bounds (`last ++ 0x00`, `prefix ++ 0xFF`) always fit. Every range bound and decoded cursor is checked before any store call: bounds give `KeyTooLong`, a cursor gives `BadCursor`.
- **Unique constraints use SQL NULL semantics (ruling 23)**: when any field of a unique constraint is `None`, no unique entry is written or checked (insert, upsert, update, remove). Two records with no email both insert; the same `Some(email)` still clashes.
- **Values**: deterministic CBOR (RFC 8949 core). Field names are camelCase (applied by the `#[app::state]`/record macro, not by the SDK). `None` is written as CBOR null, never omitted. An integral float stays a float (`1.0` is `f93c00`), floats use the shortest exact form, `-0.0` is preserved, NaN is refused, map keys sort by encoded bytes, lengths are definite.
- **JSON (D20, ruling 25)**: `u64`/`i64` in event data and `AppError` fields are decimal strings; use `calimero_app_sdk::json::{u64_str, i64_str, opt_u64_str, opt_i64_str}` (the macros apply them).
- **Cursors** are the base64url of the last storage key, bound to the index and prefix, not signed.
- **Prefixes** longer than the index or count fields are refused (`PrefixTooLong`); `limit: 0` is an empty page.

## Verify

`cargo test -p calimero-app-sdk`, `cargo build -p calimero-app-sdk --target wasm32-wasip2`, and `cargo clippy -p calimero-app-sdk --all-targets -- -D warnings`
