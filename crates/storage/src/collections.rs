//! High-level data structures for storage.

use core::cell::{RefCell, RefMut};
use core::cmp::Ordering;
use core::fmt;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use std::collections::BTreeMap;
use std::sync::LazyLock;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
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
pub mod fugue;
pub use fugue::{FugueError, FugueNode, FugueTree, Side};
pub mod fugue_text;
pub use fugue_text::FugueText;
pub mod mark_schema;
pub use mark_schema::{DefaultMarks, Expand, MarkSchema};
pub mod rich_text;
pub use rich_text::{AttrRun, Attrs, DeltaOp, DeltaUndo, Mark, MarkId, RichText, Span, UndoStep};
pub mod rich_document;
pub use rich_document::{Block, BlockId, BlockView, RichDocument};
pub mod lww_register;
pub use lww_register::LwwRegister;
pub mod tee_secret;
pub use tee_secret::{Sealed, TeeSecret};
pub mod blob_ref;
pub use blob_ref::BlobRef;
pub mod crdt_meta;
pub use crdt_meta::{
    CrdtMeta, CrdtType, Decomposable, MergeStrategy, Mergeable, StorageKey, StorageStrategy,
};
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
#[cfg(not(target_arch = "wasm32"))]
mod abi_type;
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
/// Sync-merge drop accounting, for harnesses that must not silently assert
/// nothing when every delta is refused.
#[cfg(any(test, feature = "testing"))]
pub use root::{dropped_action_count, reset_dropped_action_count};
pub mod error;
pub use error::StoreError;

pub mod user;
pub use user::UserStorage;
pub mod shared;
pub use shared::WriterSetCell;
pub mod permissioned;
pub use permissioned::{
    Authorizer, Op, Ownable, OwnerAcl, PermissionedStorage, ProtocolAuthorizer, SharedStorage,
    TeeAuthorityAcl, TeeOnly, WriterSetAcl,
};
pub mod access_control;
pub use access_control::AccessControl;
mod authored_common;
pub mod guarded;
pub use guarded::{
    Authored, ContentAddressed, ContentHash, Editable, Edits, Guarded, GuardedEntries, GuardedKeys,
    Moderated, ModeratedOnce, Moderation, Once, Owner, OwnerEdits, OwnerOnce, Owning, Policy,
    WriteOnce,
};
pub mod authored_map;
pub use authored_map::AuthoredMap;
pub mod authored_sorted_map;
pub use authored_sorted_map::AuthoredSortedMap;
pub mod authored_vector;
pub use authored_vector::AuthoredVector;
pub mod indexed_map;
pub use indexed_map::{IndexValue, Indexed, IndexedMap};
pub mod frozen;
pub use frozen::FrozenStorage;
pub mod frozen_cell;
pub use frozen_cell::Frozen;
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
#[must_use = "this is a read-only copy from storage; dropping it makes the `get` a no-op. \
              To mutate and persist use `get_mut`/`entry().or_default()`; to keep a copy bind it"]
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

/// Domain separator for the id of a `TeeOnly` state field.
const DOMAIN_SEPARATOR_TEE_ONLY: &[u8] = b"__calimero_tee_only__";

/// The first bytes of every id in a `TeeOnly` cell's subtree.
///
/// A `TeeOnly` cell stores nothing until the TEE's first write, so until then
/// anyone may create an entity at its id, or at an id beneath it, and the TEE's
/// write there is refused. Merge must be able to tell such an id from the id
/// alone, since the entity at it claims whatever it likes about itself. Any
/// other id starts with these bytes with probability 2^-64.
const TEE_ONLY_ID_TAG: [u8; 8] = *b"\xCAtee\x00nly";

/// Whether `id` lies in a `TeeOnly` cell's subtree: the cell's own id, or one
/// derived beneath it by [`compute_id`] or [`compute_collection_id`].
pub(crate) fn is_tee_only_id(id: Id) -> bool {
    id.as_bytes().starts_with(&TEE_ONLY_ID_TAG)
}

fn tee_only(hash: [u8; 32]) -> Id {
    let mut bytes = hash;
    bytes[..TEE_ONLY_ID_TAG.len()].copy_from_slice(&TEE_ONLY_ID_TAG);
    Id::new(bytes)
}

/// `hash` as an id, marked TEE-only when `parent` is. Beneath a cell's value it
/// takes `cell_tag` and the parent's binding to the cell's anchor instead.
fn derived_id(parent: Option<Id>, hash: [u8; 32], cell_tag: [u8; 8]) -> Id {
    match parent {
        Some(parent) if is_tee_only_id(parent) => tee_only(hash),
        Some(parent) if is_cell_bound_id(parent) => {
            let mut bytes = hash;
            bytes[..cell_tag.len()].copy_from_slice(&cell_tag);
            bytes[cell_tag.len()..CELL_BOUND_LEN]
                .copy_from_slice(&parent.as_bytes()[cell_tag.len()..CELL_BOUND_LEN]);
            Id::new(bytes)
        }
        _ => Id::new(hash),
    }
}

/// Domain separators for the ids of a `SharedStorage` cell.
const DOMAIN_SEPARATOR_CELL_FIELD: &[u8] = b"__calimero_cell_field__";
const DOMAIN_SEPARATOR_CELL_WRITERS: &[u8] = b"__calimero_cell_writers__";
const DOMAIN_SEPARATOR_CELL_ANCHOR: &[u8] = b"__calimero_cell_anchor__";
const DOMAIN_SEPARATOR_CELL_VALUE: &[u8] = b"__calimero_cell_value__";

/// The first bytes of a field-derived `SharedStorage` cell's wrapper id.
///
/// The wrapper holds the cell's writer set, and a node checks every later write
/// against the set it stored first. A field-derived id is predictable, so the
/// rest of it is a hash of the field and of the writer set the cell was created
/// with: merge refuses a first write there whose writer set hashes otherwise,
/// and a set anyone else names lands at an id no state refers to.
const CELL_ID_TAG: [u8; 8] = *b"\xCAcel\x00lid";

/// The first bytes of a `SharedStorage` cell's value id, and of every entry id
/// beneath it. The next [`CELL_BINDING_LEN`] bytes bind the id to the cell's
/// anchor, so only a `SharedMember` of that anchor may be stored there.
///
/// A member never changes its anchor and an entity never changes its storage
/// type, so without the binding a member of another anchor, or any other
/// entity, could take the value's id, or an entry's under it, before the cell's
/// own write reaches a node. Binding the id lets merge tell from the id alone
/// which anchor may hold it. A forger must find another anchor whose binding
/// matches: a 96-bit second preimage.
const CELL_ENTRY_ID_TAG: [u8; 8] = *b"\xCAshr\x00ent";

/// [`CELL_ENTRY_ID_TAG`] for a collection id beneath a cell's value, bound the
/// same way. A collection's own entity carries no stamp of its cell (it is
/// `Public`, its bytes only its id), so a `Public` entity may be stored there
/// too; its entries still take [`CELL_ENTRY_ID_TAG`].
const CELL_COLLECTION_ID_TAG: [u8; 8] = *b"\xCAshr\x00col";

/// Bytes of a cell id that are a hash binding it, after the tag.
const CELL_BINDING_LEN: usize = 12;

/// The tag and the binding: what every id beneath a cell's value shares.
const CELL_BOUND_LEN: usize = CELL_ENTRY_ID_TAG.len() + CELL_BINDING_LEN;

fn truncated_hash(parts: &[&[u8]]) -> [u8; CELL_BINDING_LEN] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let hash: [u8; 32] = hasher.finalize().into();
    let mut out = [0; CELL_BINDING_LEN];
    out.copy_from_slice(&hash[..CELL_BINDING_LEN]);
    out
}

#[expect(clippy::expect_used, reason = "serializing into a Vec cannot fail")]
fn writers_binding(
    prefix: &[u8],
    writers: &BTreeMap<calimero_account::AccountId, crate::entities::OpMask>,
) -> [u8; CELL_BINDING_LEN] {
    let writers = borsh::to_vec(writers).expect("serialize writer set");
    truncated_hash(&[DOMAIN_SEPARATOR_CELL_WRITERS, prefix, &writers])
}

/// The wrapper id of the cell at `field_id` created with `writers`.
pub(crate) fn cell_id(
    field_id: Id,
    writers: &BTreeMap<calimero_account::AccountId, crate::entities::OpMask>,
) -> Id {
    let mut bytes = [0; 32];
    bytes[..CELL_ID_TAG.len()].copy_from_slice(&CELL_ID_TAG);
    bytes[CELL_ID_TAG.len()..CELL_BOUND_LEN].copy_from_slice(&truncated_hash(&[
        DOMAIN_SEPARATOR_CELL_FIELD,
        field_id.as_bytes(),
    ]));
    let binding = writers_binding(&bytes[..CELL_BOUND_LEN], writers);
    bytes[CELL_BOUND_LEN..].copy_from_slice(&binding);
    Id::new(bytes)
}

/// Whether `id` is a field-derived cell's wrapper id.
pub(crate) fn is_cell_id(id: Id) -> bool {
    id.as_bytes().starts_with(&CELL_ID_TAG)
}

/// Whether `id` is the wrapper id of a cell created with `writers`.
pub(crate) fn cell_id_binds(
    id: Id,
    writers: &BTreeMap<calimero_account::AccountId, crate::entities::OpMask>,
) -> bool {
    let bytes = id.as_bytes();
    is_cell_id(id) && bytes[CELL_BOUND_LEN..] == writers_binding(&bytes[..CELL_BOUND_LEN], writers)
}

/// The id of the value of the cell anchored at `anchor`. A `TeeOnly` cell's
/// value keeps the TEE-only mark, whose own rule already reserves it.
pub(crate) fn cell_value_id(anchor: Id) -> Id {
    if is_tee_only_id(anchor) {
        return compute_id(anchor, shared::VALUE_KEY);
    }
    let mut bytes = [0; 32];
    bytes[..CELL_ENTRY_ID_TAG.len()].copy_from_slice(&CELL_ENTRY_ID_TAG);
    bytes[CELL_ENTRY_ID_TAG.len()..CELL_BOUND_LEN].copy_from_slice(&anchor_binding(anchor));
    bytes[CELL_BOUND_LEN..].copy_from_slice(&truncated_hash(&[
        DOMAIN_SEPARATOR_CELL_VALUE,
        anchor.as_bytes(),
    ]));
    Id::new(bytes)
}

fn anchor_binding(anchor: Id) -> [u8; CELL_BINDING_LEN] {
    truncated_hash(&[DOMAIN_SEPARATOR_CELL_ANCHOR, anchor.as_bytes()])
}

/// Whether `id` lies in a cell's value subtree: the value's id, or one derived
/// beneath it by [`compute_id`] or [`compute_collection_id`].
pub(crate) fn is_cell_bound_id(id: Id) -> bool {
    is_cell_collection_id(id) || id.as_bytes().starts_with(&CELL_ENTRY_ID_TAG)
}

/// Whether `id` is a collection's id beneath a cell's value.
pub(crate) fn is_cell_collection_id(id: Id) -> bool {
    id.as_bytes().starts_with(&CELL_COLLECTION_ID_TAG)
}

/// Whether `id` lies in the value subtree of the cell anchored at `anchor`.
pub(crate) fn cell_bound_id_binds(id: Id, anchor: Id) -> bool {
    is_cell_bound_id(id)
        && id.as_bytes()[CELL_ENTRY_ID_TAG.len()..CELL_BOUND_LEN] == anchor_binding(anchor)
}

/// Whether an entity stamped `stamp` may live at `id` as far as cells go: a
/// `Shared` wrapper only at a cell's wrapper id, a `SharedMember` only at an id
/// bound to its anchor, and either at a TEE-only id, whose own rule decides.
/// Any other stamp may live at an id no cell derived.
pub(crate) fn shared_stamp_fits(id: Id, stamp: &StorageType) -> bool {
    match stamp {
        StorageType::Shared { .. } => is_cell_id(id) || is_tee_only_id(id),
        StorageType::SharedMember { anchor, .. } => {
            cell_bound_id_binds(id, *anchor) || is_tee_only_id(id)
        }
        StorageType::Public | StorageType::Frozen | StorageType::User { .. } => true,
    }
}

/// The id of a `TeeOnly` state field named `field_name`.
pub(crate) fn tee_only_id(field_name: &str) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEPARATOR_TEE_ONLY);
    hasher.update(field_name.as_bytes());
    tee_only(hasher.finalize().into())
}

/// Compute the ID for a key in a map.
/// Uses domain separation to prevent collision with collection IDs.
/// An entry of a TEE-only parent is TEE-only too, and one beneath a cell's
/// value is bound to that cell.
pub(crate) fn compute_id(parent: Id, key: &[u8]) -> Id {
    derived_id(Some(parent), entry_hash(parent, key), CELL_ENTRY_ID_TAG)
}

/// A fresh random id for an entry of `parent`, marked as [`compute_id`] marks
/// one: TEE-only beneath a TEE-only parent and bound to the cell beneath a
/// cell's value, so an entry a vector pushes into a cell lives where merge
/// lets a member of that cell live.
pub(crate) fn random_entry_id(parent: Id) -> Id {
    derived_id(Some(parent), *Id::random().as_bytes(), CELL_ENTRY_ID_TAG)
}

/// [`compute_id`] without the TEE-only mark, for book-keeping that sits
/// beside a cell's value rather than in it: a `Shared` anchor's rotation log,
/// which is not the TEE's to write.
pub(crate) fn compute_unmarked_id(parent: Id, key: &[u8]) -> Id {
    Id::new(entry_hash(parent, key))
}

fn entry_hash(parent: Id, key: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(parent.as_bytes());
    hasher.update(DOMAIN_SEPARATOR_ENTRY);
    hasher.update(key);
    hasher.finalize().into()
}

/// Compute a deterministic collection ID from parent ID and field name.
/// This ensures the same collection gets the same ID across all nodes.
/// Uses domain separation to prevent collision with map entry IDs.
/// A collection nested in a TEE-only parent is TEE-only too, and one beneath a
/// cell's value is bound to that cell.
pub(crate) fn compute_collection_id(parent_id: Option<Id>, field_name: &str) -> Id {
    let mut hasher = Sha256::new();
    if let Some(parent) = parent_id {
        hasher.update(parent.as_bytes());
    }
    hasher.update(DOMAIN_SEPARATOR_COLLECTION);
    hasher.update(field_name.as_bytes());
    derived_id(parent_id, hasher.finalize().into(), CELL_COLLECTION_ID_TAG)
}

/// Domain separator for the owner-bound half of an owned entry's id.
const DOMAIN_SEPARATOR_OWNED: &[u8] = b"__calimero_owned_entry__";

/// Bytes of the slot an owned entry's id keeps: its bucket in the parent's child
/// trie, and every mark (the TEE-only tag) a slot carries.
const OWNED_SLOT_PREFIX_LEN: usize = 12;

/// The bytes after the slot prefix of every owned entry's id.
///
/// An entity at an owner-derived id that is not the owner's own entry would
/// refuse the owner's write for good, since a stored `StorageType` never
/// changes. Merge refuses any other entity at a tagged id, which it can only do
/// if the id alone says so. Any other id carries these bytes with probability
/// 2^-64.
const OWNED_ID_TAG: [u8; 8] = *b"\xCAowned\x00\x00";

// The slot prefix must cover the nibbles the child trie buckets by, so the
// entries of one key under every owner share a bucket.
const _: () = assert!(OWNED_SLOT_PREFIX_LEN * 2 >= crate::child_trie::DEPTH);
const _: () = assert!(OWNED_SLOT_PREFIX_LEN + OWNED_ID_TAG.len() <= 32);

/// The id `owner`'s entry at `slot` lives at: the slot's first bytes, the owned
/// tag, then a hash of those bytes and the owner.
///
/// Two owners writing one key therefore write two entities, and an id says
/// whose entry it is: [`owned_id_binds`] checks it from the id and the stamp
/// alone. Only the slot prefix is hashed, so the function is idempotent and an
/// owned id is its own slot.
pub(crate) fn owned_entry_id(slot: Id, owner: &AccountId) -> Id {
    let prefix = &slot.as_bytes()[..OWNED_SLOT_PREFIX_LEN];
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEPARATOR_OWNED);
    hasher.update(prefix);
    hasher.update(owner.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();

    let mut bytes = [0; 32];
    let tag_end = OWNED_SLOT_PREFIX_LEN + OWNED_ID_TAG.len();
    bytes[..OWNED_SLOT_PREFIX_LEN].copy_from_slice(prefix);
    bytes[OWNED_SLOT_PREFIX_LEN..tag_end].copy_from_slice(&OWNED_ID_TAG);
    bytes[tag_end..].copy_from_slice(&digest[..32 - tag_end]);
    Id::new(bytes)
}

/// Whether `id` carries the owned tag, so only an owned entry may live there.
pub(crate) fn is_owned_id(id: Id) -> bool {
    id.as_bytes()[OWNED_SLOT_PREFIX_LEN..OWNED_SLOT_PREFIX_LEN + OWNED_ID_TAG.len()] == OWNED_ID_TAG
}

/// Whether `id` is `owner`'s entry id for the slot it names.
pub(crate) fn owned_id_binds(id: Id, owner: &AccountId) -> bool {
    is_owned_id(id) && owned_entry_id(id, owner) == id
}

/// Whether `a` and `b` share the slot prefix an owned id keeps.
fn same_slot(a: Id, b: Id) -> bool {
    a.as_bytes()[..OWNED_SLOT_PREFIX_LEN] == b.as_bytes()[..OWNED_SLOT_PREFIX_LEN]
}

/// The id an entry written at `slot` with `stamp` is stored under: an owned
/// entry's is bound to its owner, any other's is the slot.
pub(crate) fn stored_id(slot: Id, stamp: &StorageType) -> Id {
    match stamp {
        StorageType::User { owner, .. } => owned_entry_id(slot, owner),
        _ => slot,
    }
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
#[expect(
    dead_code,
    reason = "Never constructed: the map reaches its children through `Interface` \
              directly. Retained as the declaration of a map's child layout and as \
              the only in-tree user of `#[derive(Collection)]`, which would other- \
              wise go unexercised by this build."
)]
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

/// Decode the stored VALUE bytes of a rotation-log map child back into the
/// [`RotationLogEntry`](crate::rotation_log::RotationLogEntry) it holds (P3 of
/// core#2716).
///
/// A rotation-log map child is an ordinary [`UnorderedMap`] entry, so its stored
/// value is `borsh(Entry<(RotationLogEntry, [u8; 32])>)` — NOT a bare
/// `RotationLog` blob. The node-side direct reader (`delta_store`) reads child
/// bytes straight from RocksDB outside a storage env, so it cannot go through
/// the `UnorderedMap` handle; this exposes the exact borsh layout the collection
/// writes (`item` then the `#[storage]` `Element`, whose only serialized field
/// is its id). Returns `None` if the bytes are not a valid map entry.
///
/// Note the tuple is VALUE-first: a map entry stores `(V, K)` so the value sits
/// at offset 0 whatever the key type. This is the one reader that spells the
/// layout out, so it moves in lockstep with
/// [`UnorderedMap::inner`](unordered_map::UnorderedMap).
pub fn decode_rotation_log_entry_child(
    bytes: &[u8],
) -> Option<crate::rotation_log::RotationLogEntry> {
    borsh::from_slice::<Entry<(crate::rotation_log::RotationLogEntry, [u8; 32])>>(bytes)
        .ok()
        .map(|entry| entry.item.0)
}

#[expect(unused_qualifications, reason = "AtomicUnit macro is unsanitized")]
type StoreResult<T> = std::result::Result<T, StoreError>;

static ROOT_ID: LazyLock<Id> = LazyLock::new(Id::root);

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
        let id = id.unwrap_or_else(Id::random);

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
    pub(crate) fn new_shared(
        id: Option<Id>,
        field_name: Option<&str>,
        crdt_type: CrdtType,
        writers: std::collections::BTreeSet<calimero_account::AccountId>,
    ) -> Self {
        Self::new_shared_scoped(
            id,
            field_name,
            crdt_type,
            crate::entities::full_mask(writers),
        )
    }

    /// [`new_shared`](Self::new_shared) with explicit per-writer masks.
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    pub(crate) fn new_shared_scoped(
        id: Option<Id>,
        field_name: Option<&str>,
        crdt_type: CrdtType,
        writers: std::collections::BTreeMap<calimero_account::AccountId, crate::entities::OpMask>,
    ) -> Self {
        let id = id.unwrap_or_else(Id::random);

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
        storage.set_shared_domain_scoped(writers);

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

    /// Open a *handle* to a collection that already lives in storage at a known
    /// `id`, WITHOUT creating or re-registering it.
    ///
    /// Unlike [`new`](Self::new) / `new_with_field_name_*`, this does NOT call
    /// `add_child_to(ROOT, ..)` — the caller owns the parent linkage (e.g. the
    /// rotation-log map is a child of its `Shared` anchor, not of ROOT). Unlike
    /// [`new_detached`](Self::new_detached), `children_ids` is left `None` so the
    /// first access lazily loads the existing children from the index — a
    /// detached collection pre-seeds an EMPTY child set and would therefore read
    /// back as empty even when the entity has children on disk.
    ///
    /// The element is stamped with `crdt_type` (matching how the entity was
    /// created) and marked clean (`is_dirty = false`): opening must not, on its
    /// own, re-emit an `Add` for an entity that already exists.
    fn open_existing(id: Id, crdt_type: CrdtType) -> Self {
        let mut storage = Element::new(Some(id));
        storage.metadata.crdt_type = Some(crdt_type);
        storage.is_dirty = false;

        Self {
            children_ids: RefCell::new(None),
            storage,
            _priv: PhantomData,
        }
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
    pub(crate) fn reassign_deterministic_id_under(
        &mut self,
        parent_id: Option<Id>,
        field_name: &str,
        crdt_type: CrdtType,
    ) {
        if parent_id.is_some() {
            self.adopt_ambient_domain();
        }
        let new_id = compute_collection_id(parent_id, field_name);
        self.reassign_id(parent_id, new_id, field_name, crdt_type);
    }

    /// Like [`reassign_deterministic_id_with_crdt_type`], to an id the caller
    /// derived: a `TeeOnly` field's [`tee_only_id`] rather than its plain
    /// field-name id.
    ///
    /// [`reassign_deterministic_id_with_crdt_type`]: Self::reassign_deterministic_id_with_crdt_type
    pub(crate) fn reassign_top_level_id(
        &mut self,
        new_id: Id,
        field_name: &str,
        crdt_type: CrdtType,
    ) {
        self.reassign_id(None, new_id, field_name, crdt_type);
    }

    #[expect(clippy::expect_used, reason = "fatal error if cleanup fails")]
    fn reassign_id(
        &mut self,
        parent_id: Option<Id>,
        new_id: Id,
        field_name: &str,
        crdt_type: CrdtType,
    ) {
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
    /// `#[app::migrate]` re-runs independently on every node without emitting
    /// sync deltas, so two replicas building the same vector from
    /// byte-identical v1 state would otherwise mint *different* random
    /// element ids and diverge. Re-keying by append index makes the element
    /// ids a pure function of position, restoring CIP Invariant I9.
    ///
    /// Unlike the map/set path (entries are already keyed by `compute_id`
    /// at insert, so their reassign can early-return once the collection id
    /// is correct), this always re-keys the children: the *children* are
    /// what carry the random ids, independent of the collection's own id.
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
        // `compute_id(parent, index)`. Uses the re-key clear so `Frozen`
        // children are relocated (re-inserted below) rather than rejected.
        self.clear_for_rekey().expect("clear for reindex");
        self.reassign_deterministic_id_under(parent_id, field_name, crdt_type);

        let parent = self.id();
        for (index, (item, storage_type)) in snapshot.into_iter().enumerate() {
            let id = compute_id(parent, &(index as u64).to_le_bytes());
            let _reinserted = self
                .insert_with_storage_type(Some(id), item, storage_type, None)
                .expect("re-insert vector child during reindex");
        }
    }

    /// Inserts an item into the collection.
    ///
    /// `crdt_type` stamps the ENTRY. Passing `None` here is what left the
    /// `Entry`/`or_default` write-back path unstamped while the direct
    /// `map.insert` path was stamped: the entry then carried no declaration,
    /// took the legacy branch in `try_merge_non_root`, and resolved
    /// last-write-wins with the app's rule never called. An app that reaches
    /// its map only through `entry(..).or_default()` — which is the ergonomic
    /// way, and what the reference app does — got no dispatch at all.
    fn insert(&mut self, id: Option<Id>, item: T, crdt_type: Option<CrdtType>) -> StoreResult<T> {
        // Entries inherit this collection's own storage domain. For an ordinary
        // collection the element is `Public`, so this is the previous default;
        // when the collection sits in a guarded domain (a writer-set cell, an
        // owned entry) `insert_with_storage_type` stamps every entry for it. This
        // is the chokepoint for the
        // `Entry`/`or_default` write-back path and for collections that insert via
        // the bare `Collection::insert` (sets, vectors, RGA), so guarding a
        // collection covers those paths too — not only the direct `map.insert`.
        let inherited = self.storage.metadata.storage_type.clone();
        self.insert_with_storage_type(id, item, inherited, crdt_type)
            .map(|(_id, item)| item)
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
    ///
    /// Returns the item alongside the element id it was stored under, because
    /// the id is the only stable way to address the entry afterwards —
    /// enumeration is `(created_at, id)` ordered over a set other replicas
    /// insert into, so a position is not (core#3637).
    /// `crdt_type` stamps the ENTRY, not the collection.
    ///
    /// It is what makes app-defined merge reachable: `try_merge_non_root`
    /// dispatches on the entry's own `crdt_type`, and an entry that declares
    /// nothing takes the legacy branch and resolves last-write-wins. Only the
    /// collections that know their value type pass `Some` — the generic
    /// `insert` cannot, since `T` there is already the erased item.
    ///
    /// `slot` is the address the collection derives for the entry (`None` for a
    /// fresh random one). The entry is stored at [`stored_id`] of it, which only
    /// this chokepoint can settle: the final stamp is decided here, and a
    /// re-key re-inserts entries with the stamps they had before their
    /// collection moved into an owned entry. Every owned entry is therefore
    /// stored at its owner-bound id, however the caller reached it.
    pub(crate) fn insert_with_storage_type(
        &mut self,
        slot: Option<Id>,
        item: T,
        storage_type: StorageType,
        crdt_type: Option<CrdtType>,
    ) -> StoreResult<(Id, T)> {
        let slot = slot.unwrap_or_else(|| random_entry_id(self.id()));
        if self.skips_sealed_write() {
            return Ok((slot, item));
        }
        let storage_type = self.stamp_for_put(storage_type)?;
        let id = stored_id(slot, &storage_type);
        let mut collection = CollectionMut::new(self);

        let mut entry = Entry {
            item,
            storage: Element::new(Some(id)),
        };
        // A frozen entry is tagged `FrozenStorage`: it carries no wire
        // authorization, so the tag is what a host-side repair (HashComparison,
        // level-wise) stores it back as `Frozen` by, rather than `Public`,
        // which the collection's read filter would then hide.
        let crdt_type = crdt_type.or_else(|| {
            matches!(storage_type, StorageType::Frozen).then_some(CrdtType::FrozenStorage)
        });
        // Update the `StorageType`.
        entry.storage.metadata.storage_type = storage_type;
        entry.storage.metadata.crdt_type = crdt_type;

        collection.insert(&mut entry)?;

        let stored_id = entry.storage.id();
        Ok((stored_id, entry.item))
    }

    #[inline(never)]
    fn get(&self, id: Id) -> StoreResult<Option<T>> {
        Ok(self.find_admitted(id)?.map(|entry| entry.item))
    }

    /// Whether every entry here is owned by whoever wrote it, each owner holding
    /// its own entry per key: `Guarded` owning policies, `UserStorage` and
    /// `AuthoredVector`.
    fn holds_owned_entries(&self) -> bool {
        matches!(self.storage.domain, crate::domain::Domain::Owned(_))
    }

    /// The id the entry at `slot` has for the calling account.
    ///
    /// Where each entry is owned by whoever wrote it, that is the caller's own
    /// entry: keys are unique per owner, not per collection. Inside an entry
    /// owned by one account, it is that account's. Anywhere else it is `slot`.
    pub(crate) fn resolve(&self, slot: Id) -> Id {
        match &self.storage.domain {
            crate::domain::Domain::Owned(_) => {
                owned_entry_id(slot, &AccountId::from(crate::env::account_id()))
            }
            crate::domain::Domain::OwnedBy(owner) => owned_entry_id(slot, owner),
            _ => slot,
        }
    }

    /// Load the entry at `id` if it belongs to this collection's domain. An entry
    /// the domain does not admit is one no honest writer could have made (a
    /// patched peer's), and reads as absent on every node.
    fn find_admitted(&self, id: Id) -> StoreResult<Option<Entry<T>>> {
        let entry = <Interface<S>>::find_by_id::<Entry<_>>(id)?;
        Ok(entry.filter(|entry| {
            self.storage
                .domain
                .admits(&entry.storage.metadata.storage_type)
        }))
    }

    /// Whether this write lands in a collection inside frozen data while a value
    /// is being re-keyed: it is skipped, and the re-key reports it.
    pub(crate) fn skips_sealed_write(&self) -> bool {
        self.storage.domain == crate::domain::Domain::Sealed && crate::domain::skip_sealed_write()
    }

    /// The stamp a new entry gets here, after refusing a caller without
    /// authority for this collection's domain.
    pub(crate) fn stamp_for_put(&self, requested: StorageType) -> StoreResult<StorageType> {
        let domain = &self.storage.domain;
        if domain.is_open() {
            return Ok(requested);
        }
        crate::domain::check_authority(domain)?;
        Ok(domain.stamp_for(requested)?)
    }

    /// The stamp an entry written here gets, without the authority check: what
    /// a value's nested collections inherit when it is stored here.
    pub(crate) fn nested_stamp(&self) -> StorageType {
        let inherited = self.storage.metadata.storage_type.clone();
        self.storage
            .domain
            .stamp_for(inherited)
            .unwrap_or(StorageType::Frozen)
    }

    /// Refuse removing entries from a collection in a domain the caller has no
    /// authority for.
    fn check_delete(&self) -> StoreResult<()> {
        Ok(crate::domain::check_authority(&self.storage.domain)?)
    }

    /// Take the ambient domain, unless this collection's domain is set by the
    /// policy of the wrapper that owns it. Called when a nested collection is
    /// re-keyed into the entry that now holds it.
    pub(crate) fn adopt_ambient_domain(&mut self) {
        if !self.storage.domain.is_policy() {
            self.storage.domain = crate::domain::ambient();
        }
    }

    fn contains(&self, id: Id) -> StoreResult<bool> {
        Ok(self.children_cache()?.contains(&id))
    }

    fn get_mut(&mut self, id: Id) -> StoreResult<Option<EntryMut<'_, T, S>>> {
        let entry = self.find_admitted(id)?;
        if entry.is_some() && !self.storage.domain.is_open() {
            crate::domain::check_authority(&self.storage.domain)?;
        }

        Ok(entry.map(|entry| EntryMut {
            collection: CollectionMut::new(self),
            entry,
            removed: false,
            dirty: false,
        }))
    }

    /// Number of children.
    ///
    /// Reads the child trie's maintained count — one row — rather than
    /// materialising every child id. That distinction is not cosmetic: a
    /// contract that derives an id from the current length counts on EVERY
    /// write, so a linear count is a linear write, and the collection walls on
    /// gas again with the index layer doing nothing wrong. (Measured: the chat
    /// contract reached 1,187 messages with a linear `len` and stops walling
    /// entirely without it.)
    ///
    /// The cache is preferred when it is already materialised, since then the
    /// answer costs nothing. `CollectionMut::insert` writes the trie before
    /// touching the cache, so the two never disagree.
    fn len(&self) -> StoreResult<usize> {
        // A guarded domain may hold entries it does not admit, which the trie
        // count includes, so count the admitted set instead.
        if !self.storage.domain.is_open() {
            return Ok(self.children_cache()?.len());
        }
        if let Some(cached) = self.children_ids.borrow().as_ref() {
            return Ok(cached.len());
        }
        Ok(crate::index::Index::<S>::child_count(self.id()) as usize)
    }

    fn entries(
        &self,
    ) -> StoreResult<impl ExactSizeIterator<Item = StoreResult<T>> + DoubleEndedIterator + '_> {
        // Snapshot the child ids so the returned iterator does not borrow the
        // `RefMut` guard (which is dropped at the end of this statement); each
        // entry is still read lazily on iteration.
        let ids: Vec<Id> = self.children_cache()?.iter().copied().collect();
        let iter = ids.into_iter().map(|child| {
            let entry = self
                .find_admitted(child)?
                .ok_or(StoreError::StorageError(StorageError::NotFound(child)))?;

            Ok(entry.item)
        });

        Ok(iter)
    }

    /// Snapshot every child as `(item, storage_type)` in child-list order.
    /// `storage_type` carries the per-entry owner stamp (AuthoredMap/Shared);
    /// a re-key that re-inserts plain `item`s would drop it and silently
    /// downgrade authored entries to `Public`, so callers re-insert via
    /// `insert_with_storage_type`.
    pub(crate) fn entries_with_storage_type(&self) -> StoreResult<Vec<(T, StorageType)>> {
        let ids: Vec<Id> = self.children_cache()?.iter().copied().collect();
        let mut out = Vec::with_capacity(ids.len());
        for child in ids {
            let entry = <Interface<S>>::find_by_id::<Entry<T>>(child)?
                .ok_or(StoreError::StorageError(StorageError::NotFound(child)))?;
            out.push((entry.item, entry.storage.metadata.storage_type));
        }
        Ok(out)
    }

    fn nth(&self, index: usize) -> StoreResult<Option<Id>> {
        Ok(self.children_cache()?.get_index(index).copied())
    }

    fn last(&self) -> StoreResult<Option<Id>> {
        Ok(self.children_cache()?.last().copied())
    }

    fn clear(&mut self) -> StoreResult<()> {
        self.check_delete()?;
        let mut collection = CollectionMut::new(self);

        collection.clear()?;

        Ok(())
    }

    /// Clears the collection as part of a deterministic re-key relocation.
    ///
    /// Identical to [`clear`](Self::clear) except it removes children via
    /// [`Interface::relocate_child_from`], which does not reject `Frozen`
    /// children — a re-key re-inserts every entry under a new id, so frozen
    /// data is preserved rather than deleted.
    fn clear_for_rekey(&mut self) -> StoreResult<()> {
        let mut collection = CollectionMut::new(self);

        collection.clear_for_rekey()?;

        Ok(())
    }

    /// Lazily loads the child-id set from the index and hands back a `RefMut`
    /// guard over it. The guard ties the borrow to the returned value, so
    /// callers hold the `RefCell` borrow for exactly as long as they use the
    /// set — no aliasing `&mut` outlives it (F166).
    #[expect(clippy::expect_used, reason = "cache is populated immediately above")]
    fn children_cache(&self) -> StoreResult<RefMut<'_, IndexSet<Id>>> {
        let mut cache = self.children_ids.borrow_mut();

        if cache.is_none() {
            // `IndexNotFound` here means the parent has NO index record yet — a
            // just-created or freshly-synced collection with no children, which
            // is legitimately empty. Genuine read corruption surfaces as
            // `DeserializationError` (from the index borsh decode) and is
            // propagated by the `Err(e)` arm below, NOT collapsed to empty
            // (F169). The one case indistinguishable from "never created" is a
            // *lost* index record whose child entries survive — inherent to the
            // storage model, where the index is the sole record of children.
            let domain = &self.storage.domain;
            let children: IndexSet<Id> = match <Interface<S>>::child_info_for(self.id()) {
                Ok(info) => info
                    .into_iter()
                    .filter(|c| domain.admits(&c.metadata.storage_type))
                    .map(|c| c.id())
                    .collect(),
                Err(StorageError::IndexNotFound(_)) => IndexSet::new(),
                Err(e) => return Err(StoreError::StorageError(e)),
            };

            *cache = Some(children);
        }

        Ok(RefMut::map(cache, |c| c.as_mut().expect("children")))
    }
}

/// Reads of a keyed collection, whose entries store `(value, key)`.
impl<V, K, S> Collection<(V, K), S>
where
    V: BorshSerialize + BorshDeserialize,
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    S: StorageAdaptor,
{
    /// Whether the key stored in the entry at `id` derives the slot `id` names.
    ///
    /// Apply checks that an owned entry's id is its owner's, but it cannot check
    /// the key inside: an entry's bytes end in a key of unknown length. A
    /// patched owner could therefore store one key under another key's slot.
    /// Such an entry reads as absent everywhere. Other collections address
    /// entries by the key's own id, so there is nothing to check.
    fn key_fits(&self, id: Id, key: &K) -> bool {
        !self.holds_owned_entries() || same_slot(id, compute_id(self.id(), key.as_ref()))
    }

    /// The entry at `id`, if this collection admits it and its key fits.
    fn find_keyed(&self, id: Id) -> StoreResult<Option<Entry<(V, K)>>> {
        Ok(self
            .find_admitted(id)?
            .filter(|entry| self.key_fits(id, &entry.item.1)))
    }

    /// The `(value, key)` at `id`, if this collection admits it and its key fits.
    fn get_keyed(&self, id: Id) -> StoreResult<Option<(V, K)>> {
        Ok(self.find_keyed(id)?.map(|entry| entry.item))
    }

    /// Every entry with its id, in child order. An entry whose key does not fit
    /// its id is skipped; one the child list names but storage cannot load is an
    /// error, as in [`entries`](Collection::entries).
    fn keyed_entries(&self) -> StoreResult<impl Iterator<Item = StoreResult<(Id, (V, K))>> + '_> {
        let ids: Vec<Id> = self.children_cache()?.iter().copied().collect();
        Ok(ids
            .into_iter()
            .filter_map(move |id| match self.find_admitted(id) {
                Ok(Some(entry)) => self
                    .key_fits(id, &entry.item.1)
                    .then_some(Ok((id, entry.item))),
                Ok(None) => Some(Err(StoreError::StorageError(StorageError::NotFound(id)))),
                Err(error) => Some(Err(error)),
            }))
    }

    /// Every owned entry with its owner, in child order: only `owner`'s when one
    /// is given. Reads the stored bytes of only the entries it returns.
    fn owned_entries(&self, owner: Option<&AccountId>) -> StoreResult<Vec<(AccountId, (V, K))>> {
        let children = match <Interface<S>>::child_info_for(self.id()) {
            Ok(children) => children,
            Err(StorageError::IndexNotFound(_)) => Vec::new(),
            Err(error) => return Err(StoreError::StorageError(error)),
        };
        self.owned_among(children, owner, |_| true)
    }

    /// Every owner's entry at `slot`, ascending by id. One trie bucket holds
    /// them all, since an owned id keeps its slot's prefix.
    fn entries_at(&self, slot: Id) -> StoreResult<Vec<(AccountId, (V, K))>> {
        let children =
            <Index<S>>::children_with_prefix(self.id(), &slot.as_bytes()[..OWNED_SLOT_PREFIX_LEN]);
        self.owned_among(children, None, |key| {
            compute_id(self.id(), key.as_ref()) == slot
        })
    }

    fn owned_among(
        &self,
        children: Vec<ChildInfo>,
        owner: Option<&AccountId>,
        wanted: impl Fn(&K) -> bool,
    ) -> StoreResult<Vec<(AccountId, (V, K))>> {
        let domain = &self.storage.domain;
        let mut out = Vec::new();
        for child in children {
            let StorageType::User { owner: stamped, .. } = child.metadata.storage_type else {
                continue;
            };
            if !domain.admits(&child.metadata.storage_type) || owner.is_some_and(|o| *o != stamped)
            {
                continue;
            }
            if let Some(entry) = self.find_keyed(child.id())? {
                if wanted(&entry.item.1) {
                    out.push((stamped, entry.item));
                }
            }
        }
        Ok(out)
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
    /// Set by `deref_mut`. A `get_mut` used only for reading never writes back,
    /// so its Drop must not emit a spurious Update (F167).
    dirty: bool,
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

    /// The stamp of this entry: what a replacement value's nested collections
    /// inherit.
    fn stamp(&self) -> &StorageType {
        &self.entry.storage.metadata.storage_type
    }

    fn remove(mut self) -> StoreResult<T> {
        self.collection.check_delete()?;
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
        // Any real mutation goes through here, so this is the one place that can
        // authoritatively mark the entry as needing a write-back (F167).
        self.dirty = true;
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
        // already been created, and saving would create a conflicting Update action.
        // Don't save a read-only `get_mut` either: with no mutation there is
        // nothing to persist, and a spurious Update bumps `updated_at`,
        // recomputes hashes, and can win an LWW race it should have lost (F167).
        if self.removed || !self.dirty {
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

        // Only touch the cache if it is ALREADY materialised. Calling
        // `children_cache()` here would populate it — reading every existing
        // child just to record one new id — which is an O(n) cost on every
        // insert and defeats the bounded link the trie provides. A fresh
        // contract call starts with an empty cache, so this fired on
        // essentially every write.
        //
        // Leaving it unpopulated is safe: the trie is authoritative, so a
        // later read materialises the cache from it with this child included.
        if let Some(cache) = self.collection.children_ids.borrow_mut().as_mut() {
            let _ignored = cache.insert(item.id());
        }

        Ok(())
    }

    fn clear(&mut self) -> StoreResult<()> {
        let mut children = self.collection.children_cache()?;

        for child in children.drain(..) {
            let _ = <Interface<S>>::remove_child_from(self.collection.id(), child)?;
        }

        Ok(())
    }

    /// Clears the collection for a deterministic re-key relocation, removing
    /// children via [`Interface::relocate_child_from`] so `Frozen` entries are
    /// relocated rather than rejected. See [`Collection::clear_for_rekey`].
    fn clear_for_rekey(&mut self) -> StoreResult<()> {
        let mut children = self.collection.children_cache()?;

        for child in children.drain(..) {
            let _ = <Interface<S>>::relocate_child_from(self.collection.id(), child)?;
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

/// Logged when a fallible collection comparison hits a store read error. The
/// comparison still returns a total result (it never panics), but the fault is
/// surfaced so it is not silently swallowed — in particular two unreadable
/// collections compare *equal*, which a caller must not mistake for "converged".
/// These `PartialEq`/`Ord` impls are not on the sync/merkle convergence path
/// (that compares hashes, not whole collections), so a masked difference here
/// cannot hide replication divergence; the warning keeps the fault observable
/// regardless.
#[cold]
fn warn_collection_compare_read_error() {
    tracing::warn!(
        target: "storage::collections",
        "store read error during collection comparison/format; degraded to a \
         total fallback instead of panicking — any difference between the \
         unreadable sides is not observed by this comparison"
    );
}

/// Total equality over two fallible collection reads.
///
/// `PartialEq`/`Ord`/`Debug` on these collections have to read from the
/// store, which can fail. The impls used to `.unwrap()` that read and panic
/// mid-comparison — e.g. inside a `BTreeMap` lookup or a `tracing` log — on a
/// transient or corrupt store. Instead we degrade to a *total* result: two
/// unreadable collections compare equal, and a readable one never equals an
/// unreadable one. No comparison can panic; a read error is logged via
/// [`warn_collection_compare_read_error`] so the fault stays observable.
pub(crate) fn fallible_iter_eq<I, T, E>(l: Result<I, E>, r: Result<I, E>) -> bool
where
    I: Iterator<Item = T>,
    T: PartialEq,
{
    match (l, r) {
        (Ok(l), Ok(r)) => l.eq(r),
        (Err(_), Err(_)) => {
            warn_collection_compare_read_error();
            true
        }
        _ => {
            warn_collection_compare_read_error();
            false
        }
    }
}

/// Total ordering over two fallible collection reads (see
/// [`fallible_iter_eq`]). An unreadable collection sorts before a readable
/// one; two unreadable collections compare equal. Keeps `Ord` total and
/// consistent with [`fallible_iter_partial_cmp`] rather than panicking.
pub(crate) fn fallible_iter_cmp<I, T, E>(l: Result<I, E>, r: Result<I, E>) -> Ordering
where
    I: Iterator<Item = T>,
    T: Ord,
{
    match (l, r) {
        (Ok(l), Ok(r)) => l.cmp(r),
        (Err(_), Ok(_)) => {
            warn_collection_compare_read_error();
            Ordering::Less
        }
        (Ok(_), Err(_)) => {
            warn_collection_compare_read_error();
            Ordering::Greater
        }
        (Err(_), Err(_)) => {
            warn_collection_compare_read_error();
            Ordering::Equal
        }
    }
}

/// Total partial-ordering over two fallible collection reads, matching the
/// read-failure ordering of [`fallible_iter_cmp`] so `PartialOrd` stays
/// consistent with `Ord`. Returns `None` only for genuinely incomparable
/// elements, never for a store fault.
pub(crate) fn fallible_iter_partial_cmp<I, T, E>(
    l: Result<I, E>,
    r: Result<I, E>,
) -> Option<Ordering>
where
    I: Iterator<Item = T>,
    T: PartialOrd,
{
    match (l, r) {
        (Ok(l), Ok(r)) => l.partial_cmp(r),
        (Err(_), Ok(_)) => {
            warn_collection_compare_read_error();
            Some(Ordering::Less)
        }
        (Ok(_), Err(_)) => {
            warn_collection_compare_read_error();
            Some(Ordering::Greater)
        }
        (Err(_), Err(_)) => {
            warn_collection_compare_read_error();
            Some(Ordering::Equal)
        }
    }
}

impl<T: Eq + BorshSerialize + BorshDeserialize, S: StorageAdaptor> Eq for Collection<T, S> {}

impl<T: PartialEq + BorshSerialize + BorshDeserialize, S: StorageAdaptor> PartialEq
    for Collection<T, S>
{
    fn eq(&self, other: &Self) -> bool {
        fallible_iter_eq(
            self.entries().map(Iterator::flatten),
            other.entries().map(Iterator::flatten),
        )
    }
}

impl<T: Ord + BorshSerialize + BorshDeserialize, S: StorageAdaptor> Ord for Collection<T, S> {
    fn cmp(&self, other: &Self) -> Ordering {
        fallible_iter_cmp(
            self.entries().map(Iterator::flatten),
            other.entries().map(Iterator::flatten),
        )
    }
}

impl<T: PartialOrd + BorshSerialize + BorshDeserialize, S: StorageAdaptor> PartialOrd
    for Collection<T, S>
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        fallible_iter_partial_cmp(
            self.entries().map(Iterator::flatten),
            other.entries().map(Iterator::flatten),
        )
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
