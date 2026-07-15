# calimero-primitives - Shared Foundational Types

The dependency-free vocabulary of IDs, keys, and wire types (`ContextId`, `ApplicationId`, `PublicKey`, `Hash`, `CrdtType`, ...) that every other Calimero crate builds on.

## Package Identity

- **Crate**: `calimero-primitives`
- **Entry**: `src/lib.rs`
- **Key deps**: `ed25519-dalek` (signing/verification), `sha2` (SHA-256 for `Hash`), `bs58` (base58 text encoding), `zeroize` (key wiping), `borsh` (optional, feature-gated wire encoding), `serde`/`serde_json`, `multiaddr` + `url` (`common::multiaddr_to_url`)

## Commands

```bash
# Build (default features)
cargo build -p calimero-primitives

# Build with borsh enabled (most consumers use this)
cargo build -p calimero-primitives --features borsh

# Test (dev-deps reimport self with `borsh`, so tests exercise both encodings)
cargo test -p calimero-primitives

# Test a single module
cargo test -p calimero-primitives hash::
cargo test -p calimero-primitives crdt::tests -- --nocapture
```

There is no `default` feature (`default = []`); `borsh` is opt-in. The dev-profile reimports `calimero-primitives` itself with `features = ["borsh"]` (see `Cargo.toml`), so `cargo test` always compiles the borsh paths regardless of what a downstream crate enables.

## Module Layout

```
src/
├── lib.rs          # `pub mod` list only - no re-exports, no prelude
├── hash.rs          # Hash - the 32-byte digest every ID newtype wraps
├── identity.rs      # PrivateKey, PublicKey, Did, RootKey, ClientKey, ContextUser
├── context.rs        # ContextId, Context, ContextConfigParams, UpgradePolicy, GroupMemberRole
├── application.rs    # ApplicationId, SignerId, AppKey, Application, ApplicationBlob, Version (semver), ApplicationSource
├── blobs.rs           # BlobId, BlobInfo, BlobMetadata
├── crdt.rs             # CrdtType - merge-semantics tag shared by storage + sync
├── alias.rs             # Alias<T> - fixed-size human-readable name, scoped by ScopedAlias
├── metadata.rs           # MetadataRecord + validate_metadata_payload (group/context/member metadata)
├── events.rs              # NodeEvent / ContextEvent / ContextEventPayload (WS event wire types)
├── sync_status.rs          # SyncState (sync_status RPC + SyncStatus WS event)
├── version.rs               # Version (build/release metadata, distinct from application::Version)
├── common.rs                 # DIGEST_SIZE, ZERO_HASH, ResultAlt, multiaddr_to_url
├── reflect.rs                 # Reflect/ReflectExt - non-'static TypeId + downcast helpers
├── utils.rs                    # compact_path - shortens generic type-name paths for logging
└── tests/                       # out-of-line #[path] test modules for hash/application/alias
```

`lib.rs` is 14 lines of `pub mod` declarations and nothing else - no facade re-exports, no prelude module. Callers import from the specific module (`calimero_primitives::context::ContextId`, not a flattened root).

## Type Inventory

| Type | Module | Purpose | Bytes / Encoding |
| --- | --- | --- | --- |
| `Hash` | `hash` | Generic 32-byte SHA-256 digest; base58 `Display`/`Serialize`, raw bytes for `borsh` | 32 bytes; `Copy`; base58 text over serde, raw bytes over borsh |
| `ContextId` | `context` | Newtype over `Hash` identifying a context | 32 bytes, same encoding as `Hash` |
| `ApplicationId` | `application` | Newtype over `Hash` identifying an application; `ZERO_APPLICATION_ID` sentinel | 32 bytes |
| `BlobId` | `blobs` | Newtype over `Hash` identifying a stored blob | 32 bytes |
| `PublicKey` | `identity` | Newtype over `Hash`; Ed25519 verifying key | 32 bytes |
| `PrivateKey` | `identity` | Raw `[u8; 32]` Ed25519 signing key; `ZeroizeOnDrop`, no `Clone`/`Copy`/serde | 32 bytes, never serialized |
| `SignerId` | `application` | Non-empty `did:key:...` string identifying MPK bundle signer | variable-length string; length-prefixed under borsh |
| `AppKey` | `application` | `(app_id, signer_id)` pair; `Display`/`FromStr` as `"appId:signerId"` | text format; length-prefixed fields under borsh |
| `Version` (application) | `application` | Validated `major.minor.patch[-pre][+build]` semver string | newtype `Box<str>`; no borsh impl |
| `Version` (version) | `version` | Build/release metadata (version, build, commit, rustc) | serde/borsh; borsh decode caps each string at `MAX_VERSION_STRING_LEN` (256) |
| `Alias<T>` | `alias` | Fixed-capacity human-readable name, phantom-scoped to a `ScopedAlias` type | fixed `[u8; 50]` buffer + `u8` len; text over serde, no borsh impl |
| `CrdtType` | `crdt` | Merge-semantics tag shared by storage (persistence) and sync (wire classification) | enum; borsh discriminant pinned by explicit test, appended-only |
| `MetadataRecord` | `metadata` | Group/member/context metadata (`name`, opaque `data` map, `updated_at`, `updated_by`) | serde (`camelCase`) + borsh |
| `SyncState` | `sync_status` | Coarse sync phase for RPC + WS event | serde, internally tagged `state`; no borsh |
| `NodeEvent`/`ContextEvent`/... | `events` | WebSocket event envelope and payloads | serde only, tagged JSON |
| `Did`/`RootKey`/`ClientKey`/`ContextUser` | `identity` | DID-adjacent identity records | serde only, `#[non_exhaustive]` |
| `Context` | `context` | Snapshot of a context's id/app/root-hash/DAG-heads/version/name | serde (`camelCase`), `#[non_exhaustive]` builder methods |
| `UpgradePolicy` | `context` | `Automatic` / `LazyOnAccess` app-upgrade propagation | serde; hand-written borsh with a rejected legacy tag `2` |
| `GroupMemberRole` | `context` | `Admin`/`Member`/`ReadOnly`/`ReadOnlyTee` | serde + borsh, deliberately NOT `#[non_exhaustive]` |

## Mental Model

Every ID type in the workspace (`ContextId`, `ApplicationId`, `BlobId`, `PublicKey`) is the same 32-byte `Hash` wrapped in a distinct newtype, so the compiler - not a convention - stops a `BlobId` from being passed where an `ApplicationId` is expected. `Hash` itself stores only the raw bytes and computes its base58 form on demand (`Display`, `to_base58`, `encode_base58`), trading a cached string for a `Copy` struct that stays cheap on hot paths like delta-store iteration and RocksDB key parsing.

This crate has no dependency on any other Calimero crate - it is the leaf that `calimero-crypto`, `calimero-storage`, `calimero-node`, `calimero-server`, and the rest of the workspace (25+ crates) depend on for the IDs and keys that cross module boundaries. `CrdtType` living here (rather than in `calimero-storage`) is what lets both the storage merge dispatcher and the sync wire protocol agree on one enum without either depending on the other.

## Invariants and Gotchas

- **`PublicKey::verify` uses `verify_strict`, not `verify`**: it rejects the malleable, non-canonical Ed25519 signature encodings that plain `verify` accepts (small-order/cofactored/non-strict-SUF-CMA edge cases). Do not swap this for `VerifyingKey::verify` - that reopens a signature-malleability hole.
- **`PrivateKey` never derives serialization or `Clone`/`Copy`**: the only sanctioned way to reach the raw bytes is `as_bytes()`. Serializing or copying it would produce an untracked copy of the secret that `ZeroizeOnDrop` cannot wipe. The inner type is a plain `[u8; 32]` (not `Hash`) specifically so the derived `ZeroizeOnDrop` can zero it directly.
- **All 32-byte digest newtypes (`Hash`, `ContextId`, `ApplicationId`, `BlobId`, `PublicKey`) share one text encoding**: base58 over Serde (`Display`/`FromStr`/`Serialize`), raw fixed bytes over borsh (only under the `borsh` feature). Do not mix - a base58 string is never valid input to a borsh decoder for these types.
- **`CrdtType`'s borsh tags are pinned and append-only**: a dedicated test (`test_borsh_discriminant_tags_are_stable`) locks each variant's discriminant. `RotationLog` was deliberately added last to avoid shifting later tags. New variants must be appended at the end, never inserted.
- **`UpgradePolicy`'s borsh decoder explicitly rejects tag `2`** (the removed `Coordinated` policy) instead of silently reinterpreting it - a stored/in-flight legacy value fails loudly rather than resurrecting removed semantics.
- **`Alias<T>` is a fixed 50-byte buffer, not a `String`**: characters are restricted to `[A-Za-z0-9._-]` because aliases are interpolated into store keys and URL paths; `MAX_ALIAS_LEN` has a compile-time assertion that it fits in `u8`. `ScopedAlias` is a marker trait (`PublicKey`'s scope is `ContextId`; `ContextId`/`ApplicationId` are unscoped, `Scope = ()`) used purely as a phantom type parameter - there's no runtime scope check in this crate.
- **`version::Version`'s `BorshDeserialize` is hand-written, not derived**: each string field is read through `read_capped_string`, which checks the length prefix against `MAX_VERSION_STRING_LEN` (256) *before* allocating, so a hostile peer can't force a multi-gigabyte allocation via a crafted length prefix during handshake. `BorshSerialize` is still derived.
- **`application::Version` (semver) and `version::Version` (build metadata) are unrelated types that happen to share a name** - the former validates `major.minor.patch`, the latter carries release/build/commit/rustc strings; import the one you mean explicitly.
- **`MetadataRecord` has no `Default`**: a record always carries a real `updated_by` signer; a `Default` would have to fabricate an all-zero `PublicKey` that looks like a valid signature authority.
- **`reflect.rs`'s `non_static_type_id` uses an internal `transmute` + `unsafe`** to get a `TypeId` for non-`'static` types (ported from an unmerged `castaway` PR). Treat it as settled, audited code rather than a pattern to copy elsewhere.

Part of [crates/](../AGENTS.md).
