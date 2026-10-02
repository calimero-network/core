//! Indexing system for efficient tree navigation.

#[cfg(test)]
#[path = "tests/index.rs"]
mod tests;

use core::any::TypeId;
use core::cell::RefCell;
use core::marker::PhantomData;
use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use tracing::info;

use crate::address::Id;
use crate::child_trie::{self, ChildTrie};
use crate::entities::{ChildInfo, EntryRules, Metadata, StorageType, UpdatedAt};
use crate::hash_meter::{Digest, Sha256};
use crate::interface::StorageError;
use crate::store::{Key, StorageAdaptor};

pub(crate) const MAX_PARENT_CHAIN: usize = 256; // most ancestors an entity may have

// Deferred ancestor recomputation (#2238).
//
// `recalculate_ancestor_hashes_for` walks from a given node up to root,
// re-reading and re-hashing each ancestor. During a merge (e.g. `Root::sync`
// applying 20 deltas to the same parent), the per-action call pattern runs
// this walk 20 times with ~identical starts, redoing the same O(K) hash
// work at every parent along the path. Batching collapses that to one walk
// per unique starting id per merge.
//
// Mechanism: a thread-local slot holding `Option<(TypeId, BTreeSet<Id>)>`.
// The `TypeId` identifies the `StorageAdaptor` the active scope was opened
// against. When `recalculate_ancestor_hashes_for<Index<S>>` is called it
// only defers if the slot's `TypeId` matches `S`'s — calls against a
// different adaptor (e.g. `Index<MockedStorage<1>>` while a
// `DeferredAncestorScope<MockedStorage<2>>` is active) bypass the defer and
// walk immediately, so a flush never runs against the wrong backend.
//
// The scope does not cross `.await` boundaries. `Root::sync` is synchronous
// from the WASM side, so one scope per merge is sufficient.
thread_local! {
    static DEFERRED_ANCESTORS: RefCell<Option<(TypeId, BTreeSet<Id>)>> =
        const { RefCell::new(None) };
}

// Cross-thread serialization for index read-modify-write (core#2571).
//
// The Merkle index mutators below (`add_child_to`, the hash recomputes, the
// child-removal path) read an index entry, mutate it in memory, then write it
// back. On the native node a local write (the execute path, under
// `context.lock()`) and a sync apply (the dedicated `SyncSessionActor`, #2316)
// run on DIFFERENT threads with no shared lock — so two concurrent
// read-modify-writes on the same parent index entry lose one update. The lost
// update drops a child from a collection's `children` list while that child's
// `Key::Entry` still exists; the collection's `full_hash` then diverges from
// every peer's and HashComparison can never reconcile it (the same-DAG-heads /
// different-root split-brain that loops `wait_for_sync` to timeout).
//
// Serializing the read-modify-writes makes each one atomic: a caller always
// reads the CURRENT list and appends its child, so no concurrent add is lost
// regardless of interleave. The guard is reentrant — a single logical operation
// nests these calls on one thread — and native-only: WASM app execution is
// single-threaded, so there is no race and the guard compiles out to nothing.
//
// Why ONE global lock rather than a per-entity/per-context lock: it is
// deadlock-free by construction (a single lock has no acquisition order to
// invert), whereas locking individual entries would let two threads grab a
// parent and an ancestor in opposite orders during the ancestor-hash walk and
// deadlock. Index mutations are microsecond-scale in-memory + one store write,
// so global serialization is cheap; a per-context lock keyed by the
// `RuntimeEnv` context is a safe future optimization if contention ever shows.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod mutation_lock {
    use std::cell::Cell;
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());
    thread_local! {
        static DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    /// Held for the duration of an index read-modify-write. The global lock is
    /// released only when the OUTERMOST guard on this thread drops, so nested
    /// mutators (e.g. `add_child_to` → `recalculate_ancestor_hashes_for`) don't
    /// self-deadlock.
    ///
    /// Like a `MutexGuard`, this must reach its `Drop`. `std::mem::forget`-ing
    /// it leaks the held lock AND leaves this thread's reentrancy `DEPTH`
    /// elevated forever, so the thread would never re-acquire the lock and
    /// concurrent mutations from other threads would stop being serialized.
    /// Don't `mem::forget` it (same hazard as `DeferredAncestorScope`).
    #[must_use]
    pub(crate) struct Guard(#[allow(dead_code)] Option<MutexGuard<'static, ()>>);

    pub(crate) fn guard() -> Guard {
        DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            if current == 0 {
                // Recover from poisoning rather than propagate. The lock only
                // SERIALIZES access — it adds no transactionality, and the
                // storage layer has none either, so a panic mid-mutation can
                // leave the index partially updated whether or not this lock
                // exists (that is the pre-existing, lock-free behaviour). What
                // recovery buys is that one thread's panic doesn't permanently
                // brick every other thread's index writes; surfacing the panic
                // is the panicking caller's job, not this guard's.
                Guard(Some(
                    LOCK.lock().unwrap_or_else(|poison| poison.into_inner()),
                ))
            } else {
                Guard(None)
            }
        })
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // Release the lock (outermost guard only) BEFORE clearing depth, so
            // the "DEPTH > 0 ⇒ this thread holds the lock" invariant never
            // momentarily inverts.
            drop(self.0.take());
            DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
}

/// Acquire the reentrant index-mutation guard. Native serializes concurrent
/// read-modify-writes; WASM is single-threaded so this is a no-op.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn index_mutation_guard() -> mutation_lock::Guard {
    mutation_lock::guard()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn index_mutation_guard() {}

/// RAII scope that defers ancestor-hash recomputation for a specific
/// `StorageAdaptor`.
///
/// While this guard is alive, calls to `Index::<S>::recalculate_ancestor_hashes_for`
/// record the starting id into a thread-local dirty-set instead of walking
/// immediately. Walks are flushed either by [`DeferredAncestorScope::finish`]
/// (propagates errors) or by `Drop` as a fail-safe (logs errors).
///
/// # Callers that care about errors
///
/// Prefer `finish()?` at end of scope so storage failures during flush
/// propagate to the caller rather than being silently logged. `Drop` is a
/// best-effort safety net for exception / panic paths.
///
/// # Nesting
///
/// Only the outermost scope owns the dirty-set; inner scopes of the same
/// adaptor type are no-ops (their additions participate in the outer
/// flush). Scopes of different adaptor types on the same thread log a
/// warning and the inner becomes a no-op — mixing adaptors under a single
/// scope has no well-defined semantics, production code never exercises this.
///
/// # `std::mem::forget` is unsafe in practice
///
/// Forgetting a scope without calling `finish()` or dropping leaves the
/// thread-local dirty-set populated indefinitely. Every subsequent
/// `recalculate_ancestor_hashes_for` on that thread for the same adaptor
/// would silently defer with no flush, producing a stale merkle tree.
/// This is safe Rust but incorrect — the API assumes the scope's lifetime
/// ends before the thread returns to user code. Treat like `MutexGuard`:
/// do not `mem::forget` it.
///
/// # Visibility
///
/// `pub(crate)` because misuse (opening a scope without corresponding
/// storage operations, or losing it via `mem::forget`) can corrupt the
/// merkle tree. Internal code paths (currently `Root::sync`) own the
/// invariants around its use.
#[must_use = "the scope must be held to defer recomputation; finish() it or let it drop to flush"]
pub(crate) struct DeferredAncestorScope<S: StorageAdaptor> {
    /// True iff this scope created the thread-local dirty-set.
    is_outermost: bool,
    /// True iff the caller already called `finish()`; Drop becomes a no-op.
    flushed: bool,
    _phantom: PhantomData<S>,
}

impl<S: StorageAdaptor> DeferredAncestorScope<S> {
    /// Begin deferring ancestor recomputation. Flush via `finish()` (for
    /// error-propagating flow) or let the scope drop (fail-safe).
    pub(crate) fn new() -> Self {
        let type_id = TypeId::of::<S>();
        let is_outermost = DEFERRED_ANCESTORS.with(|slot| {
            let mut borrowed = slot.borrow_mut();
            match borrowed.as_ref() {
                Some((existing, _)) if *existing == type_id => false,
                Some((existing, _)) => {
                    tracing::warn!(
                        target: "storage::merkle",
                        ?existing,
                        new_type = ?type_id,
                        "nested DeferredAncestorScope with a different StorageAdaptor; \
                         inner scope is a no-op"
                    );
                    false
                }
                None => {
                    *borrowed = Some((type_id, BTreeSet::new()));
                    true
                }
            }
        });
        Self {
            is_outermost,
            flushed: false,
            _phantom: PhantomData,
        }
    }

    /// Flush deferred ancestor walks, propagating any storage error.
    ///
    /// After calling this, `Drop` is a no-op. Prefer this over relying on
    /// `Drop` for call sites that want storage errors to surface (e.g.
    /// `Root::sync` — an error here means the merkle tree is inconsistent
    /// and the caller should be told).
    ///
    /// # Errors
    ///
    /// Returns `StorageError` on the first failed ancestor walk. Any dirty
    /// entries not yet processed are left in the thread-local set so the
    /// `Drop` fail-safe can retry them after the caller's error unwinds.
    /// If the `Drop` path also fails, the set is cleared to avoid leaking
    /// into subsequent operations on the same thread.
    pub(crate) fn finish(mut self) -> Result<(), StorageError> {
        self.flushed = true;
        if !self.is_outermost {
            return Ok(());
        }
        Self::flush_impl()
    }

    /// Drains the thread-local dirty-set one entry at a time, running the
    /// real ancestor walk for each. On error, the remaining entries stay in
    /// the set (available for a `Drop`-time retry); on the Drop path the
    /// caller is expected to clear what's left.
    fn flush_impl() -> Result<(), StorageError> {
        let type_id = TypeId::of::<S>();
        loop {
            // Pop one id at a time. Keep the BTreeSet (and TypeId tag) in
            // place; only remove the tag entirely when the set is empty.
            let next_id = DEFERRED_ANCESTORS.with(|slot| {
                let mut borrowed = slot.borrow_mut();
                match borrowed.as_mut() {
                    Some((existing, set)) if *existing == type_id => {
                        let next = set.pop_first();
                        if set.is_empty() {
                            *borrowed = None;
                        }
                        next
                    }
                    _ => None,
                }
            });
            let Some(id) = next_id else {
                return Ok(());
            };
            <Index<S>>::recalculate_ancestor_hashes_for_now(id)?;
        }
    }

    /// Drops any dirty entries remaining in the thread-local after a failed
    /// flush. Called from the `Drop` path when the fail-safe flush itself
    /// errors, so a stuck set can't leak into the next operation on this
    /// thread. Any entries discarded here were never flushed — the caller
    /// has already logged the underlying error.
    fn discard_dirty_set() {
        let type_id = TypeId::of::<S>();
        DEFERRED_ANCESTORS.with(|slot| {
            let mut borrowed = slot.borrow_mut();
            let matches = matches!(borrowed.as_ref(), Some((t, _)) if *t == type_id);
            if matches {
                *borrowed = None;
            }
        });
    }
}

impl<S: StorageAdaptor> Drop for DeferredAncestorScope<S> {
    fn drop(&mut self) {
        // Explicit finish() was called — nothing to do.
        if self.flushed {
            return;
        }
        // Non-outermost scopes don't own the dirty-set; outer drops later.
        if !self.is_outermost {
            return;
        }
        // Fail-safe flush. Errors are logged because Drop can't return a
        // Result. This path is typically reached when the caller returned
        // `?` from inside the scope and never got to `finish()`, so the
        // merkle tree is already in an uncertain state from the caller's
        // original error. We still attempt to flush remaining entries,
        // and if the flush itself fails, we discard the rest to avoid
        // leaking the dirty set into the next operation on this thread.
        if let Err(err) = Self::flush_impl() {
            tracing::error!(
                target: "storage::merkle",
                ?err,
                "deferred ancestor recompute failed during Drop; merkle tree may be inconsistent. \
                 Prefer .finish()? over Drop for error propagation."
            );
            Self::discard_dirty_set();
        }
    }
}

/// Where the seal of a delete of the written-once entry at `id`, under
/// `rules`, is kept: see [`Index::seal_written_once`].
fn seal_id(id: Id, rules: &EntryRules) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(b"calimero.storage.written-once-seal");
    hasher.update(id.as_bytes());
    hasher.update([u8::from(rules.immutable)]);
    match rules.moderators {
        Some(anchor) => {
            hasher.update([1]);
            hasher.update(anchor.as_bytes());
        }
        None => hasher.update([0]),
    }
    Id::new(hasher.finalize().into())
}

/// Index entry for an entity.
///
/// New fields must use borsh-CANONICAL types (no `HashMap`/`HashSet` or other
/// nondeterministically-ordered containers): the node's tombstone GC tells an
/// index row apart from opaque entity data by re-serializing a decoded value and
/// requiring byte-identical output. A non-canonical field would silently break
/// that guard. `calimero_node::gc`'s `entity_index_borsh_roundtrips` test locks
/// the invariant and must stay green.
///
/// Serialized by hand (see the impls below) so that `full_hash` is stored only
/// when it cannot be derived: for an entity with no children it is
/// `H(own_hash)`, which is most entities.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntityIndex {
    /// Entity ID.
    id: Id,

    /// Parent ID.
    parent_id: Option<Id>,

    /// Full hash (entity + descendants).
    full_hash: [u8; 32],

    /// Own hash (entity only).
    own_hash: [u8; 32],

    /// Entity metadata.
    pub metadata: Metadata,

    /// Tombstone marker. When set, entity data is deleted but index kept for CRDT sync.
    /// Enables proper conflict resolution (delete vs update) in distributed scenarios.
    /// Garbage collected after retention period (default: 1 day).
    pub deleted_at: Option<u64>,

    /// Ids of this entity's children that have been removed (tombstoned).
    ///
    /// A deletion is the *absence* of a child from `children`, which the
    /// add-biased HC/LevelWise comparison can't see. This list makes the
    /// deletion explicit so it can be carried on the sync wire
    /// (`TreeNode.deleted_children`): a peer that still holds the child learns
    /// of the deletion during comparison and applies delete-wins — without
    /// anyone pushing the live entity. The child's `deleted_at` + signed
    /// metadata are read from the child's own tombstone index at wire-build
    /// time, so only the id is kept here, and an id whose tombstone
    /// GC has collected is dropped by the same sweep (`crate::reclaim`).
    pub deleted_children: Vec<Id>,
}

/// The full hash of an entity with no children: the fold over an empty trie,
/// as `Index::full_hash_from_root` computes it.
fn childless_full_hash(own_hash: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(own_hash).into()
}

/// `full_hash` follows `own_hash` behind a one-byte tag: `0` when it equals
/// [`childless_full_hash`] and is omitted, `1` when the 32 bytes follow. The
/// encoding stays canonical, which tombstone GC relies on (it recognises an
/// index row by a byte-exact re-serialization): decoding refuses the one form
/// the encoder never produces, an explicit hash that could have been omitted.
impl BorshSerialize for EntityIndex {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        self.id.serialize(writer)?;
        self.parent_id.serialize(writer)?;
        self.own_hash.serialize(writer)?;
        if self.full_hash == childless_full_hash(&self.own_hash) {
            0_u8.serialize(writer)?;
        } else {
            1_u8.serialize(writer)?;
            self.full_hash.serialize(writer)?;
        }
        self.metadata.serialize(writer)?;
        self.deleted_at.serialize(writer)?;
        self.deleted_children.serialize(writer)
    }
}

impl BorshDeserialize for EntityIndex {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let id = Id::deserialize_reader(reader)?;
        let parent_id = Option::<Id>::deserialize_reader(reader)?;
        let own_hash = <[u8; 32]>::deserialize_reader(reader)?;
        let derived = childless_full_hash(&own_hash);
        let full_hash = match u8::deserialize_reader(reader)? {
            0 => derived,
            1 => {
                let full_hash = <[u8; 32]>::deserialize_reader(reader)?;
                if full_hash == derived {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "non-canonical index row: a derivable full hash stored explicitly",
                    ));
                }
                full_hash
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid full-hash tag in index row",
                ))
            }
        };
        Ok(Self {
            id,
            parent_id,
            full_hash,
            own_hash,
            metadata: Metadata::deserialize_reader(reader)?,
            deleted_at: Option::<u64>::deserialize_reader(reader)?,
            deleted_children: Vec::<Id>::deserialize_reader(reader)?,
        })
    }
}

// The slim index's flags byte, after the id: which optional fields follow.
const SLIM_PARENT: u8 = 0x01;
const SLIM_FULL: u8 = 0x02;
const SLIM_DELETED: u8 = 0x04;
const SLIM_DELETED_CHILDREN: u8 = 0x08;
/// `own_hash` is all zero and not stored: a tombstone's, whose data is gone.
const SLIM_OWN_ZERO: u8 = 0x10;
/// `deleted_at` is set and equals `metadata.updated_at`, so it is not stored:
/// a delete raises `updated_at` to its own stamp (`Index::mark_deleted`).
const SLIM_DELETED_AT_UPDATED: u8 = 0x20;
const SLIM_KNOWN: u8 = SLIM_PARENT
    | SLIM_FULL
    | SLIM_DELETED
    | SLIM_DELETED_CHILDREN
    | SLIM_OWN_ZERO
    | SLIM_DELETED_AT_UPDATED;

/// An [`EntityIndex`] decoded from an entity row (see [`crate::row`]) whose
/// `own_hash` may still have to come from the row's data.
///
/// ```text
/// slim = flags(1) ‖ [parent_id] ‖ [own_hash] ‖ [full_hash]
///        ‖ metadata ‖ [deleted_at] ‖ [varint n ‖ n deleted children]
/// ```
///
/// The id is not stored: the row's key names it, and the reader passes it in.
/// Each optional field is present exactly when its flag is set, except
/// `own_hash`, which is omitted when the row derives it from its data or when
/// it is all zero (`SLIM_OWN_ZERO`), and `deleted_at`, which is omitted when it
/// equals `metadata.updated_at` (`SLIM_DELETED_AT_UPDATED`) — as on every
/// tombstone. An omitted `full_hash` means `childless_full_hash(own_hash)`, so
/// it too can only be resolved once `own_hash` is known — hence the two-step
/// decode.
#[derive(Debug)]
pub(crate) struct SlimIndex {
    index: EntityIndex,
    own_derived: bool,
    full_hash: Option<[u8; 32]>,
}

impl SlimIndex {
    pub(crate) fn deserialize(
        reader: &mut &[u8],
        own_derived: bool,
        id: Id,
    ) -> std::io::Result<Self> {
        let invalid =
            |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_owned());
        let flags = u8::deserialize_reader(reader)?;
        if flags & !SLIM_KNOWN != 0 {
            return Err(invalid("unknown flags in index row"));
        }
        let parent_id = (flags & SLIM_PARENT != 0)
            .then(|| Id::deserialize_reader(reader))
            .transpose()?;
        let own_zero = flags & SLIM_OWN_ZERO != 0;
        if own_zero && own_derived {
            return Err(invalid("a derived own hash marked zero in index row"));
        }
        let own_hash = if own_derived || own_zero {
            [0; 32]
        } else {
            let own_hash = <[u8; 32]>::deserialize_reader(reader)?;
            if own_hash == [0; 32] {
                return Err(invalid("a zero own hash stored explicitly in index row"));
            }
            own_hash
        };
        let full_hash = (flags & SLIM_FULL != 0)
            .then(|| <[u8; 32]>::deserialize_reader(reader))
            .transpose()?;
        let metadata = Metadata::deserialize_reader(reader)?;
        let deleted_at = match (
            flags & SLIM_DELETED != 0,
            flags & SLIM_DELETED_AT_UPDATED != 0,
        ) {
            (false, false) => None,
            (false, true) => Some(*metadata.updated_at),
            (true, false) => {
                let deleted_at = u64::deserialize_reader(reader)?;
                if deleted_at == *metadata.updated_at {
                    return Err(invalid(
                        "a deleted_at equal to updated_at stored in index row",
                    ));
                }
                Some(deleted_at)
            }
            (true, true) => return Err(invalid("deleted_at both stored and implied in index row")),
        };
        let deleted_children = if flags & SLIM_DELETED_CHILDREN != 0 {
            let count = calimero_prelude::row::take_varint(reader)
                .and_then(|count| usize::try_from(count).ok())
                .filter(|&count| count > 0)
                .ok_or_else(|| invalid("invalid deleted-children count in index row"))?;
            if reader.len() / 32 < count {
                return Err(invalid("deleted children overrun the index row"));
            }
            (0..count)
                .map(|_| Id::deserialize_reader(reader))
                .collect::<std::io::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Ok(Self {
            index: EntityIndex {
                id,
                parent_id,
                full_hash: [0; 32],
                own_hash,
                metadata,
                deleted_at,
                deleted_children,
            },
            own_derived,
            full_hash,
        })
    }

    /// Completes the decode with `Sha256` of the row's data, if it has any.
    /// `None` for a non-canonical row: a derived hash with no data to derive
    /// it from, or an explicit one that could have been derived.
    pub(crate) fn finish(self, data_hash: Option<[u8; 32]>) -> Option<EntityIndex> {
        let Self {
            mut index,
            own_derived,
            full_hash,
        } = self;
        if own_derived {
            index.own_hash = data_hash?;
        } else if data_hash == Some(index.own_hash) {
            return None;
        }
        let derived_full = childless_full_hash(&index.own_hash);
        index.full_hash = match full_hash {
            None => derived_full,
            Some(explicit) if explicit == derived_full => return None,
            Some(explicit) => explicit,
        };
        Some(index)
    }
}

impl EntityIndex {
    /// The slim form [`SlimIndex`] reads: no id, and `own_hash` left out when
    /// the row derives it from its data.
    pub(crate) fn serialize_slim<W: std::io::Write>(
        &self,
        writer: &mut W,
        own_derived: bool,
    ) -> std::io::Result<()> {
        let explicit_full = self.full_hash != childless_full_hash(&self.own_hash);
        let own_zero = !own_derived && self.own_hash == [0; 32];
        let deleted_at = self
            .deleted_at
            .filter(|deleted_at| *deleted_at != *self.metadata.updated_at);
        let mut flags = 0_u8;
        if self.parent_id.is_some() {
            flags |= SLIM_PARENT;
        }
        if explicit_full {
            flags |= SLIM_FULL;
        }
        match (self.deleted_at, deleted_at) {
            (Some(_), Some(_)) => flags |= SLIM_DELETED,
            (Some(_), None) => flags |= SLIM_DELETED_AT_UPDATED,
            (None, _) => {}
        }
        if !self.deleted_children.is_empty() {
            flags |= SLIM_DELETED_CHILDREN;
        }
        if own_zero {
            flags |= SLIM_OWN_ZERO;
        }
        flags.serialize(writer)?;
        if let Some(parent_id) = &self.parent_id {
            parent_id.serialize(writer)?;
        }
        if !own_derived && !own_zero {
            self.own_hash.serialize(writer)?;
        }
        if explicit_full {
            self.full_hash.serialize(writer)?;
        }
        self.metadata.serialize(writer)?;
        if let Some(deleted_at) = deleted_at {
            deleted_at.serialize(writer)?;
        }
        if !self.deleted_children.is_empty() {
            let mut count = Vec::new();
            calimero_prelude::row::put_varint(&mut count, self.deleted_children.len() as u64);
            writer.write_all(&count)?;
            for child in &self.deleted_children {
                child.serialize(writer)?;
            }
        }
        Ok(())
    }

    /// Sets `own_hash`, keeping `full_hash` childless-consistent when it was.
    #[cfg(test)]
    pub(crate) fn set_own_hash(&mut self, own_hash: [u8; 32]) {
        let childless = self.full_hash == childless_full_hash(&self.own_hash);
        self.own_hash = own_hash;
        if childless {
            self.full_hash = childless_full_hash(&own_hash);
        }
    }

    /// Builds a minimal index carrying just an id, for tests in
    /// downstream crates that need a borsh-serializable `EntityIndex`
    /// (e.g. snapshot generation, which discovers entities by
    /// deserializing stored Index values). Hidden because the fields
    /// are otherwise private and this is not part of the stable API.
    #[doc(hidden)]
    #[must_use]
    pub fn minimal_for_test(id: Id) -> Self {
        Self {
            id,
            parent_id: None,
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: Metadata::default(),
            deleted_at: None,
            deleted_children: Vec::new(),
        }
    }

    /// Like [`Self::minimal_for_test`] but with a chosen `full_hash`.
    ///
    /// A context's ROOT index carries the hash that
    /// `ContextRegistry::compute_root_hash` reads, so a test that needs the
    /// state-derived root to *move* has to be able to set it.
    #[must_use]
    pub fn minimal_for_test_with_full_hash(id: Id, full_hash: [u8; 32]) -> Self {
        Self {
            full_hash,
            ..Self::minimal_for_test(id)
        }
    }

    /// Like [`Self::minimal_for_test`] but parented, and with a chosen
    /// `full_hash`.
    ///
    /// Snapshot install writes index rows verbatim, so a test for the
    /// receive-side trie rebuild has to be able to produce a row that names its
    /// parent — that link is the only thing the rebuild has to work from.
    #[doc(hidden)]
    #[must_use]
    pub fn minimal_for_test_with_parent(id: Id, parent_id: Id, full_hash: [u8; 32]) -> Self {
        Self {
            parent_id: Some(parent_id),
            full_hash,
            ..Self::minimal_for_test(id)
        }
    }

    /// Whether this row records the deletion of a written-once entry: its own
    /// tombstone, or the seal a delete leaves when it reaches a node before the
    /// entry does (`Index::seal_written_once`).
    ///
    /// That deletion is terminal, so the row is the lasting record that keeps
    /// the owner's key deleted. Tombstone GC must never collect it.
    #[must_use]
    pub fn is_terminal_tombstone(&self) -> bool {
        self.deleted_at.is_some()
            && matches!(
                self.metadata.storage_type,
                StorageType::User { rules, .. } if rules.immutable
            )
    }

    /// Returns the entity ID.
    #[must_use]
    pub fn id(&self) -> Id {
        self.id
    }

    /// Returns the parent ID, if any.
    #[must_use]
    pub fn parent_id(&self) -> Option<Id> {
        self.parent_id
    }

    /// Ids of this entity's children that have been removed (tombstoned).
    ///
    /// Carried on the sync wire so a peer that still holds a child converges
    /// to the deletion (delete-wins) without anyone pushing the live entity.
    #[must_use]
    pub fn deleted_children(&self) -> &[Id] {
        &self.deleted_children
    }

    /// Returns the full hash (entity + all descendants).
    #[must_use]
    pub fn full_hash(&self) -> [u8; 32] {
        self.full_hash
    }

    /// Returns the own hash (entity data only).
    #[must_use]
    pub fn own_hash(&self) -> [u8; 32] {
        self.own_hash
    }
}

/// An entity's index and, if it has any, its data, read as one row.
type IndexWithValue = (EntityIndex, Option<Vec<u8>>);

/// Entity index manager.
#[derive(Debug)]
pub struct Index<S: StorageAdaptor>(PhantomData<S>);

impl<S: StorageAdaptor> Index<S> {
    /// Adds a child to a parent's collection.
    /// Write `child`'s own index record, pointing it at `parent_id`.
    ///
    /// This is the half of child registration that costs O(1): it touches one
    /// record, the child's. It does not list the child among the parent's
    /// `children`, and so does not refold the parent's hash over its siblings.
    /// Returns the child's recomputed `full_hash`, which the caller needs when
    /// listing the child in its parent.
    ///
    /// With `value`, the child's data is written in the same row write.
    fn write_child_index(
        parent_id: Id,
        child: &ChildInfo,
        value: Option<&[u8]>,
    ) -> Result<[u8; 32], StorageError> {
        let mut child_index = Self::get_index(child.id())?.unwrap_or_else(|| EntityIndex {
            id: child.id(),
            parent_id: None,
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: child.metadata.clone(),
            deleted_at: None,
            deleted_children: Vec::new(),
        });
        child_index.parent_id = Some(parent_id);
        child_index.own_hash = child.merkle_hash();
        child_index.full_hash = Self::full_hash_from_trie(child.id(), child_index.own_hash);
        child_index.deleted_at = None;
        let full_hash = child_index.full_hash;
        Self::save_index_keeping(&child_index, value)?;
        Ok(full_hash)
    }

    pub(crate) fn add_child_to(parent_id: Id, child: ChildInfo) -> Result<(), StorageError> {
        Self::add_child_with_value_to(parent_id, child, None)
    }

    /// [`add_child_to`](Self::add_child_to), writing the child's data as well
    /// when `value` is given: in the same row write as its index, before the
    /// parent lists it, so the parent never advertises a child with no data.
    pub(crate) fn add_child_with_value_to(
        parent_id: Id,
        child: ChildInfo,
        value: Option<&[u8]>,
    ) -> Result<(), StorageError> {
        let added_child_id = child.id();
        // Serialize the read-modify-write so a concurrent local-write / sync
        // apply on the same parent can't lose a child (core#2571).
        let _mutation_guard = index_mutation_guard();

        // Get or create parent index
        let (mut parent_index, parent_value) = Self::get_index_with_value(parent_id)?
            .unwrap_or_else(|| {
                let index = EntityIndex {
                    id: parent_id,
                    parent_id: None,
                    full_hash: [0; 32],
                    own_hash: [0; 32],
                    metadata: Metadata::default(),
                    deleted_at: None,
                    deleted_children: Vec::new(),
                };
                (index, None)
            });

        // Adding a child means it is live: clear any tombstone, else find_by_id
        // hides an entity the parent hash now counts (upsert-on-tombstone
        // divergence). Pairs with the deleted_children.retain below.
        let child_full_hash = Self::write_child_index(parent_id, &child, value)?;

        // Link through the parent's child trie. The list this replaces was one
        // inline blob: adding child N read N, wrote N+1 and re-hashed all of
        // them, so a write cost grew with history until it exhausted the gas
        // limit (core#3602). A trie link touches a bounded number of rows
        // whatever the parent holds.
        let new_entry = ChildInfo::new(child.id(), child_full_hash, child.metadata);
        let trie_root = <ChildTrie<S>>::new(parent_id).insert(new_entry);

        // A re-added (resurrected) child is live again, so it must no longer be
        // advertised as deleted on the wire — else a peer would apply a stale
        // delete-wins against it.
        parent_index
            .deleted_children
            .retain(|id| *id != added_child_id);

        let stored_full_hash = parent_index.full_hash;
        parent_index.full_hash = Self::full_hash_from_root(parent_index.own_hash, trie_root);
        // Nothing above writes the parent's own row, so the data read with its
        // index is still what it holds.
        Self::save_index_keeping(&parent_index, parent_value.as_deref())?;

        // A re-link under the hash the child already had leaves the parent's
        // hash where it was, and with it every ancestor's (see `write_value_for`).
        if parent_index.full_hash != stored_full_hash {
            Self::recalculate_ancestor_hashes_for(parent_id)?;
        }
        Ok(())
    }

    /// Adds a root entity (entity without a parent).
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or saved.
    pub fn add_root(root: ChildInfo) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();
        let mut index = Self::get_index(root.id())?.unwrap_or_else(|| EntityIndex {
            id: root.id(),
            parent_id: None,
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: root.metadata.clone(),
            deleted_at: None,
            deleted_children: Vec::new(),
        });
        index.own_hash = root.merkle_hash();
        // Keep stored full_hash current. Without this, a root holds
        // full_hash = [0; 32] until some later operation triggers
        // recomputation — callers that read the current hash would have
        // to re-hash the children Vec every time. With the stored value
        // kept current by each write site, reads become O(1).
        // See #2238 Fix 1.
        index.full_hash = Self::full_hash_from_trie(index.id, index.own_hash);
        Self::save_index(&index)?;
        Ok(())
    }

    /// An entity's full hash, folding its children through the child trie.
    ///
    /// Replaces folding a sibling list: the trie root already commits to every
    /// child, and maintaining it costs a bounded number of rows per link rather
    /// than rewriting and re-hashing every sibling. See [`ChildTrie`].
    ///
    /// The trie root is order-independent, so this is a pure function of the
    /// child set — which is what two replicas must agree on.
    pub(crate) fn full_hash_from_trie(id: Id, own_hash: [u8; 32]) -> [u8; 32] {
        Self::full_hash_from_root(own_hash, <ChildTrie<S>>::new(id).root())
    }

    /// [`full_hash_from_trie`](Self::full_hash_from_trie) over rows read
    /// through `read`, for callers that reach the store directly. `None` when
    /// the trie's root row does not decode.
    pub fn full_hash_with<F>(id: Id, own_hash: [u8; 32], read: F) -> Option<[u8; 32]>
    where
        F: Fn(crate::store::Key) -> Option<Vec<u8>>,
    {
        <ChildTrie<S>>::root_with(id, read).map(|root| Self::full_hash_from_root(own_hash, root))
    }

    /// The fold itself: `own_hash`, then the trie root unless it is empty.
    ///
    /// One definition on purpose. Callers that have just written the trie hold
    /// the new root already and should not pay a row read to get it back, so
    /// they cannot go through [`full_hash_from_trie`](Self::full_hash_from_trie)
    /// — but the RULE they apply (field order, and skipping an empty root so a
    /// childless entity hashes as itself) has to be the same one, because a
    /// copy that drifts forks replicas silently: both sides compute a hash,
    /// they just compute different ones.
    pub(crate) fn full_hash_from_root(own_hash: [u8; 32], root: [u8; 32]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(own_hash);
        if root != child_trie::EMPTY {
            hasher.update(root);
        }
        hasher.finalize().into()
    }

    /// Returns the stored full Merkle hash for an entity.
    ///
    /// The stored `full_hash` is kept current by every write site
    /// (`add_root`, `add_child_to`, `recalculate_ancestor_hashes_for_now`,
    /// `update_parent_after_child_removal`), so callers can read it
    /// directly instead of re-hashing children every time. This is the
    /// common case on the merge-apply hot path and closes #2238 Fix 1
    /// (~O(K) SHA-256 work per ancestor visit → O(1)).
    ///
    /// If the entity doesn't exist in the index, returns
    /// `StorageError::IndexNotFound`. If a future write site adds `full_hash`
    /// drift somehow, tests in `tests::merkle` enforce the invariant
    /// `stored_full_hash == recompute from the child trie` after common
    /// operation sequences.
    #[cfg(test)]
    pub(crate) fn get_full_merkle_hash_for(id: Id) -> Result<[u8; 32], StorageError> {
        Self::get_hashes_for(id)?
            .map(|(full_hash, _)| full_hash)
            .ok_or(StorageError::IndexNotFound(id))
    }

    /// Returns ancestors from immediate parent to root (root itself excluded).
    ///
    /// Public so sync code outside the storage crate (the HashComparison /
    /// LevelWise leaf builders) can carry the full ancestor chain on the
    /// wire so a receiver can reconstruct a nested entity at its correct
    /// Merkle position. This is the same read the storage layer already
    /// exposes indirectly via `Index::get_index(id)`, so promoting the
    /// helper to `pub` doesn't widen the data surface.
    ///
    /// # Errors
    ///
    /// Returns `IndexNotFound` if an ancestor referenced by a child's
    /// `parent_id` has no index entry (a corrupt/partial tree), or a
    /// `StorageError` if an index read fails.
    pub fn get_ancestors_of(id: Id) -> Result<Vec<ChildInfo>, StorageError> {
        let mut ancestors = Vec::new();
        let mut current_id = id;

        while let Some(parent_id) = Self::get_parent_id(current_id)? {
            if ancestors.len() == MAX_PARENT_CHAIN {
                return Err(StorageError::ParentChainTooLong(id));
            }
            let (parent_full_hash, _) =
                Self::get_hashes_for(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;
            let metadata =
                Self::get_metadata(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;
            ancestors.push(ChildInfo::new(parent_id, parent_full_hash, metadata));
            current_id = parent_id;
        }

        Ok(ancestors)
    }

    /// The ancestor chain a delta action ships: ids alone, immediate parent
    /// first, and the context root left off the end of any chain that reaches
    /// it through another ancestor.
    ///
    /// The receiver knows the root's id, so `Interface::apply_action` puts it
    /// back on a chain that stops short of it. A direct child of the root keeps
    /// the root as its one ancestor: an empty chain already means "no parent
    /// named" to `apply_action`, and a direct child is a collection's own
    /// entity, written far less often than its entries. Nothing else from an
    /// ancestor travels (see `crate::action`), so neither its hashes nor its
    /// metadata are read. An `Update` to an entity a peer already holds ships
    /// no chain at all (see `Interface::save_raw_stamped`).
    ///
    /// # Errors
    ///
    /// Returns a `StorageError` if an index read fails.
    pub(crate) fn get_delta_ancestors_of(id: Id) -> Result<Vec<ChildInfo>, StorageError> {
        let mut ancestors = Vec::new();
        let mut current_id = id;

        let mut steps = 0;
        while let Some(parent_id) = Self::get_parent_id(current_id)? {
            if steps == MAX_PARENT_CHAIN {
                return Err(StorageError::ParentChainTooLong(id));
            }
            steps += 1;
            if parent_id.is_root() && !ancestors.is_empty() {
                break;
            }
            ancestors.push(ChildInfo::new(parent_id, [0; 32], Metadata::default()));
            current_id = parent_id;
        }

        Ok(ancestors)
    }

    /// Returns entity metadata.
    pub(crate) fn get_metadata(id: Id) -> Result<Option<Metadata>, StorageError> {
        Ok(Self::get_index(id)?.map(|index| index.metadata))
    }

    /// Persist a new `storage_type` for an existing entity's index entry.
    ///
    /// `storage_type` is not part of any Merkle hash (it lives in
    /// [`EntityIndex::metadata`], separate from the hashed entity bytes), so
    /// this is hash-neutral and cannot cause root-hash divergence. It exists so
    /// a `SharedStorage` writer-set rotation can persist the new set on the
    /// **originating** node, whose rotation log is not appended locally (only
    /// the apply path on a *receiving* node appends it). On receivers the log is
    /// the authoritative source; this keeps the local index a correct fallback.
    ///
    /// No-op if the entity has no index entry yet.
    ///
    /// # Errors
    /// Returns `StorageError` if the index cannot be loaded or written.
    pub(crate) fn set_storage_type(
        id: Id,
        storage_type: crate::entities::StorageType,
    ) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();
        if let Some(mut index) = Self::get_index(id)? {
            if index.metadata.storage_type == storage_type {
                return Ok(()); // unchanged — skip the redundant write
            }
            index.metadata.storage_type = storage_type;
            Self::save_index(&index)?;
            // The stamp decides what the parent's collection admits, and this
            // changes it without touching the parent's trie, so the parent's
            // admitted count can no longer carry it across: recount instead.
            if let Some(parent_id) = index.parent_id {
                crate::admitted_count::forget::<S>(parent_id);
            }
        }
        Ok(())
    }

    /// Persist a new `schema_version` for an existing entity's index entry
    /// (owner-driven identity-gated migration, PR-6c).
    ///
    /// `schema_version` is Merkle-invisible (it lives in
    /// [`EntityIndex::metadata`], never in the hashed entity bytes — see
    /// [`Metadata::schema_version`]), so this is hash-neutral and cannot cause
    /// root-hash divergence. It exists because a re-write of an existing entry
    /// flows through [`Index::write_value_for`], which only touches the entity
    /// hashes and `updated_at` — it deliberately does NOT rewrite the stored
    /// metadata (see `tests/write_hook_stale_writers.rs`). The owner-driven
    /// convert therefore stamps the new schema tag through this dedicated,
    /// field-scoped setter rather than widening `write_value_for`.
    ///
    /// No-op if the entity has no index entry yet, or if the tag is unchanged.
    ///
    /// [`Metadata::schema_version`]: crate::entities::Metadata::schema_version
    ///
    /// # Errors
    /// Returns `StorageError` if the index cannot be loaded or written.
    pub(crate) fn set_schema_version(
        id: Id,
        schema_version: Option<u32>,
    ) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();
        if let Some(mut index) = Self::get_index(id)? {
            if index.metadata.schema_version == schema_version {
                return Ok(()); // unchanged — skip the redundant write
            }
            index.metadata.schema_version = schema_version;
            Self::save_index(&index)?;
        }
        Ok(())
    }

    /// Checks if an entity is deleted (tombstone marker set).
    ///
    /// Returns false if entity has no index (not found).
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    pub fn is_deleted(id: Id) -> Result<bool, StorageError> {
        Ok(Self::get_index(id)?
            .and_then(|index| index.deleted_at)
            .is_some())
    }

    /// Whether a delete of the written-once entry at `id`, under `rules`, has
    /// been recorded here before the entry itself was: see
    /// [`seal_written_once`](Self::seal_written_once).
    ///
    /// # Errors
    /// Returns `StorageError` if the seal cannot be loaded or deserialized.
    pub(crate) fn is_sealed(id: Id, rules: &EntryRules) -> Result<bool, StorageError> {
        Ok(Self::get_index(seal_id(id, rules))?.is_some_and(|seal| seal.is_terminal_tombstone()))
    }

    /// Records a delete of the written-once entry at `id` that reached this
    /// node before the entry did, so the writes that follow it are refused.
    ///
    /// The record is kept at an id derived from `id` AND the entry's rules
    /// (`seal_id`), not in the entry's own slot. A delete names the rules
    /// itself, and only a writer of the moderators they name may sign it, so
    /// anyone can seal an id under rules naming moderators of their own. Keyed
    /// by rules, such a seal only ever matches a write under those same rules,
    /// never the owner's write under the collection's real ones. It links to no
    /// parent, so no hash sees it.
    pub(crate) fn seal_written_once(
        id: Id,
        metadata: &Metadata,
        deleted_at: u64,
    ) -> Result<(), StorageError> {
        let StorageType::User { rules, .. } = &metadata.storage_type else {
            return Err(StorageError::InvalidData(
                "only an owned entry is sealed".to_owned(),
            ));
        };
        let _mutation_guard = index_mutation_guard();
        let seal = seal_id(id, rules);
        let deleted_at = Self::get_index(seal)?
            .and_then(|index| index.deleted_at)
            .map_or(deleted_at, |existing| existing.max(deleted_at));
        Self::save_index(&EntityIndex {
            id: seal,
            parent_id: None,
            full_hash: [0; 32],
            own_hash: [0; 32],
            metadata: metadata.clone(),
            deleted_at: Some(deleted_at),
            deleted_children: Vec::new(),
        })
    }

    /// Marks an entity as deleted (sets tombstone).
    pub(crate) fn mark_deleted(id: Id, deleted_at: u64) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();
        if let Some(mut index) = Self::get_index(id)? {
            // The tombstone time is a LWW CRDT nonce: never regress it. An
            // out-of-order, older `DeleteRef` must not roll back a newer
            // tombstone, which would also weaken replay protection via the
            // `updated_at` nonce below. Keep the latest of the two.
            let deleted_at = index
                .deleted_at
                .map_or(deleted_at, |existing| existing.max(deleted_at));
            index.deleted_at = Some(deleted_at);

            // Also advance the `updated_at` timestamp toward this nonce.
            // This is critical for replay protection on delete actions, and is
            // likewise monotonic — an older delete never lowers it.
            *index.metadata.updated_at = (*index.metadata.updated_at).max(deleted_at);

            // The delete removes the data and the child trie, so neither hash
            // describes anything any more. Zero them, which the row stores in
            // no bytes; a write that lifts the tombstone rehashes both.
            index.own_hash = [0; 32];
            index.full_hash = childless_full_hash(&index.own_hash);

            Self::save_index(&index)?;
        }
        Ok(())
    }

    /// Returns children from a specific collection.
    ///
    /// Collection param is ignored - entity only has one collection.
    /// Kept in API for backwards compatibility.
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    pub fn get_children_of(parent_id: Id) -> Result<Vec<ChildInfo>, StorageError> {
        let _index = Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        Ok(<ChildTrie<S>>::new(parent_id).children())
    }

    /// `parent_id`'s child `child_id`, if it has one: one trie bucket read.
    #[must_use]
    pub(crate) fn child_of(parent_id: Id, child_id: Id) -> Option<ChildInfo> {
        <ChildTrie<S>>::new(parent_id).get(child_id)
    }

    /// How many children `parent_id` has, without enumerating them.
    ///
    /// One row read: the trie maintains a subtree count along the spine an
    /// insert already walks. Callers that count on every write — a contract
    /// deriving an id from `len()`, the ancestor-recompute log line — must use
    /// this rather than `get_children_of(..).len()`, which is a full walk.
    ///
    /// Exists so the rest of the crate does not reach past `Index` into
    /// `ChildTrie`. That type's public surface is earned by the callers that
    /// reach the store directly with no `StorageAdaptor` — snapshot install and
    /// the raw-DB diagnostics — not by intra-crate use.
    #[must_use]
    pub fn child_count(parent_id: Id) -> u64 {
        <ChildTrie<S>>::new(parent_id).len()
    }

    /// The children of `parent_id` whose id starts with `prefix`, ascending by
    /// id: one trie bucket read when `prefix` covers the nibbles buckets are
    /// addressed by.
    #[must_use]
    pub fn children_with_prefix(parent_id: Id, prefix: &[u8]) -> Vec<ChildInfo> {
        <ChildTrie<S>>::new(parent_id).children_with_prefix(prefix)
    }

    /// A page of `parent_id`'s children with id at or above `from`, and the id
    /// to resume from; see [`ChildTrie::children_from`].
    #[must_use]
    pub fn children_from(parent_id: Id, from: Id, at_least: usize) -> (Vec<ChildInfo>, Option<Id>) {
        <ChildTrie<S>>::new(parent_id).children_from(from, at_least)
    }

    /// Returns (full_hash, own_hash) tuple for an entity.
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    #[expect(clippy::type_complexity, reason = "Not too complex")]
    pub fn get_hashes_for(id: Id) -> Result<Option<([u8; 32], [u8; 32])>, StorageError> {
        Ok(Self::get_index(id)?.map(|index| (index.full_hash, index.own_hash)))
    }

    /// Loads entity index from storage.
    ///
    /// Returns the full `EntityIndex` for an entity if it exists.
    /// Used by sync protocols to traverse the Merkle tree.
    ///
    /// # Errors
    /// Returns `StorageError` if index cannot be loaded or deserialized.
    pub fn get_index(id: Id) -> Result<Option<EntityIndex>, StorageError> {
        S::storage_read_index(id)
            .transpose()
            .map_err(StorageError::DeserializationError)
    }

    /// [`get_index`](Self::get_index), with the entity's data from the same
    /// row read, so a caller that rewrites the index can hand the data back
    /// to [`save_index_keeping`](Self::save_index_keeping) instead of the
    /// write reading the row again.
    fn get_index_with_value(id: Id) -> Result<Option<IndexWithValue>, StorageError> {
        let row = S::storage_read_entity(id);
        let Some(index) = row.index else {
            return Ok(None);
        };
        let index = index.map_err(StorageError::DeserializationError)?;
        Ok(Some((index, row.data)))
    }

    /// Checks if an entity has an index.
    pub(crate) fn has_index(id: Id) -> bool {
        S::storage_read(Key::Index(id)).is_some()
    }

    /// Returns the parent ID of an entity.
    pub(crate) fn get_parent_id(child_id: Id) -> Result<Option<Id>, StorageError> {
        Ok(Self::get_index(child_id)?.and_then(|index| index.parent_id))
    }

    /// Checks if a collection has any children.
    ///
    /// Collection param ignored - just checks if entity has any children.
    pub fn has_children(parent_id: Id) -> Result<bool, StorageError> {
        let parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        let _ = &parent_index;
        Ok(<ChildTrie<S>>::new(parent_id).root() != child_trie::EMPTY)
    }

    /// Recalculates ancestor hashes recursively up to root.
    ///
    /// Defers to a `DeferredAncestorScope<S>` if one is active on this
    /// thread (#2238): during a merge, many calls with the same starting
    /// id dedupe into a single walk at scope flush. A scope opened against
    /// a *different* `StorageAdaptor` type does not apply — this call
    /// falls through to the immediate walk, preventing cross-backend
    /// contamination.
    ///
    /// The impl block's `S: 'static` bound is required to compute
    /// `TypeId::of::<S>()` for the per-adaptor thread-local lookup. All
    /// production `StorageAdaptor` impls are unit/const structs so they
    /// trivially satisfy it.
    pub(crate) fn recalculate_ancestor_hashes_for(id: Id) -> Result<(), StorageError> {
        let self_type = TypeId::of::<S>();
        let deferred = DEFERRED_ANCESTORS.with(|slot| {
            let mut borrowed = slot.borrow_mut();
            match borrowed.as_mut() {
                Some((existing_type, set)) if *existing_type == self_type => {
                    let _ = set.insert(id);
                    true
                }
                _ => false,
            }
        });
        if deferred {
            return Ok(());
        }
        Self::recalculate_ancestor_hashes_for_now(id)
    }

    /// The immediate, non-deferred ancestor walk.
    ///
    /// Called directly from `recalculate_ancestor_hashes_for` when no
    /// defer scope is active, and from `DeferredAncestorScope::drop`
    /// when flushing a deferred set.
    pub(crate) fn recalculate_ancestor_hashes_for_now(id: Id) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();
        // The entity whose hash moved, as its parent's trie lists it. After the
        // first step it is the parent just saved, so it comes from that save
        // rather than from reading the row back; its metadata is what reading
        // the row would give, which is what the trie records a child with.
        let Some(start) = Self::get_index(id)? else {
            return Ok(());
        };
        let mut parent = start.parent_id;
        let mut child = ChildInfo::new(id, start.full_hash, start.metadata);

        let mut steps = 0;
        while let Some(parent_id) = parent {
            if steps == MAX_PARENT_CHAIN {
                return Err(StorageError::ParentChainTooLong(id));
            }
            steps += 1;
            let (mut parent_index, parent_value) = Self::get_index_with_value(parent_id)?
                .ok_or(StorageError::IndexNotFound(parent_id))?;

            // Store the child's hash in the parent's trie. When the slot holds
            // it already, or the parent does not list the child, the trie and
            // so the parent's hash are as they were, and every ancestor above
            // already folds that hash: every write that moves a hash walks from
            // the entity it moved, and a walk passes a level only by bringing
            // it up to date. Going on could only rewrite each row above with the
            // bytes it already holds.
            let Some(trie_root) = <ChildTrie<S>>::new(parent_id).refresh(&child) else {
                break;
            };
            if parent_id.is_root() {
                info!(
                    target: "storage::merkle",
                    child_id = %child.id(),
                    new_child_hash = %hex::encode(child.merkle_hash()),
                    "ROOT MERKLE: Child hash updated"
                );
            }

            let old_full_hash = parent_index.full_hash;
            parent_index.full_hash = Self::full_hash_from_root(parent_index.own_hash, trie_root);

            if parent_id.is_root() && old_full_hash != parent_index.full_hash {
                // A field, so it is read only when the event is enabled: this
                // runs on the ancestor walk of every write.
                info!(
                    target: "storage::merkle",
                    parent_id = %parent_id,
                    old_full_hash = %hex::encode(old_full_hash),
                    new_full_hash = %hex::encode(parent_index.full_hash),
                    children_count = Self::child_count(parent_id),
                    "ROOT MERKLE: Root hash recalculated from ancestor"
                );
            }

            // The trie writes above touch the trie's rows, not the parent's, so
            // the data read with its index is still what it holds.
            Self::save_index_keeping(&parent_index, parent_value.as_deref())?;
            parent = parent_index.parent_id;
            child = ChildInfo::new(parent_id, parent_index.full_hash, parent_index.metadata);
        }

        Ok(())
    }

    /// Removes and deletes a child from a collection.
    ///
    /// Uses tombstone-based deletion. Deleting a child also tombstones its
    /// entire subtree (see [`tombstone_descendants_of`](Self::tombstone_descendants_of)):
    /// a non-recursive delete would leave descendant index rows live, so they
    /// could never be reclaimed by tombstone GC and would linger as orphans
    /// under a tombstoned parent.
    ///
    /// `deleted_at` is the caller's tombstone nonce — the same value carried on
    /// the `DeleteRef` wire action — so the child, every descendant, and the
    /// remote replicas that replay the `DeleteRef` all stamp the subtree with
    /// one identical timestamp and converge.
    ///
    /// To move a child to a different parent, add it to the new parent instead.
    pub(crate) fn remove_child_from(
        parent_id: Id,
        child_id: Id,
        deleted_at: u64,
    ) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();
        // Tombstone the subtree first (while `child_id`'s `children` are still
        // readable), then the child itself, then detach it from its parent.
        Self::tombstone_descendants_of(child_id, deleted_at)?;
        Self::delete_entity_and_create_tombstone(child_id, deleted_at)?;
        Self::update_parent_after_child_removal(parent_id, child_id)?;
        Self::recalculate_ancestor_hashes_for(parent_id)?;
        Ok(())
    }

    /// Deletes entity data and creates a tombstone marker for a single entity.
    ///
    /// Step 1 of deletion: Remove actual data, keep index for CRDT sync.
    pub(crate) fn delete_entity_and_create_tombstone(
        id: Id,
        deleted_at: u64,
    ) -> Result<(), StorageError> {
        // Tombstone first, then drop the data: if mark_deleted fails the entry
        // is left intact and the delete can be retried, instead of leaving the
        // data gone with no tombstone while the parent still lists it live.
        Self::mark_deleted(id, deleted_at)?;
        let _ignored = S::storage_remove(Key::Entry(id));
        // The entity's own child trie goes with its data. Nothing else reaches
        // those rows — they are their own keyspace, so tombstone GC (which
        // requires a row to decode as a tombstoned `EntityIndex`) never sees
        // them. Left behind, they resurrect as ghost children the next time a
        // deterministically-named collection is re-created at the same id.
        <ChildTrie<S>>::new(id).drop_all();
        Ok(())
    }

    /// Tombstones every descendant of `root_id` at `deleted_at`.
    ///
    /// Deleting an entity detaches its whole subtree from the live Merkle tree
    /// (the root is dropped from its parent's `children`), but the descendant
    /// index rows remain. Without tombstoning them they keep `deleted_at =
    /// None` forever, so tombstone GC can never reclaim them and they leak. This
    /// walks the subtree via each node's `children` and tombstones the rows.
    ///
    /// Each descendant is resolved with the SAME canonical delete-vs-update
    /// tiebreak that [`apply_delete_ref_action`] uses for a directly-deleted
    /// entity: a descendant whose own `updated_at` is STRICTLY newer than
    /// `deleted_at` (a concurrent update that causally follows the delete) keeps
    /// its data; equal-or-older state is tombstoned — so `deleted_at ==
    /// updated_at` tombstones the descendant (delete wins on the tie), matching
    /// the single-level rule. Running the identical rule here and on the
    /// `DeleteRef` replay path means every replica stamps the subtree the same
    /// way and converges, including the live-update-wins "zombie" case (a
    /// surviving entity under a tombstoned ancestor), which is the existing
    /// single-level concurrent add/update-vs-delete semantics applied
    /// transitively.
    ///
    /// `Frozen` descendants are the one exception: they are immutable and never
    /// deleted, so the walk skips a `Frozen` node and its whole subtree rather
    /// than tombstoning it. A local delete never reaches this state — it is
    /// rejected up front by `remove_child_from`'s `find_frozen_descendant` scan.
    /// The skip here is the replay-side fallback: on `apply_delete_ref_action`
    /// this replica can't reject a peer's `DeleteRef` without diverging, so it
    /// preserves the frozen data (leaving it a detached orphan) and converges.
    ///
    /// Internal subtree parent-lists and hashes are intentionally NOT
    /// recomputed: the whole subtree is detached at the root, so none of it
    /// feeds a live hash. Only the root's removal from its parent (done by the
    /// caller) touches the live tree.
    ///
    /// Acquires the reentrant index-mutation guard internally and holds it for
    /// the whole walk, so it is safe to call whether or not a caller already
    /// holds it (`remove_child_from` does; `apply_delete_ref_action` does not).
    ///
    /// [`apply_delete_ref_action`]: crate::interface::Interface::apply_delete_ref_action
    pub(crate) fn tombstone_descendants_of(
        root_id: Id,
        deleted_at: u64,
    ) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();

        // Single depth-first pass: read each node's index once, descend via the
        // `children` read BEFORE tombstoning it, then tombstone the node itself
        // — except the root, which the caller tombstones. The guard is held for
        // the whole walk, so a concurrent native writer can't insert a child
        // that the traversal would miss.
        let mut stack = vec![root_id];
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            // A child trie that lists an ancestor would otherwise be walked forever.
            if !seen.insert(id) {
                continue;
            }
            let Some(index) = Self::get_index(id)? else {
                continue;
            };
            // Frozen data is immutable and never deleted: skip a Frozen node AND
            // its subtree (don't recurse) so deleting a non-frozen parent leaves
            // the frozen descendant as a surviving orphan. Mirrors the
            // `RemoveMode::Delete` Frozen guard in `Interface`. Scoped to
            // descendants (`id != root_id`): both callers (`remove_child_from`,
            // `apply_delete_ref_action`) reject deleting Frozen data upstream, so
            // the root is never Frozen here; the caller tombstones it regardless.
            if id != root_id
                && matches!(
                    index.metadata.storage_type,
                    crate::entities::StorageType::Frozen
                )
            {
                continue;
            }
            {
                for child in <ChildTrie<S>>::new(id).children() {
                    stack.push(child.id());
                }
            }
            // The root is tombstoned by the caller; only its descendants here.
            if id == root_id {
                continue;
            }
            // Strictly-newer local state wins (a concurrent update that causally
            // follows this delete); equal-or-older is tombstoned (`deleted_at ==
            // updated_at` ⇒ delete wins). Same `<` tiebreak as
            // `apply_delete_ref_action`.
            if deleted_at < *index.metadata.updated_at {
                continue;
            }
            Self::delete_entity_and_create_tombstone(id, deleted_at)?;
        }

        Ok(())
    }

    /// Returns the id of the first `Frozen` entity in `root_id`'s subtree
    /// (excluding `root_id` itself), or `None` if the subtree holds no frozen
    /// data.
    ///
    /// Read-only pre-check for the local delete guard in
    /// [`remove_child_from`](crate::interface::Interface::remove_child_from): a
    /// genuine subtree delete must refuse to strand `Frozen` data, so the
    /// caller rejects the delete and asks the operator to relocate the frozen
    /// entity out of the subtree first. Kept separate from
    /// [`tombstone_descendants_of`](Self::tombstone_descendants_of) because the
    /// check must complete and reject BEFORE any state is mutated.
    pub(crate) fn find_frozen_descendant(root_id: Id) -> Result<Option<Id>, StorageError> {
        let _mutation_guard = index_mutation_guard();
        let mut stack = vec![root_id];
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(index) = Self::get_index(id)? else {
                continue;
            };
            if id != root_id
                && matches!(
                    index.metadata.storage_type,
                    crate::entities::StorageType::Frozen
                )
            {
                return Ok(Some(id));
            }
            {
                for child in <ChildTrie<S>>::new(id).children() {
                    stack.push(child.id());
                }
            }
        }
        Ok(None)
    }

    /// Removes a child reference from a parent without creating a tombstone.
    ///
    /// Used when reassigning collection IDs - we need to remove the old child
    /// reference from the parent but don't want to create a tombstone since
    /// the collection is being moved, not deleted.
    pub(crate) fn remove_child_reference_only(
        parent_id: Id,
        child_id: Id,
    ) -> Result<(), StorageError> {
        Self::update_parent_after_child_removal(parent_id, child_id)?;
        Self::recalculate_ancestor_hashes_for(parent_id)?;
        Ok(())
    }

    /// Updates parent's children list and hash after child removal.
    ///
    /// Step 2 of deletion: Remove child from parent's index and recalculate hash.
    /// Made pub(crate) so Interface::apply_delete_ref_action can use it.
    pub(crate) fn update_parent_after_child_removal(
        parent_id: Id,
        child_id: Id,
    ) -> Result<(), StorageError> {
        let _mutation_guard = index_mutation_guard();
        let mut parent_index =
            Self::get_index(parent_id)?.ok_or(StorageError::IndexNotFound(parent_id))?;

        // Remove child from the parent's trie (collection name ignored).
        let trie_root = <ChildTrie<S>>::new(parent_id).remove(child_id);

        // Record the removal so the deletion is explicit on the sync wire
        // (the child vanishes from `children`, which the add-biased HC/LevelWise
        // comparison can't otherwise observe). Deduped; the per-child
        // `deleted_at` + signed metadata are read from the child's own tombstone
        // index at wire-build time.
        if !parent_index.deleted_children.contains(&child_id) {
            parent_index.deleted_children.push(child_id);
        }

        // Recalculate parent's hash from the trie root.
        parent_index.full_hash = Self::full_hash_from_root(parent_index.own_hash, trie_root);
        Self::save_index(&parent_index)?;

        Ok(())
    }

    /// Removes an entity's index from storage.
    #[cfg(test)]
    pub(crate) fn remove_index(id: Id) {
        _ = S::storage_remove(Key::Index(id));
    }

    /// Saves entity index to storage.
    pub(crate) fn save_index(index: &EntityIndex) -> Result<(), StorageError> {
        _ = S::storage_write_index(index);
        Ok(())
    }

    /// Saves entity index to storage over `value`, the entity's data as read
    /// with the index (see [`get_index_with_value`](Self::get_index_with_value)):
    /// one row write when there is data, nothing read back.
    fn save_index_keeping(index: &EntityIndex, value: Option<&[u8]>) -> Result<(), StorageError> {
        match value {
            Some(data) => Self::save_index_with_value(index, data),
            None => Self::save_index(index),
        }
    }

    /// Saves entity index to storage together with the entity's data, in one
    /// row write.
    pub(crate) fn save_index_with_value(
        index: &EntityIndex,
        data: &[u8],
    ) -> Result<(), StorageError> {
        S::storage_write_entity(index, data);
        Ok(())
    }

    /// Updates entity's own_hash and recalculates full_hash.
    ///
    /// Returns the calculated full_hash (includes descendants).
    #[cfg(test)]
    pub(crate) fn update_hash_for(
        id: Id,
        merkle_hash: [u8; 32],
        updated_at: Option<UpdatedAt>,
        crdt_type: Option<crate::collections::crdt_meta::CrdtType>,
    ) -> Result<[u8; 32], StorageError> {
        let _mutation_guard = index_mutation_guard();
        let (index, _) = Self::rehashed(id, merkle_hash, updated_at, crdt_type)?;
        Self::save_index(&index)?;
        <Index<S>>::recalculate_ancestor_hashes_for(id)?;
        Ok(index.full_hash)
    }

    /// Stores `data` as entity `id`'s value and updates its index to match
    /// (`own_hash = Sha256(data)`, `full_hash`, `updated_at`, and `crdt_type`
    /// when given) — one row write for both, so the value and the hash that
    /// records it can never come from different writers (core#2571), and a
    /// reader never sees one without the other.
    ///
    /// `lift_tombstone_at`: a value write that causally follows an existing
    /// tombstone must lift it, or `find_by_id` would keep hiding the bytes just
    /// written (the entity's `updated_at` already outran the tombstone in the
    /// caller's LWW guard, so the write won — but a stale `deleted_at` would
    /// silently suppress it, diverging replicas on delete-then-update vs
    /// update-only delivery). Only a stamp strictly newer than `deleted_at`
    /// lifts it, so ties and older writes do not resurrect; the `updated_at`
    /// replay nonce never moves back past it, mirroring `mark_deleted`.
    ///
    /// Returns the calculated full_hash (includes descendants).
    pub(crate) fn write_value_for(
        id: Id,
        data: &[u8],
        updated_at: UpdatedAt,
        crdt_type: Option<crate::collections::crdt_meta::CrdtType>,
        lift_tombstone_at: Option<u64>,
    ) -> Result<[u8; 32], StorageError> {
        let _mutation_guard = index_mutation_guard();
        let (mut index, stored_full_hash) =
            Self::rehashed(id, Sha256::digest(data).into(), Some(updated_at), crdt_type)?;
        if let Some(at) = lift_tombstone_at {
            if index.deleted_at.is_some_and(|deleted_at| at > deleted_at) {
                index.deleted_at = None;
                *index.metadata.updated_at = (*index.metadata.updated_at).max(at);
            }
        }
        Self::save_index_with_value(&index, data)?;
        // A rewrite that leaves the full hash where it was (the bytes a link
        // just wrote, or a value set to what it held) moves no ancestor: the
        // parent's trie already holds this hash, or a walk still pending from
        // the change that made it differ will store it.
        if index.full_hash != stored_full_hash {
            <Index<S>>::recalculate_ancestor_hashes_for(id)?;
        }
        Ok(index.full_hash)
    }

    /// Entity `id`'s index with `own_hash` set to `merkle_hash` and
    /// `full_hash` recomputed, not yet saved, and the full hash it had stored.
    fn rehashed(
        id: Id,
        merkle_hash: [u8; 32],
        updated_at: Option<UpdatedAt>,
        crdt_type: Option<crate::collections::crdt_meta::CrdtType>,
    ) -> Result<(EntityIndex, [u8; 32]), StorageError> {
        // RMW on this entity's index entry — the caller holds the mutation
        // guard, serializing against a concurrent `add_child_to` on the same
        // entry so neither clobbers the other (core#2571).
        let mut index = Self::get_index(id)?.ok_or(StorageError::IndexNotFound(id))?;
        let old_own_hash = index.own_hash;
        let old_full_hash = index.full_hash;
        index.own_hash = merkle_hash;
        index.full_hash = Self::full_hash_from_trie(id, index.own_hash);
        if let Some(updated_at) = updated_at {
            index.metadata.updated_at = updated_at;
        }
        // `crdt_type` is set only at `add_root`/`add_child_to` creation; a plain
        // hash update never carried it, so a root's merge-dispatch tag was frozen
        // at whatever it was first stored as. A JS app that opts into the WASM
        // root merge stamps the `JsRoot` marker on every `persist_root_state`
        // write, but if the root was first created opaque (`None`) — e.g. a joiner
        // materialises it via sync before ever writing locally — that stamp was
        // silently dropped here, so the root stayed opaque and never routed to the
        // guest merge. Threading the update through lets a local write (re)assert
        // the marker. `None` means "leave unchanged" (every non-root caller);
        // callers only ever pass `Some` to SET a tag, never to clear one.
        if let Some(crdt_type) = crdt_type {
            index.metadata.crdt_type = Some(crdt_type);
        }

        // Log detailed info for root entity hash updates
        if id.is_root() {
            // O(1): the trie maintains the count. Enumerating every child here
            // put a LINEAR READ on every root hash update — that is, on every
            // write — and hex-formatted each one into a String on the way.
            // Measured at 599 records: 7,697 reads and 640 KB read for a single
            // insert, versus 76 writes. It was also built eagerly, so the cost
            // was paid even when the log level discarded the result.
            let children_count = <ChildTrie<S>>::new(id).len();
            // The per-child dump is a genuine diagnostic, so keep it — but only
            // materialise it when something is actually listening at TRACE.
            let children_hashes: Vec<String> = if tracing::enabled!(target: "storage::merkle", tracing::Level::TRACE)
            {
                <ChildTrie<S>>::new(id)
                    .children()
                    .iter()
                    .map(|child| format!("{}:{}", child.id(), hex::encode(child.merkle_hash())))
                    .collect()
            } else {
                Vec::new()
            };
            info!(
                target: "storage::merkle",
                %id,
                old_own_hash = %hex::encode(old_own_hash),
                new_own_hash = %hex::encode(merkle_hash),
                old_full_hash = %hex::encode(old_full_hash),
                new_full_hash = %hex::encode(index.full_hash),
                children_count,
                children_hashes = ?children_hashes,
                "ROOT MERKLE: Hash update for root entity"
            );
        }

        Ok((index, old_full_hash))
    }
}
