//! Group-writable storage with an authenticated, mutable writer set.
//!
//! `SharedStorage<T>` wraps a single value writable by any signer in the
//! current writer set. The writer set itself is rotatable by a current writer
//! (unless `frozen`). Trust mirrors `UserStorage<T>`: the runtime signs each
//! write, peers verify the signature against the writer set at merge time.
//!
//! # Why this is a handle, not an inline value
//!
//! `SharedStorage<T>` is modeled on [`Root<T>`](super::root::Root): it is a
//! thin handle over a [`Collection`] that holds the value as a single,
//! `Shared`-stamped child entry. Borsh-serializing the wrapper therefore emits
//! **only its `Element`** (id + metadata) — a reference — exactly like any
//! other collection field. The value body lives in its own storage entity and
//! syncs as a per-entity `Update` action, verified at merge against the writer
//! set (the same path that guards a collection's child entries).
//!
//! This is what makes writer-set rotation *authenticated*. The earlier design
//! kept `value` inline in the wrapper struct, so it rode in the enclosing
//! `#[app::state]` root-state blob, and writer-set convergence was an LWW on a
//! `writers_nonce` that did **not** verify who rotated — any context member
//! could hand-craft a root-state delta swapping the writer set. By moving the
//! value out of root state, the rotation can ride a signed per-entity action
//! instead, and a non-writer's forged rotation is rejected at merge.
//!
//! # Where the current writer set comes from
//!
//! - **At apply time on a peer** (the security boundary): the node resolves the
//!   writer set from the entity's *rotation log* via
//!   `rotation_log_reader::writers_at(delta.parents)` and verifies the action's
//!   signature against it. A forged rotation from a non-writer never updates the
//!   log, so it is rejected — see the node sync layer.
//! - **During local execution** (this module): there is no DAG/`happens_before`
//!   context and the local rotation log is only appended for *received* deltas,
//!   so the authoritative local source is the wrapper entity's **rotation log**
//!   (latest entry) falling back to its **index metadata**
//!   ([`Index::get_metadata`]) — both of which are only ever written by a
//!   signature-verified action (apply) or the local node's own committed
//!   write/rotation. It deliberately does NOT fall back to the in-memory
//!   `Element` metadata, which is deserialized from the unverified root-state
//!   blob; trusting it would reintroduce the forgeable writer-set source this
//!   change removes.
//!
//! # Merge semantics
//!
//! The wrapper itself carries no CRDT value to merge — the value is a separate
//! entity. The only field merged at root-state time is `frozen` (monotonic:
//! once frozen on either side, stays frozen). There is deliberately **no**
//! writer-set merge here: convergence is the rotation log + ADR 0001, applied
//! and verified on the node side, not an LWW on root-state bytes.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{compute_collection_id, compute_id, Collection, StoreError};
use crate::address::Id;
use crate::entities::{ChildInfo, Data, Element, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::rotation_log;
use crate::store::{Key, MainStorage, StorageAdaptor};

/// Fixed sub-key under which the wrapper's single value entry is stored.
/// The value entry's id is `compute_id(wrapper_id, VALUE_KEY)` so every node
/// derives the same id for the value of a given wrapper.
const VALUE_KEY: &[u8] = b"__calimero_shared_value__";

/// Group-writable storage with an authenticated, mutable writer set.
///
/// A handle over a [`Collection`] holding the value as one `Shared`-stamped
/// child entry. Borsh = the inner collection's `Element` (a reference) plus the
/// monotonic `frozen` flag; the value body never rides root state.
///
/// When `T` is a **collection** (`UnorderedMap`, `UnorderedSet`, …), in-place
/// edits MUST go through [`get_mut`](SharedStorage::get_mut): it re-establishes
/// the `Shared{writers}` domain on the collection element so every entry
/// inserted through it is guarded at merge.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct SharedStorage<
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor = MainStorage,
> {
    /// Holds the single value entry (`Shared`-stamped). The collection's own
    /// `Element` is the wrapper entity; its metadata carries the writer set.
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<T, S>,
    /// If true, `rotate_writers` is rejected. Monotonic: set at construction or
    /// by merge, never cleared. Rides root state inline — unlike the value, this
    /// is a small scalar that is never double-written as a per-entity action, so
    /// it cannot cause the root-hash divergence the inline value once did. It is
    /// fail-safe: a forged `frozen=true` only *locks* rotation (a minor DoS),
    /// and the monotonic merge means it can never be forged back to `false`.
    frozen: bool,
    /// Lazy cache of the deserialized value entry (Root's pattern). Not part of
    /// borsh — the value is a separate entity loaded on first access.
    #[borsh(skip, bound(serialize = "", deserialize = ""))]
    value: core::cell::RefCell<Option<T>>,
    #[borsh(skip)]
    _adaptor: core::marker::PhantomData<S>,
}

impl<T, S> core::fmt::Debug for SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + core::fmt::Debug,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedStorage")
            .field("inner", &self.inner)
            .field("frozen", &self.frozen)
            .field("value", &self.value)
            .finish()
    }
}

impl<T> SharedStorage<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
{
    /// Create a new SharedStorage with a random ID and the given initial
    /// writer set. Use this for nested fields; the `#[app::state]` macro
    /// canonicalises the id via [`reassign_deterministic_id`] after `init`.
    ///
    /// [`reassign_deterministic_id`]: SharedStorage::reassign_deterministic_id
    pub fn new(writers: BTreeSet<PublicKey>, frozen: bool) -> Self {
        let inner = Collection::new_shared(None, None, CrdtType::SharedStorage, writers.clone());
        Self::from_inner(inner, writers, frozen)
    }

    /// Create a new SharedStorage with a deterministic ID derived from
    /// `field_name`. Use this for top-level state fields.
    pub fn new_with_field_name(
        field_name: &str,
        writers: BTreeSet<PublicKey>,
        frozen: bool,
    ) -> Self {
        // Pass the deterministic id explicitly — `new_shared(None, ..)` would
        // mint a random one, breaking this method's documented contract for any
        // direct caller (the `#[app::state]` macro relocates `new()`'s random id
        // via `reassign_deterministic_id`, but this constructor must be
        // deterministic on its own).
        let id = compute_collection_id(None, field_name);
        let inner = Collection::new_shared(
            Some(id),
            Some(field_name),
            CrdtType::SharedStorage,
            writers.clone(),
        );
        Self::from_inner(inner, writers, frozen)
    }

    /// Wrap a freshly-registered, `Shared`-stamped wrapper collection and
    /// materialise its single value entry with `T::default()`.
    ///
    /// The value entry is created **eagerly** (not lazily) so that when `T` is a
    /// collection (`UnorderedMap`, …) its own id is minted exactly once, at
    /// genesis, and is therefore stable across reloads and identical on every
    /// node (it ships in the genesis state). A lazy "materialise on first
    /// access" would mint a fresh random collection id on each node that wrote
    /// before the value synced — diverging the subtree.
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    fn from_inner(
        mut inner: Collection<T, MainStorage>,
        writers: BTreeSet<PublicKey>,
        frozen: bool,
    ) -> Self {
        let value_id = compute_id(inner.id(), VALUE_KEY);
        let value = inner
            .insert_with_storage_type(
                Some(value_id),
                T::default(),
                StorageType::Shared {
                    writers,
                    signature_data: None,
                },
            )
            .expect("failed to write initial SharedStorage value");
        Self {
            inner,
            frozen,
            value: core::cell::RefCell::new(Some(value)),
            _adaptor: core::marker::PhantomData,
        }
    }

    /// Reassign the wrapper's ID to a deterministic one based on `field_name`.
    /// Called by the `#[app::state]` macro after `init()` returns so the same
    /// ID is produced across all nodes when the wrapper was created via
    /// `new()` (random ID). Runs before the state is broadcast, so relocating
    /// the value entry here ships no stale delta.
    #[expect(clippy::expect_used, reason = "fatal error if cleanup fails")]
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        let new_id = compute_collection_id(None, field_name);
        if self.inner.id() == new_id {
            return;
        }

        let writers = self.current_writers();

        // Capture the value, then relocate it with the wrapper. `from_inner`
        // creates the value entry eagerly, so it is present here — but read with
        // a `T::default()` fallback rather than asserting presence: relocation
        // must ALWAYS leave a value entry at the new id, so that even if the old
        // entry were somehow missing (storage corruption, a future lazy-creation
        // refactor) we never end up with the wrapper at the new id and no value
        // entry, which `load_value` would silently paper over with a default.
        let old_value_id = compute_id(self.inner.id(), VALUE_KEY);
        let carried_value: T = self
            .inner
            .get(old_value_id)
            .expect("read SharedStorage value during reassign")
            .unwrap_or_default();
        let _ignored = MainStorage::storage_remove(Key::Entry(old_value_id));
        let _ignored = MainStorage::storage_remove(Key::Index(old_value_id));
        let _ = <Index<MainStorage>>::remove_child_reference_only(self.inner.id(), old_value_id);

        // Relocate the wrapper entity to its deterministic id, then re-stamp the
        // Shared domain (the reassign preserves storage_type, but re-set to be
        // explicit) and persist.
        self.inner
            .reassign_deterministic_id_with_crdt_type(field_name, CrdtType::SharedStorage);
        self.inner.element_mut().set_shared_domain(writers.clone());
        let _saved = <Interface<MainStorage>>::save(&mut self.inner)
            .expect("failed to persist relocated SharedStorage wrapper");

        // Re-write the value entry under the new wrapper id (always).
        let new_value_id = compute_id(self.inner.id(), VALUE_KEY);
        let value = self
            .inner
            .insert_with_storage_type(
                Some(new_value_id),
                carried_value,
                StorageType::Shared {
                    writers,
                    signature_data: None,
                },
            )
            .expect("failed to relocate SharedStorage value");
        *self.value.borrow_mut() = Some(value);
    }
}

impl<T, S> SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    S: StorageAdaptor,
{
    /// The id of the value entry under this wrapper.
    fn value_id(&self) -> Id {
        compute_id(self.inner.id(), VALUE_KEY)
    }

    /// Lazily load (and cache) the value entry. Unlike [`Root::get`], the value
    /// entry may not exist yet on a peer that received the wrapper before the
    /// value's per-entity `Add` synced, so a missing entry yields `T::default()`
    /// rather than panicking.
    ///
    /// The `unsafe` ptr cast ties the returned `&mut T` to `&self` and **drops
    /// the `RefMut` guard** at the end of this call (mirroring [`Root::get`]) —
    /// so a later call does not collide with a still-held `RefCell` borrow.
    /// Sound because storage values are never aliased (each read deserializes a
    /// fresh copy) and execution is single-threaded WASM.
    #[expect(
        clippy::mut_from_ref,
        clippy::expect_used,
        reason = "lazy cache, mirrors Root::get"
    )]
    fn load_value(&self) -> &mut T {
        let mut slot = self.value.borrow_mut();
        let value = slot.get_or_insert_with(|| {
            self.inner
                .get(self.value_id())
                .expect("read SharedStorage value")
                .unwrap_or_default()
        });
        #[expect(unsafe_code, reason = "necessary for caching, mirrors Root::get")]
        let value = unsafe { &mut *core::ptr::from_mut(value) };
        value
    }
}

impl<T, S> SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + Data,
    S: StorageAdaptor,
{
    /// Mutable access to a collection value (`UnorderedMap`, `UnorderedSet`, …)
    /// for in-place editing.
    ///
    /// Re-establishes the writer domain on the collection's element before
    /// handing out the reference, so every entry inserted through it inherits
    /// `Shared{writers}` and is guarded at merge — including after a reload.
    /// Only collections (which implement [`Data`]) get this; a scalar value is
    /// edited via [`insert`](SharedStorage::insert) instead.
    ///
    /// # Errors
    /// Currently infallible; the `Result` is preserved for forward compatibility.
    pub fn get_mut(&mut self) -> Result<&mut T, StoreError> {
        let writers = self.current_writers();
        let value = self.load_value();
        let already_current = matches!(
            &value.element().metadata.storage_type,
            StorageType::Shared { writers: w, .. } if w == &writers
        );
        if !already_current {
            value.element_mut().set_shared_domain(writers);
        }
        Ok(value)
    }
}

impl<T, S> SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    S: StorageAdaptor,
{
    /// Get a reference to the current value (anyone can read).
    ///
    /// # Errors
    /// Currently infallible; the `Result` is preserved for forward compatibility.
    pub fn get(&self) -> Result<&T, StoreError> {
        Ok(self.load_value())
    }

    /// The current writer set, resolved only from **verified** local sources.
    ///
    /// Resolution order:
    /// 1. **Rotation log** (`rotation_log::load`). On a node that *received*
    ///    rotations, the apply path appends every signature-verified rotation
    ///    here, so the latest entry is the authoritative, verified writer set.
    ///    (The full causal `writers_at(parents)` resolution is the node-side
    ///    apply-time check; with no DAG context during local execution the
    ///    most-recently-appended entry is the right local answer.)
    /// 2. **Index `storage_type`** — written by `add_child_to` at construction,
    ///    by `apply_action` for a received bootstrap/write, and by
    ///    [`Index::set_storage_type`] on the *originating* node's own rotation
    ///    (whose log stays empty because it does not self-apply).
    ///
    /// It deliberately does **not** fall back to the in-memory `Element`
    /// metadata: that is populated from the borsh-deserialized root-state blob,
    /// which is unverified — trusting it would reintroduce the forgeable
    /// writer-set source this change exists to remove (a forged root-state delta
    /// could influence the local gate). Any wrapper a writer can legitimately
    /// act on has an index entry (construction and sync-apply both write one);
    /// if neither source has a writer set, fail closed with the empty set rather
    /// than trust unverified bytes.
    fn current_writers(&self) -> BTreeSet<PublicKey> {
        if let Ok(Some(log)) = rotation_log::load::<S>(self.inner.id()) {
            if let Some(entry) = log.entries.last() {
                return entry.new_writers.clone();
            }
            if let Some(snapshot) = log.snapshot {
                return snapshot.writers;
            }
        }
        if let Ok(Some(metadata)) = <Index<S>>::get_metadata(self.inner.id()) {
            if let StorageType::Shared { writers, .. } = metadata.storage_type {
                return writers;
            }
        }
        BTreeSet::new()
    }

    /// Read the current writer set. Public so callers can present a
    /// "members with edit rights" UI and compute incremental rotations.
    pub fn writers(&self) -> BTreeSet<PublicKey> {
        self.current_writers()
    }

    /// Whether the writer set has been frozen. Once frozen, `rotate_writers`
    /// is permanently rejected.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Returns the signature attached to the most recently applied rotation of
    /// the wrapper entity, if any. Reads the wrapper's index metadata first
    /// (the applied, verified state), then the in-memory `Element`.
    pub fn signature(&self) -> Option<SignatureData> {
        if let Ok(Some(metadata)) = <Index<S>>::get_metadata(self.inner.id()) {
            if let StorageType::Shared { signature_data, .. } = metadata.storage_type {
                return signature_data;
            }
        }
        match &self.inner.element().metadata.storage_type {
            StorageType::Shared { signature_data, .. } => *signature_data,
            _ => None,
        }
    }

    /// Replace the value. The executor must be in the current writer set.
    ///
    /// Writes the value entry stamped `Shared{writers}`, which emits a signed
    /// per-entity `Update` action verified against the writer set on peers.
    /// Returns the previous value (`Some(T::default())` on the first call).
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the executor is not in the writer set.
    #[expect(clippy::unwrap_in_result, reason = "value entry id is well-formed")]
    pub fn insert(&mut self, value: T) -> Result<Option<T>, StoreError> {
        let executor: PublicKey = env::executor_id().into();
        let writers = self.current_writers();
        if !writers.contains(&executor) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not a writer of this SharedStorage".to_owned(),
            )));
        }

        let old = self.inner.get(self.value_id())?.unwrap_or_default();

        let value_id = self.value_id();
        let shared = StorageType::Shared {
            writers,
            signature_data: None,
        };
        let new = self
            .inner
            .insert_with_storage_type(Some(value_id), value, shared.clone())?;
        // Track the current writer set on the value entry's own index. After a
        // rotation this node received, the value entry's index still carries the
        // pre-rotation set (apply does not patch a child's own `storage_type`);
        // without this, the runtime's post-execution `update_signature_in_place`
        // would see a writer-set mismatch (the just-signed write claims the new
        // set) and abort the whole transaction. Hash-neutral; no-ops when
        // unchanged.
        let _ignored = <Index<S>>::set_storage_type(value_id, shared);
        *self.value.borrow_mut() = Some(new);
        Ok(Some(old))
    }

    /// Rotate the writer set. Must be called by a current writer; rejected if
    /// `frozen` or if `new_writers` is empty.
    ///
    /// Re-stamps the **wrapper entity** with `Shared{new_writers}` and persists
    /// it, emitting a signed per-entity `Update` for the wrapper. On a peer the
    /// action is verified against the *old* writer set (resolved from the
    /// rotation log at the delta's causal point), and on success the rotation is
    /// appended to the wrapper's rotation log — so a forged rotation from a
    /// non-writer is rejected and never updates the log.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `frozen`, if `new_writers` is empty, or if
    /// the executor is not currently in the writer set.
    pub fn rotate_writers(&mut self, new_writers: BTreeSet<PublicKey>) -> Result<(), StoreError> {
        if self.frozen {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Cannot rotate writers of frozen SharedStorage".to_owned(),
            )));
        }
        if new_writers.is_empty() {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Cannot rotate to an empty writer set".to_owned(),
            )));
        }
        let executor: PublicKey = env::executor_id().into();
        let writers = self.current_writers();
        if !writers.contains(&executor) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not a current writer".to_owned(),
            )));
        }

        let wrapper_id = self.inner.id();
        let new_shared = StorageType::Shared {
            writers: new_writers.clone(),
            signature_data: None,
        };

        // Stamp the wrapper entity with the new set and persist. This emits a
        // signed per-entity `Update` for the wrapper; a receiver verifies it
        // against the *old* writer set (its rotation log at the delta's causal
        // point) and appends the new set to the wrapper's rotation log.
        self.inner
            .element_mut()
            .set_shared_domain(new_writers.clone());
        let _saved = <Interface<S>>::save(&mut self.inner)?;

        // Re-stamp the single value entry too. The value lives in its own
        // entity, verified at merge against *its* writer set; without
        // re-stamping, a writer added by this rotation could never write the
        // value (the entry would stay guarded by the pre-rotation set).
        // Re-stamping emits a signed `Update` (data unchanged → hash unchanged,
        // no divergence) so the receiver appends the new set to the value
        // entry's own rotation log, and the new writer's later writes verify.
        // This is the single-child analogue of the per-collection
        // anchor-inheritance that retroactive collection revocation will
        // generalise. The value entry is created eagerly at construction, so it
        // is present here; read with a `T::default()` fallback and re-stamp
        // unconditionally so the rotation is never silently lost if it were
        // absent.
        let value_id = self.value_id();
        let value = self.inner.get(value_id)?.unwrap_or_default();
        let _restamped =
            self.inner
                .insert_with_storage_type(Some(value_id), value, new_shared.clone())?;
        // Originating-node fallback: persist the new set on the value entry's
        // and wrapper's index too (the rotation log is only appended on
        // receivers, so this node's own log stays empty).
        let _ignored = <Index<S>>::set_storage_type(value_id, new_shared.clone());
        let _ignored = <Index<S>>::set_storage_type(wrapper_id, new_shared);

        // Invalidate the lazy cache so the next access reloads the value with
        // the new writer stamp rather than serving a copy whose in-memory
        // element still carries the pre-rotation set.
        *self.value.borrow_mut() = None;
        Ok(())
    }
}

// Implement Data so SharedStorage can be nested in #[app::state]; the wrapper
// entity is the inner collection's element.
impl<T, S> Data for SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        self.inner.element()
    }

    fn element_mut(&mut self) -> &mut Element {
        self.inner.element_mut()
    }
}

// Mergeable: invoked at root-state merge time. The value is a separate entity
// (synced + merged per-entity), and the writer set converges via the rotation
// log on the node side — so the only field merged here is `frozen` (monotonic).
impl<T, S> Mergeable for SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        // Frozen is monotonic: once set on either side, stays set. No
        // writer-set merge here — that would be the forgeable LWW-on-root-state
        // path the handle design removes; convergence is the verified rotation
        // log + ADR 0001, applied on the node side.
        if other.frozen {
            self.frozen = true;
        }
        Ok(())
    }
}

impl<T, S> CrdtMeta for SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::SharedStorage
    }
    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured
    }
    fn can_contain_crdts() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use borsh::{BorshDeserialize, BorshSerialize};
    use calimero_primitives::identity::PublicKey;
    use serial_test::serial;

    use super::SharedStorage;
    use crate::collections::crdt_meta::{MergeError, Mergeable};
    use crate::collections::Root;
    use crate::env;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];
    const CAROL: [u8; 32] = [0x33; 32];

    /// Mergeable test value — max-wins on merge so it's a valid CRDT.
    #[derive(BorshSerialize, BorshDeserialize, Default, Debug, PartialEq, Clone, Copy)]
    struct TestVal(u64);

    impl Mergeable for TestVal {
        fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
            if other.0 > self.0 {
                self.0 = other.0;
            }
            Ok(())
        }
    }

    fn pk(bytes: [u8; 32]) -> PublicKey {
        bytes.into()
    }

    fn writers(keys: &[[u8; 32]]) -> BTreeSet<PublicKey> {
        keys.iter().copied().map(pk).collect()
    }

    #[test]
    #[serial]
    fn new_with_field_name_is_deterministic() {
        // Regression: `new_with_field_name` must produce the field-derived
        // deterministic id directly (not a random one relocated later), so a
        // direct caller without the `#[app::state]` macro still converges.
        use crate::collections::compute_collection_id;
        use crate::entities::Data;

        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let _root: Root<TestVal> = Root::new(TestVal::default);

        let expected = compute_collection_id(None, "doc");
        let a = SharedStorage::<TestVal>::new_with_field_name("doc", writers(&[ALICE]), false);
        assert_eq!(a.element().id(), expected);
        let b = SharedStorage::<TestVal>::new_with_field_name("doc", writers(&[ALICE]), false);
        assert_eq!(
            b.element().id(),
            expected,
            "must be deterministic across constructions"
        );
    }

    #[test]
    #[serial]
    fn value_entry_index_tracks_writers_through_rotation() {
        // Regression for the post-rotation write abort: the value entry's index
        // `storage_type` must follow the writer set so the runtime's
        // post-execution `update_signature_in_place` does not see a writer-set
        // mismatch (which aborts the transaction). `insert` and `rotate_writers`
        // both keep it current.
        use crate::collections::compute_id;
        use crate::entities::{Data, StorageType};
        use crate::index::Index;
        use crate::store::MainStorage;

        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE]), false));
        s.insert(TestVal(1)).unwrap();

        let value_id = compute_id(s.element().id(), super::VALUE_KEY);
        let writers_of = |id| match <Index<MainStorage>>::get_metadata(id)
            .unwrap()
            .unwrap()
            .storage_type
        {
            StorageType::Shared { writers, .. } => writers,
            other => panic!("expected Shared, got {other:?}"),
        };
        assert_eq!(writers_of(value_id), writers(&[ALICE]));

        // Rotation must carry the value entry's index to the new set.
        s.rotate_writers(writers(&[ALICE, BOB])).unwrap();
        assert_eq!(writers_of(value_id), writers(&[ALICE, BOB]));

        // A write by the newly-added writer keeps it current (would mismatch if
        // the index had stayed at the pre-rotation set).
        env::set_executor_id(BOB);
        s.insert(TestVal(2)).unwrap();
        assert_eq!(writers_of(value_id), writers(&[ALICE, BOB]));
    }

    #[test]
    #[serial]
    fn shared_collection_entries_inherit_writer_domain() {
        use crate::collections::{compute_id, LwwRegister, UnorderedMap};
        use crate::entities::{Data, StorageType};
        use crate::interface::Interface;
        use crate::store::MainStorage;

        env::reset_for_testing();
        env::set_executor_id([7u8; 32]);

        type Map = UnorderedMap<String, LwwRegister<String>>;

        let ws = writers(&[[7u8; 32]]);
        let mut guarded = Root::new(|| SharedStorage::<Map>::new(ws.clone(), false));

        // Edit the inner map in place through the guarded `get_mut`, which
        // re-establishes the writer domain on the map element first.
        let _old = guarded
            .get_mut()
            .expect("get_mut")
            .insert("k".to_owned(), LwwRegister::new("v".to_owned()))
            .expect("insert");

        // The entry must carry the Shared writer domain — the whole subtree is
        // guarded at merge, not just the SharedStorage wrapper entity.
        let map_id = <Map as Data>::id(guarded.get().expect("get"));
        let child = compute_id(map_id, "k".as_bytes());
        let entry = <Interface<MainStorage>>::find_by_id::<
            crate::collections::Entry<(String, LwwRegister<String>)>,
        >(child)
        .expect("load child")
        .expect("child exists");
        match entry.storage.metadata.storage_type {
            StorageType::Shared { writers: w, .. } => assert_eq!(w, ws),
            other => panic!("SharedStorage<Map> entry must inherit Shared, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn get_returns_default_before_insert() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE, BOB]), false));
        assert_eq!(s.get().unwrap(), &TestVal::default());
    }

    #[test]
    #[serial]
    fn insert_by_writer_succeeds() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(42)).expect("alice (writer) inserts");
        assert_eq!(s.get().unwrap(), &TestVal(42));
    }

    #[test]
    #[serial]
    fn insert_by_non_writer_short_circuits() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[BOB, CAROL]), false));
        let err = s
            .insert(TestVal(42))
            .expect_err("alice (not writer) must be rejected");
        assert!(
            err.to_string().to_lowercase().contains("writer"),
            "error should mention writer, got: {err}"
        );
        assert_eq!(s.get().unwrap(), &TestVal::default());
    }

    #[test]
    #[serial]
    fn rotate_writers_by_current_writer_succeeds() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(1)).unwrap();

        s.rotate_writers(writers(&[BOB, CAROL]))
            .expect("alice (current writer) rotates");

        // Alice can no longer write.
        let err = s
            .insert(TestVal(99))
            .expect_err("alice removed from writer set must be rejected");
        assert!(err.to_string().to_lowercase().contains("writer"));

        // Bob (new writer) can.
        env::set_executor_id(BOB);
        s.insert(TestVal(99)).expect("bob (new writer) inserts");
        assert_eq!(s.get().unwrap(), &TestVal(99));
    }

    #[test]
    #[serial]
    fn rotate_writers_by_non_writer_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE]), false));
        s.insert(TestVal(1)).unwrap();

        env::set_executor_id(BOB);
        let err = s
            .rotate_writers(writers(&[BOB]))
            .expect_err("non-writer rotation must fail");
        assert!(err.to_string().to_lowercase().contains("writer"));
    }

    #[test]
    #[serial]
    fn rotate_to_empty_writer_set_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(1)).unwrap();

        let err = s
            .rotate_writers(BTreeSet::new())
            .expect_err("rotation to empty set must fail");
        assert!(
            err.to_string().to_lowercase().contains("empty"),
            "error should mention empty, got: {err}"
        );

        // Storage is still functional — alice can still write.
        let _ = s.insert(TestVal(2)).expect("alice can still write");
        assert_eq!(s.get().unwrap(), &TestVal(2));
    }

    #[test]
    #[serial]
    fn writers_accessor_reflects_bootstrap_then_rotation() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE, BOB]), false));

        // Bootstrap-time writer set is observable.
        assert_eq!(s.writers(), writers(&[ALICE, BOB]));
        assert!(!s.is_frozen());

        // After a rotation, the accessor sees the new set.
        s.rotate_writers(writers(&[ALICE, CAROL])).unwrap();
        assert_eq!(s.writers(), writers(&[ALICE, CAROL]));
        assert!(!s.is_frozen());
    }

    #[test]
    #[serial]
    fn is_frozen_accessor_reflects_construction_and_blocks_rotation() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE]), true));

        assert!(s.is_frozen());
        assert_eq!(s.writers(), writers(&[ALICE]));

        // A frozen instance must reject rotation regardless of caller.
        let err = s
            .rotate_writers(writers(&[ALICE, BOB]))
            .expect_err("rotation on frozen instance must fail");
        assert!(
            err.to_string().to_lowercase().contains("frozen"),
            "error should mention frozen, got: {err}"
        );

        // The set is unchanged after the rejected rotation.
        assert_eq!(s.writers(), writers(&[ALICE]));
    }

    #[test]
    #[serial]
    fn merge_frozen_is_monotonic() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        // Establish ROOT so the wrapper's `add_child_to(ROOT)` succeeds.
        let _root: Root<TestVal> = Root::new(TestVal::default);
        let mut a = SharedStorage::<TestVal>::new(writers(&[ALICE]), false);
        let b = SharedStorage::<TestVal>::new(writers(&[ALICE]), true);

        Mergeable::merge(&mut a, &b).unwrap();
        assert!(a.frozen, "frozen=true on b should propagate to a");

        // Reverse direction: frozen stays once set.
        let _root2: Root<TestVal> = Root::new(TestVal::default);
        let mut a2 = SharedStorage::<TestVal>::new(writers(&[ALICE]), true);
        let b2 = SharedStorage::<TestVal>::new(writers(&[ALICE]), false);
        Mergeable::merge(&mut a2, &b2).unwrap();
        assert!(a2.frozen, "frozen=true on a2 must not be cleared by merge");
    }

    #[test]
    #[serial]
    fn frozen_at_construction_blocks_rotation() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE, BOB]), true));
        s.insert(TestVal(1)).unwrap();

        let err = s
            .rotate_writers(writers(&[ALICE]))
            .expect_err("rotation on frozen must fail");
        assert!(err.to_string().to_lowercase().contains("frozen"));
    }
}
