# calimero-store - Node-Local KV Store

The typed key-value store layer over a pluggable backend (RocksDB in production, an in-memory `BTreeMap` for tests), including transactions, layered read/write views, and optional at-rest encryption and content-addressed blob storage.

## Package Identity

- **Crate**: `calimero-store`
- **Entry**: `src/lib.rs`
- **Key deps**: `generic-array` + `typenum` (fixed-width, compile-time-sized keys), `thunderdome` (slot map, dev use), `strum` (`Column` enum iteration/naming), `camino` (UTF-8 paths), `zeroize` (wipe the at-rest encryption key), `eyre` (error type across the whole crate), `borsh`/`serde` (optional, gated by the `borsh`/`serde`/`datatypes` features)
- **Backend crate** (separate package, not a Cargo dependency of `calimero-store` - it depends the other way): `calimero-store-rocksdb` at `crates/store/impl/rocksdb`, implementing `db::Database` for RocksDB.

## calimero-store vs calimero-storage - do not confuse these

- **`calimero-store`** (this crate) is the **KV/RocksDB layer**: column families, byte-level keys, `Store::get`/`put`/`apply`, encryption-at-rest, blob chunk storage. It has no idea what a CRDT is.
- **`calimero-storage`** (`crates/storage`) is the **CRDT collections layer**: `UnorderedMap`, `Vector`, `GCounter`, merge semantics, the Merkle entity index. It is *not* a Cargo dependency of, or on, this crate - it talks to storage only through host-provided callbacks (`env::RuntimeEnv`'s `storage_read`/`storage_write`/`storage_remove` function pointers), decoupling the CRDT logic from any concrete backend. In practice the node wires those callbacks to a `calimero-store` `Handle`, but nothing in `calimero-storage`'s source references this crate directly.

If you're debugging "why did this counter not merge correctly," you want `crates/storage`. If you're debugging "why did this byte sequence not get written to disk" or "why is this column growing unbounded," you want this crate.

## Commands

```bash
# Build (default features; add datatypes for the borsh-typed value structs)
cargo build -p calimero-store
cargo build -p calimero-store --features datatypes

# Unit tests (in-memory DB, Slice, iterator merge logic)
cargo test -p calimero-store

# A specific test
cargo test -p calimero-store merges_inner_and_shadow_in_sorted_order -- --nocapture

# Sub-crates
cargo build -p calimero-store-rocksdb   # RocksDB Database impl
cargo test -p calimero-store-rocksdb
cargo build -p calimero-store-encryption
cargo test -p calimero-store-encryption
cargo build -p calimero-blobstore
cargo test -p calimero-blobstore
```

## Type/Module Inventory

| Module | Purpose |
| --- | --- |
| `Store` (`lib.rs`) | `Arc<dyn Database>` handle; the crate's front door - `open`, `handle()`, `flush`, `ping`, `apply`, plus raw (untyped) column access: `raw_put`/`raw_delete`/`raw_delete_range`/`raw_scan`/`raw_last` |
| `db` | `Database<'a>` trait (backend contract) and `Column` enum (the CF list); `db::memory::InMemoryDB` (`Ref`/`Owned` variants) is the only in-crate implementation |
| `key` | `Key<T: KeyComponents>` - a `repr(transparent)` fixed-width byte array tagged with its component layout; `AsKeyParts`/`FromKeyParts` traits; one submodule per key family (`alias`, `application`, `blobs`, `component`, `context`, `generic`, `group`, `absorb`) |
| `types` | `PredefinedEntry` trait (key + its value codec) and the concrete value types stored under each key family; gated behind the `datatypes` feature |
| `entry` | `Entry`/`Codec` traits; `Identity`, `Json`, `Borsh` codec impls |
| `handle` | `Handle<L>` - the typed `get`/`put`/`delete`/`iter` API over any `Layer` |
| `layer` | `Layer`/`ReadLayer`/`WriteLayer` traits; `layer::temporal::Temporal` (shadow-transaction overlay) and `layer::read_only::ReadOnly` (read-only view) |
| `tx` | `Transaction` - an in-memory `BTreeMap<Column, BTreeMap<key, Operation>>` staged write set; `Operation::{Put, Delete}` |
| `batch` | `StoreBatch` - the public "stage typed puts/deletes, commit atomically" API (feature `datatypes`) |
| `iter` | `DBIter` trait, `Iter<K, V>` (structured/unstructured key and value projections), `IterKeys`/`IterEntries` adapters, `IterPair` (merge two iterators) |
| `slice` | `Slice<'a>` - a cheap-clone, `Arc`-backed byte-slice type used everywhere instead of `Vec<u8>`/`&[u8]` |
| `config` | `StoreConfig` - backend path plus an optional `Zeroizing<Vec<u8>>` at-rest `encryption_key` |

## Mental Model

**Layering.** `Store` wraps `Arc<dyn Database>` and is itself the base `Layer`. `Handle<L>` sits on top and adds the *typed* API (`Entry`/codec-aware `get`/`put`), while `Layer`/`ReadLayer`/`WriteLayer` stay untyped (raw `Slice` in, `Slice` out). Two layer adapters compose over any base layer:
- `Temporal` buffers writes into a shadow `Transaction` and only reaches the backend on `commit()` - reads see committed data merged with the pending shadow (shadow wins on conflict, deletions hide the base value). Its iterator (`TemporalIterator`) does a sorted two-way merge of the base iterator and the shadow's per-column `BTreeMap` range, so `iter()` sees a consistent overlaid view without materializing anything.
- `ReadOnly` is a read-only pass-through, used where a caller must be statically prevented from writing.

**Columns are the schema.** `db::Column` (in `src/db.rs`) is the RocksDB column-family list - it *is* the crate's schema, and every variant's doc comment explains why it exists as its own CF (mostly: "this key shape would collide with an existing column's key shape," or "this is node-local and must never sync"). Notable groups:
- Synced (replicated) state: `State`, `Delta`, `Application`, `Group`, `Blobs`, `Identity`, `Alias`, `Generic`, `UnifiedOp` (the new unified causal-log op-store, mid dual-write cutover with `Delta`).
- Node-local, explicitly NOT synchronized: `PrivateState`, `ContextLocal`, `SortedIndex` (materialized secondary index for `SortedMap`, unhashed keys so byte order = key order), `AbsorbBuffer` (buffered straggler deltas under an unreadable schema), `ContextMigrationFailed`, `ContextExecutingBlob`/`ContextActivatedBlob`/`ApplicationPreviousBlob` (migration/rollback bookkeeping), `ContextResyncRequested`.
- `Column` is `#[non_exhaustive]` and `EnumIter`-derived; `RocksDB::open` creates every CF from `Column::iter()` automatically - adding a variant needs no manual migration step, but *removing* or *renaming* one does (existing on-disk CFs must still open).

**Keys are compile-time-sized.** `Key<T: KeyComponents>` is a `GenericArray<u8, T::LEN>` where `T::LEN` is a `typenum` const computed by concatenating component lengths (`component.rs`'s `impl_key_components!` macro handles tuples up to 16 components). Each key family (`key/context.rs`, `key/group/mod.rs`, etc.) defines zero-sized `KeyComponent` marker structs (e.g. `ContextId: U32`, `PublicKeyComponent`) and composes them into a concrete key type via `Key<(A, B, ...)>`. This buys two things: the key layout is enforced by the type system (you cannot accidentally build a `ContextState` key with the wrong byte length), and prefix scans are just "iterate a column, byte-compare a prefix" because related keys share a leading component (e.g. every `ContextDagDelta` row for a context shares its `ContextId` prefix).

**Values are codec-agnostic.** `Entry::Codec` decouples the key type from how the value is serialized - `Identity` (raw bytes via `AsRef<[u8]>`/`TryFrom<Slice>`), `Json` (serde, feature `serde`), `Borsh` (feature `borsh`). `PredefinedEntry` (in `types.rs`) is the trait actual value types implement to pick their codec once; a blanket `impl<T: PredefinedEntry> Entry for T` wires it into the generic `Handle`/`StoreBatch` API.

**Atomicity.** `Transaction` is a pure in-memory staging structure (no I/O). `Store::apply(&tx)` is the one place multi-key atomicity happens: on RocksDB it becomes a single `WriteBatch::write`, so a transaction spanning several keys - possibly several columns - either lands completely or not at all. `StoreBatch` is the ergonomic typed wrapper over this for callers who don't need `Temporal`'s overlay semantics.

**Durability.** `Store::flush()` calls down to `Database::flush()`, which is a no-op for `InMemoryDB` and flushes RocksDB's WAL then its memtables (`flush_wal(true)` before `flush()` - WAL first, so an interruption mid-flush still leaves every write durable via the WAL). RocksDB's `Drop` impl repeats the same two calls as a last-resort backstop for an abrupt shutdown that skipped the explicit `flush()`.

**Snapshots.** `Database::iter_snapshot` gives a frozen point-in-time iterator (RocksDB: `db.snapshot()` + `ReadOptions::set_snapshot`), used where a caller needs a consistent view across a walk that a concurrent writer must not perturb (e.g. generating a state snapshot for sync). The default trait impl just falls back to `iter()` for backends without native snapshot support.

## Encryption at Rest (calimero-store-encryption)

Package `calimero-store-encryption`, crate root `encryption/src/lib.rs`. `EncryptedDatabase<D>` wraps any `Database` impl `D` and transparently encrypts values while **leaving keys in plaintext** - iteration and prefix/range scans must still work directly against the wrapped backend, and encrypting keys would break both.

- `KeyManager` (`key_manager.rs`) derives versioned AES-256-GCM Data Encryption Keys (DEKs) from a single master Key Encryption Key (KEK, supplied by the caller - typically from a KMS/dstack attestation) via `HKDF-SHA256` with a per-version salt (`"calimero-dek-v{version}"`).
- **32-byte master-key floor**: `KeyManager::new` rejects any master key shorter than `AES_KEY_SIZE = 32` bytes - HKDF does not add entropy, so a shorter input would still stretch into a full-strength-looking but actually weak 32-byte DEK. This is a hard `bail!`, not a warning.
- Ciphertext format is `version(1) ‖ nonce(12) ‖ AES-256-GCM(ciphertext + 16-byte tag)`. The version and nonce header is bound as AAD (`aad_for`), so flipping the version byte to force a different DEK - or tampering the nonce - fails the GCM tag check rather than silently decrypting under the wrong key.
- **Key rotation** (`rotate_key`) bumps `current_version` and derives a new DEK; old DEKs stay cached so previously-written data at any prior version keeps decrypting. There is no re-encryption pass - rotation only changes what *new* writes use.
- `EncryptedDatabase::open()` (the `Database::open` trait method) always errors - it cannot self-bootstrap a master key from `StoreConfig` alone. Construct it via `EncryptedDatabase::wrap(inner_db, master_key)` after opening the inner backend yourself.
- `apply()` re-encrypts every `Put` value in the transaction before delegating to `inner.apply()`, so multi-key atomicity is preserved end to end even with encryption in the path.

## Blob Storage (calimero-blobstore)

Package `calimero-blobstore`, crate root `blobs/src/lib.rs`. `BlobManager` pairs a `calimero_store::Store` (metadata: `BlobMeta` rows in `Column::Blobs`) with a `FileSystem` blob repository (content bytes on disk, one file per `BlobId`, path = `root/<blob-id>`).

- **Content-addressed, chunked, deduplicated.** `put_sized` streams input through SHA-256 in `CHUNK_SIZE = 1 MiB` pieces; each chunk's id is the hash of its own bytes, and the root blob's id is the hash of the concatenated chunk ids. Identical content anywhere - a whole file, or a repeated chunk within one file - collapses to the same on-disk blob and the same `BlobMeta` row.
- **Reference counted.** `BlobMeta.refs` tracks how many logical owners point at a given id (root or chunk). `put`/`persist_ref` increments; `delete`/`release_ref` decrements and only removes the metadata row + file once the count hits zero. A root's `delete` releases one reference from each of its chunk links too - symmetric with `put_sized` incrementing every chunk on add.
- **Not atomic across the add/delete read-modify-write.** `persist_ref`/`release_ref` are documented `INVARIANT`s: callers must serialize add/delete for the same blob id; a concurrent interleaving could lose an increment and free content a live owner still references. Making this atomic would require moving the refcount update onto `Store::apply`'s transaction path.
- **Read path re-verifies content hashes.** `Blob::new` walks the meta graph as an explicit iterative DFS (a `Vec`-based stack, not recursion) bounded by `MAX_BLOB_DEPTH = 64` and `MAX_BLOB_NODES = 1<<20` - the graph is peer-synced, untrusted data, so a deep chain or cycle must be rejected (`BlobError::CorruptGraph`) rather than blow the stack or loop forever. Every leaf chunk is re-hashed against its own id before being yielded (`BlobError::IntegrityMismatch` on mismatch) - a tampered on-disk file is refused, not served.
- `FileSystem` also exposes `package_path`/`version_path`/`application_blob_path` for the applications directory layout (`root/applications/<package>/<version>/blobs/<id>`), each validating path components (`utils::validate_path_component`) to block path traversal.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `Store`, `flush`, `ping`, `apply`, raw column access (`raw_put`/`raw_scan`/etc. for `SortedMap`, core#2559) |
| `src/db.rs` | `Column` enum (read every variant's doc comment before adding one - each explains a real collision it avoids), `Database` trait with its default `delete_range`/`last_in_range`/`approximate_size` implementations |
| `src/db/memory.rs`, `src/db/memory/raw.rs` | `InMemoryDB` - the test/dev backend |
| `src/key.rs` + `src/key/*.rs` | `Key<T>`, `AsKeyParts`/`FromKeyParts`, and every concrete key family |
| `src/layer.rs`, `src/layer/temporal.rs`, `src/layer/read_only.rs` | The layer traits and the two overlay adapters |
| `src/tx.rs` | `Transaction`, `Operation` |
| `src/batch.rs` | `StoreBatch` |
| `src/iter.rs` | `DBIter`, `Iter`, structured/unstructured projections |
| `src/slice.rs` | `Slice<'a>` |
| `src/config.rs` | `StoreConfig` (path + optional at-rest `encryption_key`) |
| `impl/rocksdb/src/lib.rs` | `RocksDB: Database` - the only production backend; also the reference for what a `Database` impl must get right (CF handles, `WriteBatch`, snapshot iterators, WAL flush order) |
| `encryption/src/lib.rs`, `encryption/src/key_manager.rs` | `EncryptedDatabase`, `KeyManager` |
| `blobs/src/lib.rs`, `blobs/src/config.rs` | `BlobManager`, `Blob`, `FileSystem`, `BlobStoreConfig` |

## Invariants and Gotchas

- **`Column` additions are migration-free; removals/renames are not.** RocksDB CFs are created from `Column::iter()` at `open_cf`, so a new variant just works on next start. Renaming or removing one orphans (or fails to open) existing on-disk data - treat that as a real migration, not a rename.
- **Key collisions are the reason columns proliferate.** Several near-identical `context_id`-only key shapes each got their own `Column` (`ContextMigrationFailed`, `ContextExecutingBlob`, `ContextResyncRequested`, ...) specifically so they can't collide with `ContextLocal`/`Application` rows of the same byte length. When adding a new node-local marker, check whether an existing column's key shape would collide before reusing it.
- **`Temporal`'s shadow always wins**, including for iteration order - a key present in both the base layer and the shadow transaction is yielded once, from the shadow. Dropping a `Temporal` without calling `commit()` discards every staged write; nothing reaches the backend until then (see `StoreBatch`'s `drop_persists_nothing` test for the same property on the batch API).
- **`Store::apply` is the only true atomicity boundary.** Anything that needs several keys (possibly across columns) to move together - e.g. a delta record plus its context's `dag_heads` - must go through one `Transaction`/`apply` call, not sequential `put`s.
- **Encryption keeps keys in plaintext by design.** Do not "fix" this by trying to encrypt keys too - it would break every prefix/range scan the store relies on (`SortedMap`, blob meta-graph traversal, alias lookups).
- **HKDF info string / salt changes need a version bump**, mirroring the pattern in `calimero-crypto`: if the DEK derivation scheme ever changes, bump the version scheme so old and new derivations can never silently collide.
- **Blob add/delete must be serialized per id** by the caller - the refcount read-modify-write across `persist_ref`/`release_ref` is not atomic against a concurrent add/delete of the same id.
- **`Slice<'a>` is the crate's universal buffer type** - prefer it over `Vec<u8>`/`&[u8]` in new store-facing code; it's `Arc`-backed so cloning is cheap and it round-trips through `Box<[u8]>`/`Vec<u8>`/borrowed refs without forcing a copy at every layer boundary.
- **Iterator borrow lifetimes**: `DBIter`-yielded slices borrow the backend's internal cursor buffer and are invalidated by the next `next()`/`read()` call. `IterKeys`/`IterEntries` copy into an owned `Slice` before yielding for exactly this reason - don't bypass them and hold a raw `DBIter` slice across an iteration step.

Part of [crates/](../AGENTS.md).
