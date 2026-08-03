# calimero-wasm-abi - WASM ABI v1 Schema, Description, and Embedder

Defines the `wasm-abi/1` manifest format that describes a Calimero app's methods, events, and state shape, plus the trait an app's types implement to describe themselves into one, and the tooling to validate it and embed/read it as a wasm custom section.

## Package Identity

- **Crate**: `calimero-wasm-abi`
- **Entry**: `src/lib.rs` (re-exports `schema::*`, `validate::*`, `AbiType`/`TypeRegistry`, `ManifestBuilder`; `downgrade` and `embed` stay namespaced, e.g. `calimero_wasm_abi::embed::write_embedded_state_schema`)
- **Key deps**: `wasmparser` 0.118 (reads the wasm custom-section container), `serde`/`serde_json` (manifest (de)serialization), `thiserror` (`ValidationError`), `jsonschema` (dev-only, validates `wasm-abi.schema.json` in tests)

## Commands

```bash
# Build
cargo build -p calimero-wasm-abi

# Test (unit tests in src/ + integration tests in tests/)
cargo test -p calimero-wasm-abi

# One integration test file
cargo test -p calimero-wasm-abi --test abi_type_std
cargo test -p calimero-wasm-abi --test schema_validation
cargo test -p calimero-wasm-abi --test identity_downgrade_real_scenarios

# A single case
cargo test -p calimero-wasm-abi authored_map_to_unordered_is_downgrade -- --nocapture
```

`tests/` holds the integration coverage on top of the `#[cfg(test)]` unit tests inside `src/*.rs`: `abi_type_std.rs` and `abi_type_registry.rs` (the std-type descriptions and the registry's recursion guard), `abi_manifest_builder.rs`, `schema_validation.rs` (validates real manifests against `wasm-abi.schema.json` via `jsonschema`), `invariants.rs`, `identity_downgrade_real_scenarios.rs`. The app-level shapes are locked by the goldens in `apps/abi_conformance*` instead, diffed by `scripts/verify-abi.sh` in CI.

## Module Map

| Module | Purpose |
| --- | --- |
| `schema` | The `Manifest` wire format: `TypeDef`, `TypeRef`, `Method`, `Event`, `MethodIntent`, `XCallCallers`, `CrdtCollectionType`, `collection_category()` |
| `abi_type` | The `AbiType` / `AbiEvents` traits each described type implements, plus `TypeRegistry`, which collects named `TypeDef`s and guards recursion |
| `manifest_builder` | `ManifestBuilder` - what generated `__calimero_abi()` code accumulates a `Manifest` into, applying the name sort `validate_manifest` requires |
| `validate` | `validate_manifest()` - schema-version, sort-order, dangling-ref, and shape checks |
| `embed` | `write_embedded_state_schema()` / `read_embedded_state_schema[_versioned]()` - the `calimero_abi_v1` wasm custom section |
| `downgrade` | `identity_downgrades()` - detects a root state field losing identity-gated CRDT semantics across two manifests |

## Schema Types (`schema.rs`)

| Type | Kind | Purpose |
| --- | --- | --- |
| `Manifest` | struct | `schema_version`, `types: BTreeMap<String, TypeDef>`, `methods`, `events`, `state_root`, `state_version`, `migrations: Vec<MigrationEdgeAbi>` |
| `TypeDef` | enum (`kind` tag) | `Record { fields }`, `Variant { variants }`, `Bytes { size, encoding }`, `Alias { target }` |
| `TypeRef` | enum (untagged) | `Reference { $ref }`, `Scalar(ScalarType)`, `Collection { collection, crdt_type, inner_type }` |
| `ScalarType` | enum (`kind` tag) | `Bool`, `I32`, `I64`, `U32`, `U64`, `F32`, `F64`, `String`, `Bytes { size, encoding }`, `Unit` |
| `CollectionType` | enum (`kind` tag) | `List { items }`, `Map { key, value }` (key custom-(de)serialized to accept a bare `"string"`), `Record { fields }`, `Tuple { elements }` (positional, so `(K, V)` describes `[k, v]` rather than a record's `{"0":k,"1":v}`) |
| `CrdtCollectionType` | enum (`#[non_exhaustive]`) | `LwwRegister`, `Counter`, `Vector`, `UnorderedMap`, `SortedMap`, `AuthoredMap`, `UnorderedSet`, `SortedSet`, `ReplicatedGrowableArray`, `AuthoredVector`, `SharedStorage` |
| `CollectionCategory` | enum | `Convergent` / `Replayable` / `IdentityGated` - classification returned by `collection_category()`, exhaustively matched (no wildcard) so a new `CrdtCollectionType` variant fails to compile until categorized |
| `MethodIntent` | enum, `#[default] Unspecified` | `ReadOnly` (`#[app::view]`), `Mutating`, `Unspecified` (fail-safe: treated as write lock) |
| `XCallCallers` | enum, `#[default] AnyInNamespace` | Who may call an `#[app::xcall]` method: `AnyInNamespace` or `SameApp` (`from_same_app`) |
| `Method` / `Parameter` / `Field` / `Variant` / `Error` / `Event` | structs | Manifest leaves; all serde-defaulted so old manifests round-trip |
| `MigrationEdgeAbi` | struct | One `{ method, from_version }` hop; `from_version + 1` is the target |

`collection_category()` is the single source of truth for migration safety, consumed by both `downgrade.rs` (the core L1 upgrade gate) and the `mero-abi diff` CI lint:
- **Convergent** (`LwwRegister`, `Vector`, `UnorderedMap`/`Set`, `SortedMap`/`Set`) - a migrate may freely rebuild these; no per-entry provenance to lose.
- **Replayable** (`Counter`, `ReplicatedGrowableArray`) - per-executor/per-position state; converges only if the migrate body replays deterministically.
- **IdentityGated** (`AuthoredMap`, `AuthoredVector`, `SharedStorage`) - ownership/writer-set derived from `env::executor_id()`; a naive rebuild diverges and downgrading to a non-gated type silently strips authorship/ACL.

`Manifest::extract_state_schema()` slices out just `state_root` plus its transitive type dependencies (walking `TypeDef`/`TypeRef` recursively) - the form the node embeds and reads, separate from the full method/event ABI.

## Mental Model: describe -> validate -> embed

1. **Describe** (`abi_type::AbiType`): every type in an ABI position implements `type_ref` (how a use site names it) and `register` (the named `TypeDef` it contributes, if any). The compiler picks the impls, so an alias is already the aliased type, a macro has already expanded, and a re-export resolves to its definition. `#[derive(AbiType)]` writes the impl for app structs and enums; this crate covers the std types, `calimero-storage` the CRDT wrappers, `calimero-primitives`/`calimero-account` the id types. `TypeRegistry::define` is idempotent by name and marks a name in progress before building its body, so a self-referential type terminates. `Option<T>` describes as `T` - nullability lives on the containing `Field`/`Parameter`/`Method.returns_nullable`. `Vec<u8>`/`[u8; N]` become `bytes`; other sequences become `list`; `BTreeMap`/`HashMap`/`IndexMap` are keyed by `String` only. A CRDT wrapper describes as a `Collection` carrying both the unwrapped shape and a `crdt_type` tag, so a downstream deserializer knows the real wire format (`LwwRegister<T>` is an empty `Record` + `inner_type: T`, since its wire format is `(value, timestamp, node_id)` the ABI does not spell out; `Counter` and `ReplicatedGrowableArray` have no useful generic shape and are opaque empty-`Record` placeholders tagged with their `crdt_type`).
2. **Assemble** (`manifest_builder::ManifestBuilder`): the `__calimero_abi()` that `#[app::logic]` generates registers types, methods, events, the state root/version and any migration edge into a builder; `finish()` stable-sorts methods and events by name. `cargo mero build` extracts the result by running that generated entry point (see `tools/cargo-mero`), which is the only producer of an app's manifest.
3. **Validate** (`validate::validate_manifest`): checks `schema_version` parses as `wasm-abi/<major>[.<minor>]` and that the major matches `SUPPORTED_SCHEMA_MAJOR` (currently 1) - a *different* major is `UnsupportedSchemaVersion` (distinct from a malformed tag's `InvalidSchemaVersion`), so callers can tell "newer toolchain" from "garbage". Also checks `methods`/`events` are name-sorted, map keys are `string`, non-zero `bytes` sizes, and that every `$ref` resolves to a declared type (no dangling references).
4. **Embed** (`embed::write_embedded_state_schema` / `read_embedded_state_schema[_versioned]`): the writer walks the wasm binary's section stream by hand (LEB128-decoding section headers, `wasmparser` isn't used for writing), drops any pre-existing `calimero_abi_v1` custom section, and appends exactly one fresh one carrying the JSON-serialized manifest - so re-embedding is idempotent replace, not append. The reader uses `wasmparser::Parser` to scan for `calimero_abi_v1` custom sections and returns a three-way `EmbeddedSchema`: `Supported(Manifest)`, `UnsupportedVersion(String)` (parses, but a future major - `validate_manifest` was called and returned `UnsupportedSchemaVersion`), or `Absent` (missing, malformed JSON, or fails validation for any other reason). Multiple sections resolve last-`Supported`-wins, and a `Supported` section is never overwritten by a later `UnsupportedVersion` one - a security property the identity-downgrade gate depends on (an opaque section must never demote a usable one). The convenience `read_embedded_state_schema()` collapses `UnsupportedVersion` to `None` (fail-open) for callers that only consume a schema they understand; security-sensitive callers must use the `_versioned` form to fail closed instead.

`downgrade::identity_downgrades(old, new)` is a separate top-level pass over two manifests' *root* state fields (not the general schema diff): for each old root field that resolves (following `$ref`/`Alias` hops, depth-capped at `MAX_REF_DEPTH = 32`) to an `IdentityGated` CRDT - or is unresolvable at all (cycle, dangling ref, unknown `crdt_type`) - it checks whether the field's new type is still identity-gated. A field that goes from gated to `Plain`, to a different non-gated shape, or is removed, is flagged. Unresolvable resolves fail-closed (treated as "could be gated") in every case except when both old and new are unresolvable in exactly the same way (an unchanged, always-broken legacy field) - that carve-out exists so a stale-but-harmless dangling ref doesn't perpetually trip the gate.

## Key Files

| Path | What's there |
| --- | --- |
| `src/schema.rs` (~1.1K lines) | `Manifest` and all wire types; unit tests including an `_is_exhaustive` compile tripwire pair (`method_intent_is_exhaustive`, `crdt_type_is_exhaustive`) that cross-checks the Rust enums against `wasm-abi.schema.json`'s `enum` lists |
| `src/abi_type.rs` (~300 lines) | `AbiType`, `AbiEvents`, `TypeRegistry`, and the std-type impls |
| `src/manifest_builder.rs` | `ManifestBuilder` |
| `src/validate.rs` (~420 lines) | `validate_manifest`, `ValidationError` |
| `src/embed.rs` (~425 lines) | `write_embedded_state_schema`, `read_embedded_state_schema[_versioned]`, `EmbeddedSchema`, hand-rolled LEB128 read/write |
| `src/downgrade.rs` (~290 lines) | `identity_downgrades`, `IdentityDowngrade` |
| `wasm-abi.schema.json` | Hand-maintained JSON Schema mirror of the Rust types, checked for enum-completeness by `schema.rs`'s tests |
| `tests/*.rs` | Integration coverage: `abi_type_std.rs`, `abi_type_registry.rs`, `abi_manifest_builder.rs`, `schema_validation.rs` (validates against `wasm-abi.schema.json` via `jsonschema`), `invariants.rs`, `identity_downgrade_real_scenarios.rs` |

## Relation to `tools/calimero-abi` (binary `mero-abi`)

The `mero-abi` CLI (package name `mero-abi`, binary/command `calimero-abi`) is the only consumer of this crate outside the SDK/node build path. It depends on `calimero-wasm-abi` directly and exposes:

- `extract` / `types` / `state` - read a compiled wasm and print/emit its ABI (via `extract.rs`, using `Manifest` and `extract_state_schema`)
- `inspect` - dump a wasm's sections (`inspect.rs`)
- `embed <wasm> <schema>` - calls `embed::write_embedded_state_schema` in place (`embed.rs`)
- `diff <current> <baseline>` - loads two `state-schema.json` files and reports breaking changes plus unsafe identity downgrades, built on `calimero_wasm_abi::schema` and (per its own help text) the same downgrade semantics `downgrade.rs` implements

## Invariants and Gotchas

- **Old manifests must keep deserializing.** Every field added after `wasm-abi/1` shipped (`intent`, `xcall_callable`, `xcall_callers`, `state_version`, `migrations`) is `#[serde(default, skip_serializing_if = ...)]` with a documented fail-safe default (`Unspecified` -> write lock, `AnyInNamespace` -> the historical open policy, `state_version` -> `1` via `state_version_or_default()`). Adding a field without a safe default and without the skip predicate breaks every already-compiled app's manifest.
- **`CollectionCategory` match has no wildcard.** `collection_category()` and the two `_is_exhaustive` compile tripwires in `schema.rs`'s tests are written to fail to compile when a new `CrdtCollectionType` variant is added without updating both the classification and `wasm-abi.schema.json`'s enum list - don't add a wildcard arm to "fix" the compile error.
- **`schema_version` matches by major, not exact string.** `SUPPORTED_SCHEMA_MAJOR = 1` in `validate.rs` accepts any `wasm-abi/1[.N]`; a genuinely breaking format change must bump the major, not just the minor, or old readers will silently accept an incompatible manifest.
- **`read_embedded_state_schema` (non-versioned) is fail-open by design** - it collapses `UnsupportedVersion` to `None`. Anything making a security decision (the identity-downgrade / upgrade gate) must use `read_embedded_state_schema_versioned` and treat `UnsupportedVersion` as "present but opaque," not as "absent."
- **The embed writer hand-parses the wasm section stream** rather than using `wasmparser` to rebuild it, so it can preserve every other section byte-for-byte; it fails closed (returns `EmbedError::MalformedWasm`) on a truncated LEB128, an overlong (>5-byte) LEB128, or a section that would run past the input - it never emits a corrupt module on bad input.
- **`identity_downgrades` only inspects root state fields**, not the whole type graph - it is the specific "did this top-level field lose identity-gating" check, not a general schema diff (that's `mero-abi diff`'s broader job).
- **Migration method naming**: an explicit `#[migrate(method = ...)]` is used verbatim; when omitted, the default is versioned (`migrate_v{N-1}_to_v{N}`) once `state_version > 1`, and only the bare `migrate` for the first migration - avoiding a name collision across releases.
- **`TypeRegistry::define` panics on a name redefined with a different shape.** Two distinct types describing under one name would otherwise silently emit a manifest describing the wrong one; a generic instantiated at two argument types is the way to hit this.

Part of [crates/](../AGENTS.md).
