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
    use core::cell::RefCell;
    use std::collections::BTreeMap;

    use super::{IterableStorage, Key, StorageAdaptor};

    /// The scope of the storage system, which allows for multiple storage
    /// systems to be used in parallel.
    type Scope = usize;

    thread_local! {
        pub(crate) static STORAGE: RefCell<BTreeMap<(Scope, Key), Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
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
