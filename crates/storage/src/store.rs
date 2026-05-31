//! Storage operations.

use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::env::{
    private_storage_read, private_storage_remove, private_storage_write, storage_read,
    storage_remove, storage_write,
};

/// A key for storage operations.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Key {
    /// An index key.
    Index(Id),

    /// An entry key.
    Entry(Id),

    /// Sync state key for tracking last sync time with a remote node.
    SyncState(Id),

    /// Rotation log key for `SharedStorage<T>` writer-set history.
    ///
    /// Stores a [`RotationLog`](crate::rotation_log::RotationLog) per Shared
    /// entity so the verifier (P3 of #2233) can resolve `writers_at(causal_point)`
    /// for actions that pre-date the current writer set. Tag `3` to keep the
    /// existing `Index`/`Entry`/`SyncState` byte layout stable.
    RotationLog(Id),

    /// Validity marker for a `SortedMap`'s ordered secondary index (core#2559).
    ///
    /// Stores the collection's `full_hash` at the moment its `Column::SortedIndex`
    /// entries were last (re)built. An ordered read compares this to the
    /// collection's current `full_hash`: equal ⇒ the index is current and can be
    /// queried directly; different (a local write or a remote sync changed the
    /// entry set) ⇒ rebuild the index once, then serve. Node-local, not synced.
    /// Tag `4` keeps the existing byte layout stable.
    SortedIndexMeta(Id),
}

impl Key {
    /// Converts the key to a byte array.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut bytes = [0; 33];
        match *self {
            Self::Index(id) => {
                bytes[0] = 0;
                bytes[1..33].copy_from_slice(id.as_bytes());
            }
            Self::Entry(id) => {
                bytes[0] = 1;
                bytes[1..33].copy_from_slice(id.as_bytes());
            }
            Self::SyncState(id) => {
                bytes[0] = 2;
                bytes[1..33].copy_from_slice(id.as_bytes());
            }
            Self::RotationLog(id) => {
                bytes[0] = 3;
                bytes[1..33].copy_from_slice(id.as_bytes());
            }
            Self::SortedIndexMeta(id) => {
                bytes[0] = 4;
                bytes[1..33].copy_from_slice(id.as_bytes());
            }
        }
        Sha256::digest(bytes).into()
    }
}

/// Core storage operations (read, write, remove).
///
/// Base trait for all storage backends. Provides fundamental CRUD operations
/// without requiring iteration support.
///
/// # `'static` supertrait bound
///
/// Implementors must be `'static` so that `TypeId::of::<Self>()` works — it
/// keys the per-adaptor thread-local state used by
/// `DeferredAncestorScope` (#2238). In practice every implementor is a
/// unit/const struct (`MainStorage`, `MockedStorage<N>`), so this is
/// satisfied trivially; the explicit bound just removes the need for
/// `+ 'static` at every use site of `Index<S>` / `Interface<S>` /
/// `Collection<T, S>` across the crate.
pub trait StorageAdaptor: 'static {
    /// Reads data from persistent storage.
    fn storage_read(key: Key) -> Option<Vec<u8>>;

    /// Removes data from persistent storage.
    fn storage_remove(key: Key) -> bool;

    /// Writes data to persistent storage.
    fn storage_write(key: Key, value: &[u8]) -> bool;

    /// Whether writes through this adaptor participate in the synced
    /// state delta stream.
    ///
    /// `Interface::save_raw` (and the `Action::Compare` push at the end of
    /// `Interface::apply_action`, and the `Action::DeleteRef` push at the
    /// end of the delete path) records every storage mutation as an
    /// `Action` in the global delta stream. Those actions get bundled into
    /// the next outgoing `StateDelta` and replayed on every peer.
    ///
    /// That's correct for `MainStorage`, where the whole point is that
    /// writes propagate. It's a bug for `PrivateStorage`: a write that
    /// stays on this node by design should not produce a wire action.
    /// Otherwise peers replay the action against their own `MainStorage`,
    /// creating entities the author doesn't have — the #2319 "Same DAG
    /// heads, different root hash" divergence pattern (one extra
    /// `crdt_type=None, field_name=None` child appearing in the receiver's
    /// context-root index entry after every `PrivateStorage` collection
    /// construction).
    ///
    /// Default is `true` (sync-participating). `PrivateStorage` overrides
    /// to `false`. Test mocks default to `true` since they stand in for
    /// `MainStorage` in unit tests.
    fn participates_in_sync() -> bool {
        true
    }

    // === Ordered secondary index (SortedMap, core#2559) ===
    //
    // A node-local, derived, NON-synced index keyed by
    // `collection_id ‖ order_key` (unhashed, so the backend's byte order is
    // the logical key order). It lets `SortedMap` answer range/prefix/page
    // queries in `O(log n + k)` instead of scanning + sorting every entry.
    //
    // Adaptors that can't provide an ordered keyspace leave these at their
    // defaults: `index_supported()` stays `false` and `SortedMap` transparently
    // falls back to its in-memory sort. So this is purely additive — no
    // existing adaptor behaviour changes.

    /// Whether this adaptor backs the ordered secondary index. When `false`
    /// (the default), `SortedMap` ignores the index methods and sorts in
    /// memory. `MainStorage` (RocksDB) and the test mocks override to `true`.
    fn index_supported() -> bool {
        false
    }

    /// Insert/update `collection ‖ order_key -> entry` in the ordered index.
    /// Idempotent by `(collection, order_key)`.
    fn index_put(collection: Id, order_key: &[u8], entry: Id) {
        let _ = (collection, order_key, entry);
    }

    /// Remove `collection ‖ order_key` from the ordered index. No-op if absent.
    fn index_remove(collection: Id, order_key: &[u8]) {
        let _ = (collection, order_key);
    }

    /// Drop every index entry for `collection`. Used when rebuilding the index
    /// from scratch (e.g. after a remote sync changed the entry set).
    fn index_clear(collection: Id) {
        let _ = collection;
    }

    /// Return `(order_key, entry_id)` pairs for `collection` whose order key
    /// falls within `[start, end)` (per the `Bound`s), ascending, after
    /// skipping `offset` and capped at `limit` (`None` = unbounded).
    fn index_range(
        collection: Id,
        start: core::ops::Bound<Vec<u8>>,
        end: core::ops::Bound<Vec<u8>>,
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Id)> {
        let _ = (collection, start, end, offset, limit);
        Vec::new()
    }

    /// Return `(order_key, entry_id)` pairs for `collection` whose order key
    /// starts with `prefix`, ascending, after `offset`, capped at `limit`.
    fn index_prefix(
        collection: Id,
        prefix: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Id)> {
        let _ = (collection, prefix, offset, limit);
        Vec::new()
    }

    /// Return the `(order_key, entry_id)` with the largest order key in
    /// `collection` — a reverse seek for `SortedMap::last` (`O(log n)`).
    fn index_last(collection: Id) -> Option<(Vec<u8>, Id)> {
        let _ = collection;
        None
    }
}

/// Storage iteration support for GC and snapshots.
///
/// Optional trait for storage backends that support key iteration.
/// Required for garbage collection and full resync snapshot generation.
///
/// # ISP (Interface Segregation Principle)
///
/// This trait is separate from `StorageAdaptor` to avoid forcing all
/// implementations to support iteration. Some storage backends (e.g., WASM
/// environment without backend access) may not be able to efficiently iterate
/// all keys.
///
pub trait IterableStorage: StorageAdaptor {
    /// Iterates over all keys in storage.
    ///
    /// Returns all keys currently in storage. Used for:
    /// - Garbage collection of old tombstones
    /// - Full resync snapshot generation
    ///
    /// # Implementation Note
    ///
    /// For large datasets, consider returning an iterator instead of Vec
    /// to avoid memory overhead. This would require changing the return type
    /// to `Box<dyn Iterator<Item = Key>>`.
    ///
    fn storage_iter_keys() -> Vec<Key>;
}

/// The main storage system.
///
/// This is the default storage system, and is used for the main storage
/// operations in the system. It uses the environment's storage system to
/// perform the actual storage operations.
///
/// It is the only one intended for use in production, with other options being
/// implemented internally for testing purposes.
///
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub struct MainStorage;

impl StorageAdaptor for MainStorage {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        storage_read(key)
    }

    fn storage_remove(key: Key) -> bool {
        storage_remove(key)
    }

    fn storage_write(key: Key, value: &[u8]) -> bool {
        storage_write(key, value)
    }

    // Ordered index, routed to the env layer (host functions in wasm reaching
    // the node's RocksDB `SortedIndex` column via `ContextStorage`; an
    // in-memory ordered map in native tests). Composite keys are
    // `collection_id ‖ order_key` (unhashed), so the backend's byte order is the
    // logical key order — making range/prefix/pagination a native seek.

    fn index_supported() -> bool {
        true
    }

    fn index_put(collection: Id, order_key: &[u8], entry: Id) {
        crate::env::storage_index_set(&index_key(collection, order_key), entry.as_bytes());
    }

    fn index_remove(collection: Id, order_key: &[u8]) {
        crate::env::storage_index_remove(&index_key(collection, order_key));
    }

    fn index_clear(collection: Id) {
        crate::env::storage_index_remove_prefix(collection.as_bytes());
    }

    fn index_range(
        collection: Id,
        start: core::ops::Bound<Vec<u8>>,
        end: core::ops::Bound<Vec<u8>>,
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Id)> {
        use core::ops::Bound::{Excluded, Included, Unbounded};
        let prefix = collection.as_bytes();
        // env scan is `[lo, hi)`; translate the inclusive/exclusive bounds into
        // byte bounds (append 0x00 to make an inclusive end / exclusive start).
        let lo = match start {
            Included(k) => index_key(collection, &k),
            Excluded(k) => {
                let mut b = index_key(collection, &k);
                b.push(0);
                b
            }
            Unbounded => prefix.to_vec(),
        };
        let hi = match end {
            Excluded(k) => index_key(collection, &k),
            Included(k) => {
                let mut b = index_key(collection, &k);
                b.push(0);
                b
            }
            Unbounded => prefix_upper_bound(prefix),
        };
        decode_index_hits(
            prefix,
            crate::env::storage_index_scan(&lo, &hi, offset, limit),
        )
    }

    fn index_prefix(
        collection: Id,
        prefix: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Id)> {
        let coll = collection.as_bytes();
        let lo = index_key(collection, prefix);
        let hi = prefix_upper_bound(&lo);
        decode_index_hits(
            coll,
            crate::env::storage_index_scan(&lo, &hi, offset, limit),
        )
    }

    fn index_last(collection: Id) -> Option<(Vec<u8>, Id)> {
        let prefix = collection.as_bytes();
        let lo = prefix.to_vec();
        let hi = prefix_upper_bound(prefix);
        let (composite, entry_bytes) = crate::env::storage_index_last(&lo, &hi)?;
        let order_key = composite.strip_prefix(prefix)?.to_vec();
        let id: [u8; 32] = entry_bytes.as_slice().try_into().ok()?;
        Some((order_key, Id::new(id)))
    }
}

/// Build the ordered-index composite key `collection_id ‖ order_key`.
fn index_key(collection: Id, order_key: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(32 + order_key.len());
    key.extend_from_slice(collection.as_bytes());
    key.extend_from_slice(order_key);
    key
}

/// The exclusive upper bound for a byte prefix: the smallest key that does NOT
/// start with `prefix` (used to scan "all keys under this prefix"). For the
/// all-`0xFF` corner (astronomically unlikely for a 32-byte id) we fall back to
/// a longer all-`0xFF` key, which still bounds the scan.
fn prefix_upper_bound(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(&last) = end.last() {
        if last == 0xFF {
            let _ = end.pop();
        } else {
            *end.last_mut().expect("non-empty") += 1;
            return end;
        }
    }
    vec![0xFF; prefix.len() + 1]
}

/// Map raw `(composite_key, entry_id_bytes)` scan hits back to
/// `(order_key, entry_id)` by stripping the `collection_id` prefix.
fn decode_index_hits(prefix: &[u8], hits: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<(Vec<u8>, Id)> {
    hits.into_iter()
        .filter_map(|(composite, entry_bytes)| {
            let order_key = composite.strip_prefix(prefix)?.to_vec();
            let id: [u8; 32] = entry_bytes.as_slice().try_into().ok()?;
            Some((order_key, Id::new(id)))
        })
        .collect()
}

/// Node-local (private) storage adaptor.
///
/// Routes all reads/writes/removes to the node-local private-storage
/// namespace via [`env::private_storage_read`](crate::env::private_storage_read)
/// and friends. Entries stored here **do not** participate in the
/// synced Merkle tree — they stay on the node that wrote them.
///
/// # When to use
///
/// `PrivateStorage` makes sense when you need scalable node-local
/// state where only the *entries you change* are rewritten, not the
/// whole blob. The alternative — borsh-serialising a plain
/// `std::collections::BTreeMap` (or similar) into the outer private
/// blob — is correct but rewrites the entire serialised value on
/// every mutation; that's fine for small state but expensive at any
/// non-trivial scale.
///
/// # Direct usage (rare)
///
/// You normally don't write `PrivateStorage` directly. The
/// `#[app::private]` macro auto-substitutes it on tree-backed
/// structural collections (`UnorderedMap`, `UnorderedSet`,
/// `Vector`) declared as struct fields, so app code stays unchanged.
/// Direct use is reserved for the rare case where you need an
/// explicit type alias or a collection outside an `#[app::private]`
/// struct that must still go to node-local storage:
///
/// ```ignore
/// use calimero_storage::collections::UnorderedMap;
/// use calimero_storage::store::PrivateStorage;
///
/// // Type alias for use outside `#[app::private]`.
/// type LocalCache = UnorderedMap<String, Vec<u8>, PrivateStorage>;
/// ```
///
/// # What goes here vs MainStorage vs the borsh blob
///
/// | Data shape | Where it lives |
/// |---|---|
/// | Synced app state (`#[app::state]` struct fields) | `MainStorage` — synced via merkle tree |
/// | Local-only structural collections inside `#[app::private]` | `PrivateStorage` — node-local, per-entity granularity |
/// | Local-only primitives + std types (`u64`, `String`, `BTreeMap`, `Vec`) inside `#[app::private]` | The outer private blob — borsh-serialised together, rewritten on every change |
///
/// CRDT collections (`LwwRegister`, `Counter`, `GCounter`,
/// `PNCounter`, `ReplicatedGrowableArray`) are deliberately **not**
/// supported with `PrivateStorage`: CRDTs are multi-writer
/// conflict-resolution machinery, and private storage has exactly
/// one writer (this node), so the per-writer bookkeeping is overhead
/// without a corresponding semantic gain. Use a plain `u64` /
/// `String` / `Vec` instead.
///
/// Access-control collections (`SharedStorage`, `UserStorage`,
/// `FrozenStorage`) and authored collections (`AuthoredMap`,
/// `AuthoredVector`) are likewise excluded: their semantics
/// (cross-writer mutability, per-user separation, immutability,
/// per-entry authorship) all assume the synced tree.
///
/// # Background — the bug this closes
///
/// Before this adaptor existed, tree-backed collections inside
/// `#[app::private]` defaulted to `MainStorage`, so their *entries*
/// silently landed in the synced merkle tree even though the
/// containing struct's blob stayed private. The writing node ended
/// up holding entities the receiving nodes didn't, producing
/// persistent root-hash divergence on every write (see issue #2423).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub struct PrivateStorage;

impl StorageAdaptor for PrivateStorage {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        private_storage_read(key)
    }

    fn storage_remove(key: Key) -> bool {
        private_storage_remove(key)
    }

    fn storage_write(key: Key, value: &[u8]) -> bool {
        private_storage_write(key, value)
    }

    /// Private writes never participate in the synced delta stream.
    ///
    /// See the trait-level doc for the bug this closes (#2319 receiver
    /// gets extra `crdt_type=None` children of context-root every time a
    /// `PrivateStorage`-backed collection is constructed, because
    /// `Interface::save_raw` was unconditionally pushing every storage
    /// mutation onto the global delta queue regardless of adaptor).
    fn participates_in_sync() -> bool {
        false
    }
}

// Note: IterableStorage is only implemented for node's RocksDB backend.
// WASM storage (MainStorage) doesn't support key iteration as the host
// doesn't expose that functionality. This is by design - WASM apps
// shouldn't need to iterate all storage keys.
// This requires adding iteration support to the underlying storage backend (RocksDB, etc.)
//
// impl IterableStorage for MainStorage {
//     fn storage_iter_keys() -> Vec<Key> {
//         // Implement via backend iterator
//     }
// }

#[cfg(any(test, not(target_arch = "wasm32")))]
pub use mocked::MockedStorage;

/// The mocked storage system. Compiled into native builds (gated off
/// `wasm32`) so dependent crates can drive the same in-memory backend
/// from their own tests — see `calimero_node::sync::*_tests`.
#[cfg(any(test, not(target_arch = "wasm32")))]
pub mod mocked {
    use core::cell::{Cell, RefCell};
    use core::ops::Bound;
    use std::collections::BTreeMap;

    use super::{IterableStorage, Key, StorageAdaptor};
    use crate::address::Id;

    /// The scope of the storage system, which allows for multiple storage
    /// systems to be used in parallel.
    type Scope = usize;
    /// Composite key of the mock's ordered index: `(scope, collection, order_key)`.
    type IndexKey = (Scope, Id, Vec<u8>);

    thread_local! {
        pub(crate) static STORAGE: RefCell<BTreeMap<(Scope, Key), Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
        /// Ordered secondary index for the mock: `(scope, collection, order_key) -> entry_id`.
        /// `BTreeMap` iterates in key order, so within a `(scope, collection)`
        /// the entries come back sorted by `order_key` — faithfully modelling the
        /// RocksDB `Column::SortedIndex` the real adaptor uses, including its
        /// seek-based (sub-linear) range/prefix/last behaviour.
        static INDEX: RefCell<BTreeMap<IndexKey, Id>> = const { RefCell::new(BTreeMap::new()) };
        /// Counts index entries *examined* by the most recent ordered query.
        /// Lets tests prove range/page/last touch `O(window)` items, not `O(n)`.
        static INDEX_ITEMS: Cell<usize> = const { Cell::new(0) };
    }

    /// Reset the "items examined" counter (call before an instrumented query).
    pub fn reset_index_items() {
        INDEX_ITEMS.with(|c| c.set(0));
    }

    /// How many index entries the last ordered query examined.
    #[must_use]
    pub fn index_items_examined() -> usize {
        INDEX_ITEMS.with(Cell::get)
    }

    fn bump_examined() {
        INDEX_ITEMS.with(|c| c.set(c.get() + 1));
    }

    /// `true` if `key` is below the end bound `[.., end)`.
    fn within_end(key: &[u8], end: &Bound<Vec<u8>>) -> bool {
        match end {
            Bound::Included(e) => key <= e.as_slice(),
            Bound::Excluded(e) => key < e.as_slice(),
            Bound::Unbounded => true,
        }
    }

    /// The exclusive `BTreeMap` upper bound just past every key of `collection`
    /// (the next collection id), so a `.range(..)` can be a true seek that ends
    /// at the collection boundary. `Unbounded` if `collection` is all-`0xFF`.
    fn collection_upper(scope: Scope, collection: Id) -> Bound<IndexKey> {
        let mut bytes = collection.as_bytes().to_vec();
        for i in (0..bytes.len()).rev() {
            if bytes[i] == 0xFF {
                bytes[i] = 0;
            } else {
                bytes[i] += 1;
                let arr: [u8; 32] = bytes.try_into().expect("32-byte id");
                return Bound::Excluded((scope, Id::new(arr), Vec::new()));
            }
        }
        Bound::Unbounded
    }

    /// In-memory mocked storage backend, scoped by a const generic so
    /// multiple instances can coexist in one test process without
    /// stepping on each other's state.
    ///
    /// # Scope contract
    ///
    /// The `SCOPE` const generic is the **only** thing isolating one
    /// `MockedStorage<N>` from another. Two binaries linking
    /// `calimero-storage` (e.g. a test binary that depends on both
    /// `calimero-storage`'s tests and `calimero-node`'s tests via the
    /// `testing` feature) share the same thread-local `STORAGE`, so a
    /// scope-integer collision silently merges their state.
    ///
    /// Reserved scope ranges, to keep cross-crate usage from colliding:
    ///
    /// - `0..1_000`              — reserved for `calimero-storage`'s own tests.
    /// - `usize::MAX`            — reserved for `DefaultStore` fallback (see `env.rs`).
    /// - everything else         — available to dependent crates.
    ///
    /// New crates pulling in `MockedStorage` should pick a band well
    /// outside the above. Per #2272 review.
    pub struct MockedStorage<const SCOPE: usize>;

    impl<const SCOPE: usize> StorageAdaptor for MockedStorage<SCOPE> {
        fn storage_read(key: Key) -> Option<Vec<u8>> {
            STORAGE.with(|storage| storage.borrow().get(&(SCOPE, key)).cloned())
        }

        fn storage_remove(key: Key) -> bool {
            STORAGE.with(|storage| storage.borrow_mut().remove(&(SCOPE, key)).is_some())
        }

        fn storage_write(key: Key, value: &[u8]) -> bool {
            STORAGE.with(|storage| {
                storage
                    .borrow_mut()
                    .insert((SCOPE, key), value.to_vec())
                    .is_some()
            })
        }

        fn index_supported() -> bool {
            true
        }

        fn index_put(collection: Id, order_key: &[u8], entry: Id) {
            INDEX.with(|index| {
                let _ = index
                    .borrow_mut()
                    .insert((SCOPE, collection, order_key.to_vec()), entry);
            });
        }

        fn index_remove(collection: Id, order_key: &[u8]) {
            INDEX.with(|index| {
                let _ = index
                    .borrow_mut()
                    .remove(&(SCOPE, collection, order_key.to_vec()));
            });
        }

        fn index_clear(collection: Id) {
            INDEX.with(|index| {
                index
                    .borrow_mut()
                    .retain(|(scope, coll, _), _| !(*scope == SCOPE && *coll == collection));
            });
        }

        fn index_range(
            collection: Id,
            start: Bound<Vec<u8>>,
            end: Bound<Vec<u8>>,
            offset: usize,
            limit: Option<usize>,
        ) -> Vec<(Vec<u8>, Id)> {
            // Seek to the start bound (a `BTreeMap::range` is a logarithmic
            // descent), then walk forward, stopping at the collection boundary
            // or the end bound — `take_while` over a lazy range only touches
            // O(seek + items walked), and `skip(offset).take(limit)` bounds that
            // to O(offset + limit). NOT a full scan.
            let lo: Bound<IndexKey> = match start {
                Bound::Included(s) => Bound::Included((SCOPE, collection, s)),
                Bound::Excluded(s) => Bound::Excluded((SCOPE, collection, s)),
                Bound::Unbounded => Bound::Included((SCOPE, collection, Vec::new())),
            };
            INDEX.with(|index| {
                let index = index.borrow();
                let walked = index
                    .range((lo, Bound::Unbounded))
                    .take_while(|((scope, coll, key), _)| {
                        *scope == SCOPE && *coll == collection && within_end(key, &end)
                    })
                    .map(|((_, _, key), entry)| {
                        bump_examined();
                        (key.clone(), *entry)
                    })
                    .skip(offset);
                match limit {
                    Some(n) => walked.take(n).collect(),
                    None => walked.collect(),
                }
            })
        }

        fn index_prefix(
            collection: Id,
            prefix: &[u8],
            offset: usize,
            limit: Option<usize>,
        ) -> Vec<(Vec<u8>, Id)> {
            let lo: Bound<IndexKey> = Bound::Included((SCOPE, collection, prefix.to_vec()));
            INDEX.with(|index| {
                let index = index.borrow();
                let walked = index
                    .range((lo, Bound::Unbounded))
                    .take_while(|((scope, coll, key), _)| {
                        *scope == SCOPE && *coll == collection && key.starts_with(prefix)
                    })
                    .map(|((_, _, key), entry)| {
                        bump_examined();
                        (key.clone(), *entry)
                    })
                    .skip(offset);
                match limit {
                    Some(n) => walked.take(n).collect(),
                    None => walked.collect(),
                }
            })
        }

        fn index_last(collection: Id) -> Option<(Vec<u8>, Id)> {
            let lo: Bound<IndexKey> = Bound::Included((SCOPE, collection, Vec::new()));
            let hi = collection_upper(SCOPE, collection);
            INDEX.with(|index| {
                // `BTreeMap::range` is double-ended, so `next_back()` is a
                // reverse seek to the largest key — O(log n), one item examined.
                index
                    .borrow()
                    .range((lo, hi))
                    .next_back()
                    .map(|((_, _, key), entry)| {
                        bump_examined();
                        (key.clone(), *entry)
                    })
            })
        }
    }

    // MockedStorage supports iteration for testing
    impl<const SCOPE: usize> IterableStorage for MockedStorage<SCOPE> {
        fn storage_iter_keys() -> Vec<Key> {
            STORAGE.with(|storage| {
                storage
                    .borrow()
                    .keys()
                    .filter(|(scope, _)| *scope == SCOPE)
                    .map(|(_, key)| *key)
                    .collect()
            })
        }
    }
}

#[cfg(test)]
mod index_tests {
    use core::ops::Bound;

    use super::mocked::MockedStorage;
    use super::{MainStorage, StorageAdaptor};
    use crate::address::Id;

    // Dedicated mock scope for these tests (within calimero-storage's 0..1_000
    // band). Each test uses a distinct collection id so the shared thread-local
    // index never bleeds across tests.
    type S = MockedStorage<950>;

    fn coll(tag: u8) -> Id {
        Id::new([tag; 32])
    }

    fn entry(tag: u8) -> Id {
        Id::new([0x80 | tag; 32])
    }

    fn keys(pairs: Vec<(Vec<u8>, Id)>) -> Vec<Vec<u8>> {
        pairs.into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn adaptors_advertise_index_support() {
        // The mock backs the ordered index; the default-trait adaptor does not
        // (so a SortedMap over it transparently falls back to its in-memory
        // sort). `MainStorage` gets a real backend in a later stage.
        assert!(S::index_supported());
        assert!(!DefaultIndexAdaptor::index_supported());
    }

    // A bare adaptor that takes every `StorageAdaptor` default — used to pin
    // that `index_supported()` defaults to `false`.
    struct DefaultIndexAdaptor;
    impl StorageAdaptor for DefaultIndexAdaptor {
        fn storage_read(_: super::Key) -> Option<Vec<u8>> {
            None
        }
        fn storage_remove(_: super::Key) -> bool {
            false
        }
        fn storage_write(_: super::Key, _: &[u8]) -> bool {
            false
        }
    }

    #[test]
    fn put_then_range_returns_sorted_pairs() {
        let c = coll(1);
        // Insert out of order.
        S::index_put(c, b"delta", entry(4));
        S::index_put(c, b"alpha", entry(1));
        S::index_put(c, b"charlie", entry(3));
        S::index_put(c, b"bravo", entry(2));

        let all = S::index_range(c, Bound::Unbounded, Bound::Unbounded, 0, None);
        assert_eq!(
            keys(all),
            vec![
                b"alpha".to_vec(),
                b"bravo".to_vec(),
                b"charlie".to_vec(),
                b"delta".to_vec()
            ]
        );
    }

    #[test]
    fn range_respects_bounds() {
        let c = coll(2);
        for k in ["a", "b", "c", "d", "e"] {
            S::index_put(c, k.as_bytes(), entry(0));
        }

        // [b, e)
        let half_open = S::index_range(
            c,
            Bound::Included(b"b".to_vec()),
            Bound::Excluded(b"e".to_vec()),
            0,
            None,
        );
        assert_eq!(
            keys(half_open),
            vec![b"b".to_vec(), b"c".to_vec(), b"d".to_vec()]
        );

        // (b, e]
        let excl_incl = S::index_range(
            c,
            Bound::Excluded(b"b".to_vec()),
            Bound::Included(b"e".to_vec()),
            0,
            None,
        );
        assert_eq!(
            keys(excl_incl),
            vec![b"c".to_vec(), b"d".to_vec(), b"e".to_vec()]
        );
    }

    #[test]
    fn range_offset_and_limit_paginate() {
        let c = coll(3);
        for i in 0..10u8 {
            S::index_put(c, format!("k{i:02}").as_bytes(), entry(i));
        }

        let page = S::index_range(c, Bound::Unbounded, Bound::Unbounded, 3, Some(3));
        assert_eq!(
            keys(page),
            vec![b"k03".to_vec(), b"k04".to_vec(), b"k05".to_vec()]
        );
    }

    #[test]
    fn prefix_scan_matches_only_prefix() {
        let c = coll(4);
        for k in ["user:alice", "user:bob", "post:1", "user:carol", "post:2"] {
            S::index_put(c, k.as_bytes(), entry(0));
        }

        let users = S::index_prefix(c, b"user:", 0, None);
        assert_eq!(
            keys(users),
            vec![
                b"user:alice".to_vec(),
                b"user:bob".to_vec(),
                b"user:carol".to_vec()
            ]
        );
    }

    #[test]
    fn remove_drops_from_index_and_resolves_entry_ids() {
        let c = coll(5);
        S::index_put(c, b"k1", entry(1));
        S::index_put(c, b"k2", entry(2));

        // entry ids resolve correctly
        let pairs = S::index_range(c, Bound::Unbounded, Bound::Unbounded, 0, None);
        assert_eq!(
            pairs,
            vec![(b"k1".to_vec(), entry(1)), (b"k2".to_vec(), entry(2))]
        );

        S::index_remove(c, b"k1");
        let after = S::index_range(c, Bound::Unbounded, Bound::Unbounded, 0, None);
        assert_eq!(keys(after), vec![b"k2".to_vec()]);
    }

    // `MainStorage`'s index methods build composite `collection ‖ order_key`
    // keys and translate range bounds to byte bounds, routing through the env
    // layer (here the native in-memory ordered mock). This pins that
    // composite/bound/decode logic — the same code path the real host ABI and
    // RocksDB `SortedIndex` column will exercise once `index_supported()` flips.
    #[test]
    #[serial_test::serial]
    fn main_storage_index_routes_through_env() {
        crate::env::reset_for_testing();
        let c = Id::new([200u8; 32]);

        MainStorage::index_put(c, b"charlie", Id::new([3; 32]));
        MainStorage::index_put(c, b"alpha", Id::new([1; 32]));
        MainStorage::index_put(c, b"bravo", Id::new([2; 32]));

        // Full scan resolves order keys and entry ids, ascending.
        let all = MainStorage::index_range(c, Bound::Unbounded, Bound::Unbounded, 0, None);
        assert_eq!(
            all,
            vec![
                (b"alpha".to_vec(), Id::new([1; 32])),
                (b"bravo".to_vec(), Id::new([2; 32])),
                (b"charlie".to_vec(), Id::new([3; 32])),
            ]
        );

        // Half-open range [alpha, charlie).
        let r = MainStorage::index_range(
            c,
            Bound::Included(b"alpha".to_vec()),
            Bound::Excluded(b"charlie".to_vec()),
            0,
            None,
        );
        assert_eq!(keys(r), vec![b"alpha".to_vec(), b"bravo".to_vec()]);

        // Prefix scan must not bleed into other keys.
        MainStorage::index_put(c, b"al", Id::new([9; 32]));
        let pre = MainStorage::index_prefix(c, b"al", 0, None);
        assert_eq!(keys(pre), vec![b"al".to_vec(), b"alpha".to_vec()]);

        // Clear drops the whole collection's index.
        MainStorage::index_clear(c);
        assert!(
            MainStorage::index_range(c, Bound::Unbounded, Bound::Unbounded, 0, None).is_empty()
        );
    }

    #[test]
    fn index_is_isolated_per_collection() {
        let a = coll(6);
        let b = coll(7);
        S::index_put(a, b"x", entry(1));
        S::index_put(b, b"y", entry(2));

        assert_eq!(
            keys(S::index_range(
                a,
                Bound::Unbounded,
                Bound::Unbounded,
                0,
                None
            )),
            vec![b"x".to_vec()]
        );
        assert_eq!(
            keys(S::index_range(
                b,
                Bound::Unbounded,
                Bound::Unbounded,
                0,
                None
            )),
            vec![b"y".to_vec()]
        );
    }

    // Empirical proof of the documented Big-O: build a large collection and
    // assert (via the mock's "items examined" counter, which faithfully models
    // the RocksDB seek) that bounded reads touch O(window) index entries, not
    // O(n). A full scan, by contrast, touches all n — the control.
    #[test]
    #[serial_test::serial]
    fn ordered_reads_are_sublinear() {
        use super::mocked;

        let c = coll(42);
        let n = 500usize;
        for i in 0..n {
            S::index_put(c, format!("k{i:04}").as_bytes(), entry(0));
        }

        // last → a single reverse-seek item.
        mocked::reset_index_items();
        assert!(S::index_last(c).is_some());
        assert_eq!(
            mocked::index_items_examined(),
            1,
            "last must examine 1 item, not n"
        );

        // page(0, 10) → 10 items.
        mocked::reset_index_items();
        let page = S::index_range(c, Bound::Unbounded, Bound::Unbounded, 0, Some(10));
        assert_eq!(page.len(), 10);
        assert_eq!(
            mocked::index_items_examined(),
            10,
            "page(0,10) must examine 10 items, not n"
        );

        // range [k0100, k0105) → 5 items (a tight window).
        mocked::reset_index_items();
        let window = S::index_range(
            c,
            Bound::Included(b"k0100".to_vec()),
            Bound::Excluded(b"k0105".to_vec()),
            0,
            None,
        );
        assert_eq!(window.len(), 5);
        assert_eq!(
            mocked::index_items_examined(),
            5,
            "range window must examine only the matches, not n"
        );

        // Control: a full scan examines all n — confirming the counter is real.
        mocked::reset_index_items();
        let all = S::index_range(c, Bound::Unbounded, Bound::Unbounded, 0, None);
        assert_eq!(all.len(), n);
        assert_eq!(
            mocked::index_items_examined(),
            n,
            "full scan examines all n (the counter isn't trivially small)"
        );
    }
}
