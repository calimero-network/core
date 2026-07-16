# calimero-storage-macros - Derive Macros for calimero-storage Entities

Proc-macro crate that generates the `Data`/`AtomicUnit`/`Collection` trait boilerplate consumed by `calimero-storage`.

## Package Identity

- **Crate**: `calimero-storage-macros`
- **Entry**: `src/lib.rs` (the whole crate - one file, no modules)
- **Key deps**: `syn` 2.0 (features `full`, `extra-traits` - parses the derive input), `quote` 1.0 (token-stream codegen). `proc-macro2` is not a direct dependency; `proc_macro::TokenStream` (std) is converted to/from it only implicitly via `quote!`/`parse_macro_input!`
- **`[lib]`**: `proc-macro = true`

## Commands

```bash
# Build
cargo build -p calimero-storage-macros

# Test (no #[test]s live in this crate itself - see Gotchas)
cargo test -p calimero-storage-macros

# Exercise the macros via their real caller
cargo test -p calimero-storage
```

## Macro Inventory

| Macro | Kind | Attributes accepted | Generates |
| --- | --- | --- | --- |
| `AtomicUnit` | `#[proc_macro_derive]` | `#[storage]` (required, on exactly one field), `#[collection]` (0+ fields) | `impl calimero_storage::entities::Data` (`collections()`, `element()`, `element_mut()`) + `impl calimero_storage::entities::AtomicUnit` |
| `Collection` | `#[proc_macro_derive]` | `#[children(Type)]` (required, struct-level) | `impl calimero_storage::entities::Collection<Child = Type>`, `impl Default`, `impl BorshSerialize`/`BorshDeserialize` (both no-op - zero bytes in, zero bytes out) |

There is no `#[derive(Mergeable)]` here - see Gotchas.

## Mental Model

`AtomicUnit` marks a struct as a persistable entity: it must own exactly one `#[storage]`-tagged `Element` field (storage metadata: ID, path, timestamps, hashes) and may tag other fields `#[collection]` to declare them as groups of child entities. The derive walks the named fields, finds the `#[storage]` one and any `#[collection]` ones, then emits `Data::collections()` (a `BTreeMap` built by calling `MainInterface::child_info_for` for each collection field), `Data::element()`/`element_mut()` (borrows of the storage field), and the empty `AtomicUnit` marker impl. Generic type params get an auto-added `BorshSerialize + BorshDeserialize` where-bound (referenced as `calimero_sdk::borsh::...`) so the impls compile for generic entity types.

`Collection` marks a struct as a pure grouping handle for a specific child `Data` type named in `#[children(Type)]`. Its entries live as separate child elements in storage, never inline in the parent - so the derive gives it a fully no-op Borsh round-trip (`serialize` writes nothing, `deserialize_reader` returns `Default::default()`), keeping the parent struct's borsh byte stream in lock-step even though this field contributes zero bytes.

Both derives use `syn::Error::new_spanned(...).to_compile_error()` for validation failures, so misuse (enum/union target, missing attribute, unnamed fields) surfaces as a normal rustc compile error pointing at the offending item, not a panic.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `atomic_unit_derive` (~95 lines), `collection_derive` (~100 lines), and a trailing comment explaining the absent `Mergeable` derive |

## Invariants and Gotchas

- **The `AtomicUnit` doc comment is stale - do not trust it for API shape.** The rustdoc on `atomic_unit_derive` claims it generates per-field getter/setter methods and `BorshSerialize`/`BorshDeserialize` impls. Neither happens: the actual expansion only produces `Data` and `AtomicUnit` impls. Real callers (e.g. `crates/storage/src/js.rs`) still write `#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]` explicitly, confirming Borsh must be derived separately.
- **No tests live in this crate.** Correctness is exercised entirely through `calimero-storage`'s own build/tests (its `entities.rs`, `collections.rs`, `js.rs`, and `tests/common.rs` all derive `AtomicUnit`/`Collection` on real types). If you change codegen here, `cargo test -p calimero-storage-macros` will pass trivially - run `cargo test -p calimero-storage` instead.
- **`Mergeable` is deliberately not derived here.** A second implementation used to live in this crate and drifted out of sync with the one in `calimero-sdk-macros`. The single canonical `#[derive(Mergeable)]` now lives in `crates/sdk/macros/src/mergeable.rs` (with forbidden-type field validation and friendlier enum/union errors) and is re-exported from `calimero_storage::collections`. Do not resurrect a `Mergeable` derive in this crate.
- **`#[storage]` is mandatory and singular.** `atomic_unit_derive` errors if no field carries `#[storage]`; it silently uses the *first* matching field if more than one does (no duplicate check) - keep struct definitions to exactly one.
- **`Collection`'s no-op Borsh impls assume the field never round-trips real data.** Any inline fields on a `Collection` struct are reconstructed via `Default` on decode, not from the byte stream - don't rely on them surviving serialize/deserialize.

Part of [crates/](../AGENTS.md).
