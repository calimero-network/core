//! High-level data structures for storage.

use core::cell::RefCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::{fmt, ptr};
use std::collections::BTreeMap;
use std::sync::LazyLock;

use borsh::{BorshDeserialize, BorshSerialize};
use indexmap::IndexSet;
use sha2::{Digest, Sha256};

pub mod counter;
pub use counter::{Counter, GCounter, PNCounter};
pub mod unordered_map;
pub use unordered_map::UnorderedMap;
pub mod sorted_map;
pub use sorted_map::SortedMap;
pub mod unordered_set;
pub use unordered_set::UnorderedSet;
pub mod sorted_set;
pub use sorted_set::SortedSet;
pub mod vector;
pub use vector::Vector;
pub mod rga;
pub use rga::ReplicatedGrowableArray;
pub mod lww_register;
pub use lww_register::LwwRegister;
pub mod crdt_meta;
pub use crdt_meta::{CrdtMeta, CrdtType, Decomposable, Mergeable, StorageStrategy};
// Re-export of the `Mergeable` *derive macro*, whose single canonical
// implementation lives in `calimero-sdk-macros` (it shares the forbidden-type
// field lint with `#[app::state]`, which is why it can't live in this crate's
// macros). Exposing it here under the same name as the trait — serde-style — lets
// a single `use calimero_storage::collections::Mergeable;` bring in both; the two
// occupy different namespaces (trait vs. derive macro), so the shared name does
// not clash. This relies on `calimero-storage`'s existing dependency on
// `calimero-sdk` (the edge already points storage -> sdk; sdk does not depend on
// storage, so there is no cycle). The path `calimero_sdk::app::Mergeable` is
// compile-checked here: if it ever moves, this line fails to build rather than
// silently breaking, and `tests/derive_mergeable.rs` exercises the re-exported
// derive through this exact path.
pub use calimero_sdk::app::Mergeable;
pub mod composite_key;
mod crdt_impls;
mod decompose_impls;
pub mod rekey;
pub use composite_key::CompositeKey;
pub mod nested;
pub use nested::{get_nested, insert_nested, insert_nested_decomposable, NestedConfig};
pub mod nested_map;
pub use nested_map::NestedMapOps;
mod root;
#[doc(hidden)]
pub use root::Root;
pub mod error;
pub use error::StoreError;

pub mod user;
pub use user::UserStorage;
pub mod shared;
pub use shared::WriterSetCell;
pub mod permissioned;
pub use permissioned::{
    Authorizer, Op, Ownable, OwnerAcl, PermissionedStorage, ProtocolAuthorizer, SharedStorage,
    WriterSetAcl,
};
pub mod access_control;
pub use access_control::AccessControl;
mod authored_common;
pub mod authored_map;
pub use authored_map::AuthoredMap;
pub mod authored_vector;
pub use authored_vector::AuthoredVector;
pub mod frozen;
pub use frozen::FrozenStorage;
pub mod frozen_value;
pub use frozen_value::FrozenValue;

/// An owned, **read-only** view of a value returned by a collection's `get`.
///
/// Storage values are not resident in memory the way a `HashMap`'s are — every
/// `get` deserializes a fresh copy from the backing store — so this cannot be a
/// borrow like `HashMap::get`'s `&V`. Instead it owns the deserialized value and
/// exposes it *immutably only* (`Deref`, deliberately no `DerefMut`).
///
/// That turns the most common storage footgun into a compile error: mutating a
/// `get` result and forgetting to write it back used to silently discard the
/// change (the value was a throwaway copy). Now the mutation does not compile —
/// the borrow checker steers you to the right tool:
///
/// - to **mutate and persist**, use [`get_mut`] or [`entry`]`().or_default()`
///   (both write back automatically — no manual re-insert);
/// - to **take an owned copy on purpose**, `.clone()` it (when `V: Clone`).
///
/// There is intentionally no public `into_inner`/unwrap: handing back an owned,
/// mutable value with no write-back is exactly the footgun this guard closes.
///
/// [`get_mut`]: UnorderedMap::get_mut
/// [`entry`]: UnorderedMap::entry
pub struct ValueRef<V> {
    value: V,
}

impl<V> ValueRef<V> {
    /// Wrap an owned value just read from storage. Internal: only collection
    /// `get` methods mint these.
    pub(crate) const fn new(value: V) -> Self {
        Self { value }
    }

    /// Consume the guard and take ownership of the value.
    ///
    /// Deliberately **crate-internal**: exposing it publicly would re-open the
    /// footgun this guard exists to close — `map.get(k)?.into_inner()` yields an
    /// owned, mutable copy whose changes are silently dropped unless manually
    /// re-`insert`ed. Public callers should instead mutate-and-persist via
    /// [`get_mut`](UnorderedMap::get_mut) / [`entry`](UnorderedMap::entry)`().or_default()`,
    /// or take an explicit owned copy with `.clone()` when `V: Clone`. The
    /// storage crate itself uses this for the few deliberate read-modify-write
    /// paths (e.g. CRDT merge) where a held mutable guard would conflict.
    pub(crate) fn into_inner(self) -> V {
        self.value
    }
}

impl<V> Deref for ValueRef<V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<V: fmt::Debug> fmt::Debug for ValueRef<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

impl<V: PartialEq> PartialEq for ValueRef<V> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<V: Eq> Eq for ValueRef<V> {}

// Intentionally NO `Clone`/`AsRef`/`Hash`/`Borrow` impls: `ValueRef` is a
// `Deref` guard, so `guard.clone()`, `guard.as_ref()`, `guard.hash(..)` already
// resolve to `V`'s own methods through the deref. Adding inherent trait impls
// here would *shadow* those — e.g. `file_record.clone()` would yield a
// `ValueRef<FileRecord>` instead of a `FileRecord`, silently changing callers.
// To take the value out explicitly, use [`ValueRef::into_inner`].

/// Compare a guard directly against a bare `V`, so `map.get(k)?.unwrap() == v`
/// works without unwrapping the guard first. (Operator-based, so it does not
/// shadow any `Deref` method.)
impl<V: PartialEq> PartialEq<V> for ValueRef<V> {
    fn eq(&self, other: &V) -> bool {
        &self.value == other
    }
}

// fixme! macro expects `calimero_storage` to be in deps
use crate as calimero_storage;
use crate::address::Id;
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::{AtomicUnit, Collection};

/// Domain separator for map entry IDs to prevent collision with collection IDs.
/// This ensures that a map entry with key "X" never collides with a nested collection
/// with field name "X" in the same parent.
const DOMAIN_SEPARATOR_ENTRY: &[u8] = b"__calimero_entry__";

/// Domain separator for collection IDs to prevent collision with map entry IDs.
/// This ensures that a nested collection with field name "X" never collides with a
/// map entry with key "X" in the same parent.
const DOMAIN_SEPARATOR_COLLECTION: &[u8] = b"__calimero_collection__";

/// Compute the ID for a key in a map.
/// Uses domain separation to prevent collision with collection IDs.
pub(crate) fn compute_id(parent: Id, key: &[u8]) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(parent.as_bytes());
    hasher.update(DOMAIN_SEPARATOR_ENTRY);
    hasher.update(key);
    Id::new(hasher.finalize().into())
}

/// Compute a deterministic collection ID from parent ID and field name.
/// This ensures the same collection gets the same ID across all nodes.
/// Uses domain separation to prevent collision with map entry IDs.
pub(crate) fn compute_collection_id(parent_id: Option<Id>, field_name: &str) -> Id {
    let mut hasher = Sha256::new();
    if let Some(parent) = parent_id {
        hasher.update(parent.as_bytes());
    }
    hasher.update(DOMAIN_SEPARATOR_COLLECTION);
    hasher.update(field_name.as_bytes());
    Id::new(hasher.finalize().into())
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Collection<T, S: StorageAdaptor = MainStorage> {
    storage: Element,

    #[borsh(skip)]
    children_ids: RefCell<Option<IndexSet<Id>>>,

    #[borsh(skip)]
    _priv: PhantomData<(T, S)>,
}

impl<T, S: StorageAdaptor> Data for Collection<T, S> {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

/// A collection of entries in a map.
#[derive(Collection, Copy, Clone, Debug, Eq, PartialEq)]
#[children(Entry<T>)]
struct Entries<T> {
    /// Helper to associate the generic types with the collection.
    _priv: PhantomData<T>,
}

/// An entry in a map.
#[derive(AtomicUnit, BorshSerialize, BorshDeserialize, Clone, Debug)]
struct Entry<T> {
    /// The item in the entry.
    item: T,
    /// The storage element for the entry.
    #[storage]
    storage: Element,
}

#[expect(unused_qualifications, reason = "AtomicUnit macro is unsanitized")]
type StoreResult<T> = std::result::Result<T, StoreError>;

static ROOT_ID: LazyLock<Id> = LazyLock::new(|| Id::root());

/// The fixed id under which an app's `Root<T>` value lives — a single
/// child of [`ROOT_ID`]. Mirrors [`root::Root::entry_id`] but available
/// at module scope so the merge dispatch in `interface.rs` can
/// recognise it without picking a concrete `T`.
///
/// **Why `[118; 32]`?** This is the historical sentinel — the entry id
/// that has always held the app's `Root<T>` payload. It is a fixed
/// well-known constant, not a hash, and changing it would break wire
/// compatibility with every existing peer that ships state under this
/// id. Two collision-resistance properties:
///
///   1. Random ids (`Id::random()` — 32 cryptographically-random bytes)
///      collide with this constant with probability ~`2^-256`.
///   2. Field-name-derived ids (used for nested-collection entries) go
///      through `compute_id` and are bound to a parent id + field-name
///      hash, so they cannot reach `[118; 32]` from any byte-collision-
///      free hash function modulo the same astronomical odds.
///
/// In short: `[118; 32]` cannot be reached unintentionally; an attacker
/// who *could* synthesise an entity at this id would already have a
/// hash-collision primitive on the entity-id space.
pub(crate) const ROOT_ENTRY_ID: Id = Id::new([118; 32]);

/// Whether `id` addresses the app's root state — either the canonical
/// `ROOT_ID` (system root) or the `Root<T>` entry (the WASM app's
/// serialised root-state container).
///
/// Both ids share the same merge path: their content is the app's
/// serialised state and must be merged via the registered `Mergeable`
/// (or the bootstrap-aware default in `merge_root_state`), not the
/// generic non-root LWW-by-HLC path. Treating the `Root<T>` entry as a
/// non-root entity routes the whole serialised state blob through
/// `apply_lww_winner`, which on a cold join silently discards the
/// remote's data whenever the joiner's just-materialised local `Root`
/// happens to carry a later HLC (which it usually does, since it was
/// constructed after the remote's writes).
#[inline]
pub fn is_app_root_entry(id: Id) -> bool {
    id.is_root() || id == ROOT_ENTRY_ID
}

impl<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> Collection<T, S> {
    /// Creates a new collection.
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    fn new(id: Option<Id>) -> Self {
        let id = id.unwrap_or_else(|| Id::random());

        let mut this = Self {
            children_ids: RefCell::new(None),
            storage: Element::new(Some(id)),
            _priv: PhantomData,
        };

        if id.is_root() {
            let _ignored = <Interface<S>>::save(&mut this).expect("save");
        } else {
            let _ = <Interface<S>>::add_child_to(*ROOT_ID, &mut this).expect("add child");
        }

        this
    }

    /// Creates a collection whose element is stamped `Shared{writers}` (and
    /// carries the given `crdt_type`) **before** its single registration with
    /// ROOT, so it emits exactly one `Add` action carrying the `Shared` domain —
    /// not an `Add(Public)` followed by an `Update(Shared)`.
    ///
    /// Used by [`SharedStorage`](super::SharedStorage) for its wrapper entity:
    /// stamping after `add_child_to` would emit two actions for the same entity
    /// in the bootstrap delta, and a bootstrap `Update(Shared)` over a freshly
    /// `Add`ed `Public` entity is a different (untested) merge path than a
    /// single `Add(Shared)`.
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    pub(crate) fn new_shared(
        id: Option<Id>,
        field_name: Option<&str>,
        crdt_type: CrdtType,
        writers: std::collections::BTreeSet<calimero_primitives::identity::PublicKey>,
    ) -> Self {
        let id = id.unwrap_or_else(|| Id::random());

        let mut storage = match field_name {
            Some(name) => Element::new_with_field_name_and_crdt_type(
                Some(id),
                Some(name.to_string()),
                crdt_type,
            ),
            None => {
                let mut element = Element::new(Some(id));
                element.metadata.crdt_type = Some(crdt_type);
                element
            }
        };
        storage.set_shared_domain(writers);

        let mut this = Self {
            children_ids: RefCell::new(None),
            storage,
            _priv: PhantomData,
        };

        if id.is_root() {
            let _ignored = <Interface<S>>::save(&mut this).expect("save");
        } else {
            let _ = <Interface<S>>::add_child_to(*ROOT_ID, &mut this).expect("add child");
        }

        this
    }

    /// Creates a detached collection that is NOT registered with the storage system.
    ///
    /// This is used for placeholder fields that are never actually used, such as
    /// GCounter's negative map which exists only to satisfy the type signature but
    /// is never persisted or read from storage.
    ///
    /// WARNING: Collections created with this method will NOT be synced across nodes.
    /// Only use this for truly inert placeholder fields.
    fn new_detached() -> Self {
        Self {
            children_ids: RefCell::new(Some(indexmap::IndexSet::new())),
            storage: Element::new(None), // Gets a random ID but won't be persisted
            _priv: PhantomData,
        }
        // Note: No Interface::save or add_child_to call - this collection is completely detached
    }

    /// Creates a new collection with deterministic ID, field name, and CRDT type.
    ///
    /// # Arguments
    /// * `parent_id` - The ID of the parent collection (None for root-level collections)
    /// * `field_name` - The name of the field containing this collection
    /// * `crdt_type` - The CRDT type for merge dispatch
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    pub(crate) fn new_with_field_name_and_crdt_type(
        parent_id: Option<Id>,
        field_name: &str,
        crdt_type: CrdtType,
    ) -> Self {
        let id = compute_collection_id(parent_id, field_name);

        let mut this = Self {
            children_ids: RefCell::new(None),
            storage: Element::new_with_field_name_and_crdt_type(
                Some(id),
                Some(field_name.to_string()),
                crdt_type,
            ),
            _priv: PhantomData,
        };

        if id.is_root() {
            let _ignored = <Interface<S>>::save(&mut this).expect("save");
        } else {
            let _ = <Interface<S>>::add_child_to(*ROOT_ID, &mut this).expect("add child");
        }

        this
    }

    /// Reassigns the collection's ID with a specific CRDT type.
    ///
    /// This method cleans up the old storage entry and parent-child references
    /// when moving from a random ID to a deterministic one.
    ///
    /// # Arguments
    /// * `field_name` - The name of the struct field containing this collection
    /// * `crdt_type` - The CRDT type for merge dispatch
    #[expect(clippy::expect_used, reason = "fatal error if cleanup fails")]
    pub(crate) fn reassign_deterministic_id_with_crdt_type(
        &mut self,
        field_name: &str,
        crdt_type: CrdtType,
    ) {
        self.reassign_deterministic_id_under(None, field_name, crdt_type);
    }

    /// Like [`reassign_deterministic_id_with_crdt_type`], but derives the new
    /// id relative to `parent_id` via `compute_collection_id(Some(parent), ..)`.
    ///
    /// Used when a collection is nested inside another entity (e.g. a `Counter`
    /// stored as a map value): the nested collection's id must be a function of
    /// the *parent entity's deterministic id* so every node mints the same id
    /// and the children converge. With `parent_id == None` this is exactly the
    /// top-level (ROOT-relative) reassignment.
    ///
    /// [`reassign_deterministic_id_with_crdt_type`]: Self::reassign_deterministic_id_with_crdt_type
    #[expect(clippy::expect_used, reason = "fatal error if cleanup fails")]
    pub(crate) fn reassign_deterministic_id_under(
        &mut self,
        parent_id: Option<Id>,
        field_name: &str,
        crdt_type: CrdtType,
    ) {
        let new_id = compute_collection_id(parent_id, field_name);
        let old_id = self.storage.id();

        // If already has the correct ID, nothing to do
        if old_id == new_id {
            return;
        }

        let old_metadata = self.storage.metadata.clone();

        // Clean up old storage entry and index
        let _ignored = S::storage_remove(Key::Entry(old_id));
        let _ignored = S::storage_remove(Key::Index(old_id));

        // Remove old child reference from ROOT (without creating tombstone)
        let _ = <Index<S>>::remove_child_reference_only(*ROOT_ID, old_id);

        // Broadcast a tombstone for the old id when re-keying a NESTED collection
        // (`parent_id.is_some()`). Such a collection was created on-demand with a
        // random id (`Collection::new(None)`) and its `Add` was already pushed to
        // the delta; without a matching `DeleteRef` a receiver applies that `Add`
        // but never the local `storage_remove` above, so it keeps the old-id
        // entity as an orphan and its parent's `full_hash` diverges from the
        // writer's (the scaffolding-e2e PN-counter "Wait for sync" timeout).
        // Top-level init re-keys (`parent_id == None`) run before the state is
        // broadcast, so the old id was never shipped — no tombstone needed there.
        if parent_id.is_some() && S::participates_in_sync() {
            crate::delta::push_action(crate::action::Action::DeleteRef {
                id: old_id,
                deleted_at: crate::env::time_now(),
                metadata: old_metadata,
            });
        }

        // Update in-memory ID, field name, and CRDT type
        self.storage.reassign_id_and_field_name(new_id, field_name);
        self.storage.metadata.crdt_type = Some(crdt_type);

        // Add the collection with new ID to ROOT
        let _ = <Interface<S>>::add_child_to(*ROOT_ID, self)
            .expect("failed to add collection with new ID");
    }

    /// Reassigns this collection's id to the deterministic field-name id
    /// **and** re-keys every child entry to an index-derived deterministic
    /// id, preserving each entry's [`StorageType`].
    ///
    /// `Vector` (and `AuthoredVector`, which wraps it) inserts children with
    /// `Id::random()` at `push` time — concurrent appends from different
    /// replicas must not collide, so a content-derived key is impossible.
    /// That is correct for live operation (the random id rides along in the
    /// sync delta, so every peer agrees), but it breaks migrations: a
    /// `#[app::migrate]` re-runs independently on every node and emits no
    /// delta (the LazyOnAccess model), so two replicas building the same
    /// vector from byte-identical v1 state would otherwise mint *different*
    /// random element ids and diverge. Re-keying by append index makes the
    /// element ids a pure function of position, restoring CIP Invariant I9.
    ///
    /// Unlike the map/set path (entries are already keyed by `compute_id`
    /// at insert, so their reassign can early-return once the collection id
    /// is correct), this always re-keys the children: the *children* are
    /// what carry the random ids, independent of the collection's own id.
    #[expect(clippy::expect_used, reason = "fatal error if migration fails")]
    pub(crate) fn reassign_deterministic_id_with_indexed_children(
        &mut self,
        field_name: &str,
        crdt_type: CrdtType,
    ) {
        self.reassign_deterministic_id_with_indexed_children_under(None, field_name, crdt_type);
    }

    /// Parent-relative variant of
    /// [`reassign_deterministic_id_with_indexed_children`] for a vector nested
    /// inside another entity (re-keys the collection id and its index-derived
    /// children relative to `parent_id`).
    ///
    /// [`reassign_deterministic_id_with_indexed_children`]: Self::reassign_deterministic_id_with_indexed_children
    #[expect(clippy::expect_used, reason = "fatal error if migration fails")]
    pub(crate) fn reassign_deterministic_id_with_indexed_children_under(
        &mut self,
        parent_id: Option<Id>,
        field_name: &str,
        crdt_type: CrdtType,
    ) {
        // Snapshot (value, storage_type) for every child in append order
        // before mutating anything. `storage_type` carries the
        // `AuthoredVector` per-entry owner stamp, which must survive the
        // re-key or owner authorization would be lost.
        let ordered_ids: Vec<Id> = self
            .children_cache()
            .expect("read children for reindex")
            .iter()
            .copied()
            .collect();
        let mut snapshot: Vec<(T, StorageType)> = Vec::with_capacity(ordered_ids.len());
        for id in ordered_ids {
            let entry = <Interface<S>>::find_by_id::<Entry<T>>(id)
                .expect("read child entry for reindex")
                .expect("vector child entry must exist");
            snapshot.push((entry.item, entry.storage.metadata.storage_type));
        }

        // Nothing materialised to re-key: only relocate the collection's own
        // id (the plain reassign, which no-ops when the id already matches).
        // Crucially we do NOT fall through to the destructive clear+reinsert
        // below — so even if `children_cache()` ever returned an
        // under-populated set (e.g. a borsh-deserialised collection whose
        // index lookup transiently missed), we never clear children we
        // didn't snapshot, and an empty vector stays empty. The clear path
        // only runs when we hold a non-empty snapshot to restore from.
        if snapshot.is_empty() {
            self.reassign_deterministic_id_under(parent_id, field_name, crdt_type);
            return;
        }

        // Drop the old random-id children, move the collection to its
        // deterministic id, then re-insert each child under
        // `compute_id(parent, index)`.
        self.clear().expect("clear for reindex");
        self.reassign_deterministic_id_under(parent_id, field_name, crdt_type);

        let parent = self.id();
        for (index, (item, storage_type)) in snapshot.into_iter().enumerate() {
            let id = compute_id(parent, &(index as u64).to_le_bytes());
            let _reinserted = self
                .insert_with_storage_type(Some(id), item, storage_type)
                .expect("re-insert vector child during reindex");
        }
    }

    /// Inserts an item into the collection.
    fn insert(&mut self, id: Option<Id>, item: T) -> StoreResult<T> {
        // Entries inherit this collection's own storage domain. For an ordinary
        // collection the element is `Public`, so this is the previous default;
        // when the element carries `Shared{writers}` (a guarded collection) every
        // entry is stamped with that writer set. This is the chokepoint for the
        // `Entry`/`or_default` write-back path and for collections that insert via
        // the bare `Collection::insert` (sets, vectors, RGA), so guarding a
        // collection covers those paths too — not only the direct `map.insert`.
        let inherited = self.storage.metadata.storage_type.clone();
        self.insert_with_storage_type(id, item, inherited)
    }

    /// Inserts an item with a caller-provided fixed `id` and `field_name`,
    /// but no `crdt_type`. Used by containers whose merge semantics are
    /// dispatched out-of-band (i.e. not by the generic LWW/G-counter path
    /// on the entry's `crdt_type`) — currently only `Root<T>`.
    ///
    /// `id` is a plain `Id` (not `Option<Id>`) so the contract is enforced
    /// at the type level: this method *requires* a caller-provided fixed
    /// id. (`insert_with_storage_type` keeps `Option<Id>` because there
    /// `None` is a meaningful "let storage pick".)
    pub(crate) fn insert_with_field_name(
        &mut self,
        id: Id,
        item: T,
        field_name: &str,
    ) -> StoreResult<T> {
        let mut collection = CollectionMut::new(self);

        let mut entry = Entry {
            item,
            storage: Element::new_with_field_name(Some(id), Some(field_name.to_string())),
        };

        collection.insert(&mut entry)?;

        Ok(entry.item)
    }

    /// Inserts an item into the collection with a specific StorageType.
    pub(crate) fn insert_with_storage_type(
        &mut self,
        id: Option<Id>,
        item: T,
        storage_type: StorageType,
    ) -> StoreResult<T> {
        let mut collection = CollectionMut::new(self);

        let mut entry = Entry {
            item,
            storage: Element::new(id),
        };
        // Update the `StorageType`.
        entry.storage.metadata.storage_type = storage_type;

        collection.insert(&mut entry)?;

        Ok(entry.item)
    }

    #[inline(never)]
    fn get(&self, id: Id) -> StoreResult<Option<T>> {
        let entry = <Interface<S>>::find_by_id::<Entry<_>>(id)?;

        Ok(entry.map(|entry| entry.item))
    }

    fn contains(&self, id: Id) -> StoreResult<bool> {
        Ok(self.children_cache()?.contains(&id))
    }

    fn get_mut(&mut self, id: Id) -> StoreResult<Option<EntryMut<'_, T, S>>> {
        let entry = <Interface<S>>::find_by_id::<Entry<_>>(id)?;

        Ok(entry.map(|entry| EntryMut {
            collection: CollectionMut::new(self),
            entry,
            removed: false,
        }))
    }

    fn len(&self) -> StoreResult<usize> {
        Ok(self.children_cache()?.len())
    }

    fn entries(
        &self,
    ) -> StoreResult<impl ExactSizeIterator<Item = StoreResult<T>> + DoubleEndedIterator + '_> {
        let iter = self.children_cache()?.iter().copied().map(|child| {
            let entry = <Interface<S>>::find_by_id::<Entry<_>>(child)?
                .ok_or(StoreError::StorageError(StorageError::NotFound(child)))?;

            Ok(entry.item)
        });

        Ok(iter)
    }

    fn nth(&self, index: usize) -> StoreResult<Option<Id>> {
        Ok(self.children_cache()?.get_index(index).copied())
    }

    fn last(&self) -> StoreResult<Option<Id>> {
        Ok(self.children_cache()?.last().copied())
    }

    fn clear(&mut self) -> StoreResult<()> {
        let mut collection = CollectionMut::new(self);

        collection.clear()?;

        Ok(())
    }

    #[expect(
        clippy::unwrap_in_result,
        clippy::expect_used,
        clippy::mut_from_ref,
        reason = "fatal error if it happens"
    )]
    fn children_cache(&self) -> StoreResult<&mut IndexSet<Id>> {
        let mut cache = self.children_ids.borrow_mut();

        if cache.is_none() {
            // Try to load children from index
            // After CRDT sync, newly created collections might not have index entries yet
            // In that case, start with an empty set
            let children: IndexSet<Id> = match <Interface<S>>::child_info_for(self.id()) {
                Ok(info) => info.into_iter().map(|c| c.id()).collect(),
                Err(StorageError::IndexNotFound(_)) => {
                    // Collection was just created/synced, no children yet
                    IndexSet::new()
                }
                Err(e) => return Err(StoreError::StorageError(e)),
            };

            *cache = Some(children);
        }

        let children = cache.as_mut().expect("children");

        #[expect(unsafe_code, reason = "necessary for caching")]
        let children = unsafe { &mut *ptr::from_mut(children) };

        Ok(children)
    }
}

#[derive(Debug)]
struct EntryMut<'a, T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> {
    collection: CollectionMut<'a, T, S>,
    entry: Entry<T>,
    /// Flag to prevent saving on drop when the entry has been removed.
    /// When `remove()` is called, this is set to true to prevent the Drop impl
    /// from generating an Update action for the deleted entity.
    removed: bool,
}

impl<T, S> EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// The storage id of this entry. Used by the map's occupied-entry replace
    /// path to re-key nested collections in a replacement value relative to the
    /// (stable) entry id.
    fn id(&self) -> Id {
        self.entry.id()
    }

    fn remove(mut self) -> StoreResult<T> {
        let old = self
            .collection
            .get(self.entry.id())?
            .ok_or(StoreError::StorageError(StorageError::NotFound(
                self.entry.id(),
            )))?;

        let _ = <Interface<S>>::remove_child_from(self.collection.id(), self.entry.id())?;

        let _ = self
            .collection
            .children_cache()?
            .shift_remove(&self.entry.id());

        // Mark as removed to prevent Drop from creating an Update action
        // for this deleted entity
        self.removed = true;

        Ok(old)
    }
}

impl<T, S> Deref for EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.entry.item
    }
}

impl<T, S: StorageAdaptor> DerefMut for EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry.item
    }
}

impl<T, S> Drop for EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn drop(&mut self) {
        // Don't save if the entry was removed - the DeleteRef action has
        // already been created, and saving would create a conflicting Update action
        if self.removed {
            return;
        }
        self.entry.element_mut().update();
        let _ignored = <Interface<S>>::save(&mut self.entry);
    }
}

#[derive(Debug)]
struct CollectionMut<'a, T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> {
    collection: &'a mut Collection<T, S>,
}

impl<'a, T, S> CollectionMut<'a, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn new(collection: &'a mut Collection<T, S>) -> Self {
        Self { collection }
    }

    fn insert(&mut self, item: &mut Entry<T>) -> StoreResult<()> {
        let _ = <Interface<S>>::add_child_to(self.collection.id(), item)?;

        let _ignored = self.collection.children_cache()?.insert(item.id());

        Ok(())
    }

    fn clear(&mut self) -> StoreResult<()> {
        let children = self.collection.children_cache()?;

        for child in children.drain(..) {
            let _ = <Interface<S>>::remove_child_from(self.collection.id(), child)?;
        }

        Ok(())
    }
}

impl<T, S> Deref for CollectionMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Target = Collection<T, S>;

    fn deref(&self) -> &Self::Target {
        self.collection
    }
}

impl<T, S> DerefMut for CollectionMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.collection
    }
}

impl<T, S> Drop for CollectionMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn drop(&mut self) {
        self.collection.element_mut().update();
    }
}

impl<T, S: StorageAdaptor> fmt::Debug for Collection<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Collection")
            .field("element", &self.storage)
            .finish()
    }
}

impl<T, S> Default for Collection<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
        Self::new(None)
    }
}

impl<T: Eq + BorshSerialize + BorshDeserialize, S: StorageAdaptor> Eq for Collection<T, S> {}

impl<T: PartialEq + BorshSerialize + BorshDeserialize, S: StorageAdaptor> PartialEq
    for Collection<T, S>
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn eq(&self, other: &Self) -> bool {
        let l = self.entries().unwrap().flatten();
        let r = other.entries().unwrap().flatten();

        l.eq(r)
    }
}

impl<T: Ord + BorshSerialize + BorshDeserialize, S: StorageAdaptor> Ord for Collection<T, S> {
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let l = self.entries().unwrap().flatten();
        let r = other.entries().unwrap().flatten();

        l.cmp(r)
    }
}

impl<T: PartialOrd + BorshSerialize + BorshDeserialize, S: StorageAdaptor> PartialOrd
    for Collection<T, S>
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let l = self.entries().ok()?.flatten();
        let r = other.entries().ok()?.flatten();

        l.partial_cmp(r)
    }
}

impl<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> Extend<(Option<Id>, T)>
    for Collection<T, S>
{
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    fn extend<I: IntoIterator<Item = (Option<Id>, T)>>(&mut self, iter: I) {
        let mut collection = CollectionMut::new(self);

        for (id, item) in iter {
            let mut entry = Entry {
                item,
                storage: Element::new(id),
            };

            collection
                .insert(&mut entry)
                .expect("collection extension failed");
        }
    }
}

impl<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> FromIterator<(Option<Id>, T)>
    for Collection<T, S>
{
    fn from_iter<I: IntoIterator<Item = (Option<Id>, T)>>(iter: I) -> Self {
        let mut collection = Collection::new(None);
        collection.extend(iter);
        collection
    }
}
