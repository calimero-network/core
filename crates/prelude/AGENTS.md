# calimero-prelude - Root Storage Key and Migration Constants

Shared constants and a `PublicKey` type for computing the WASM application's root storage key, used by both `calimero-sdk` and `calimero-storage` to avoid duplicating the key derivation.

## Package Identity

- **Crate**: `calimero-prelude`
- **Entry**: `src/lib.rs`
- **Key deps**: `sha2` (SHA-256 for key derivation), `borsh`/`serde` (derive, for `PublicKey`), `calimero-primitives` (`DIGEST_SIZE`)

## Commands

```bash
# Build
cargo build -p calimero-prelude

# Test
cargo test -p calimero-prelude
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `root_storage_key()` | fn | Computes the SHA-256 storage key for an app's root state entry |
| `ROOT_STORAGE_ENTRY_ID` | const | `[118u8; DIGEST_SIZE]` - fixed entry ID (`118` = ASCII `'v'`) hashed into the root key |
| `DIGEST_SIZE` | const | Re-export of `calimero_primitives::common::DIGEST_SIZE` (32) |
| `PublicKey` | struct | Newtype wrapping `[u8; 32]`; `Copy`, `Eq`, borsh/serde derives, `From<[u8; 32]>`, `AsRef<[u8]>` |

`root_storage_key` and `ROOT_STORAGE_ENTRY_ID` live in `src/constants.rs` and are re-exported at the crate root. `PublicKey` is defined directly in `src/lib.rs`.

## Mental Model

This crate exists to break a duplication problem: both `calimero-sdk` (host-side, seeding root storage during migrations via `host::seed_storage`) and `calimero-storage` (storage-layer key computation, re-exported from `crates/storage/src/constants.rs`) need the *exact same* root storage key. Rather than one depending on the other, both depend on this small, dependency-light crate.

`root_storage_key()` builds a 33-byte buffer: byte 0 is the `Key::Entry` discriminant (`0x01`), bytes 1-32 are `ROOT_STORAGE_ENTRY_ID`, then SHA-256 hashes the whole buffer. This must stay bit-for-bit identical to `Key::Entry(id).to_bytes()` in the storage layer - if the two ever diverge, migrations that seed via the SDK path and lookups via the storage path will disagree on where root state lives.

`PublicKey` is unrelated to the storage-key logic - it is a bare 32-byte key wrapper available for crates that need a minimal, dependency-light public key type without pulling in a full crypto crate.

Known callers today: `calimero-sdk` (`src/env.rs`, `src/state.rs`) calls `root_storage_key()` when seeding/reading root state; `calimero-storage` (`src/constants.rs`) re-exports `root_storage_key`; `calimero-context` (`update_application` handler) uses `ROOT_STORAGE_ENTRY_ID` directly, including in tests.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `PublicKey` type; re-exports `constants` module items |
| `src/constants.rs` | `DIGEST_SIZE` re-export, `ROOT_STORAGE_ENTRY_ID`, `root_storage_key()`, and its tests |

Part of [crates/](../AGENTS.md).
