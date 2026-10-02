//! Group-writable storage with an authenticated, mutable writer set.
//!
//! `WriterSetCell<T>` wraps a single value writable by any signer in the
//! current writer set. The writer set itself is rotatable by a current admin
//! (unless `frozen`), by a governance op the node publishes; the cell only asks
//! for it. Trust mirrors `UserStorage<T>`: the runtime signs each write, peers
//! verify the signature against the writer set at merge time.
//!
//! # Why this is a handle, not an inline value
//!
//! `WriterSetCell<T>` is modeled on [`Root<T>`](super::root::Root): it is a
//! thin handle over a [`Collection`] that holds the value as a single,
//! `Shared`-stamped child entry. Borsh-serializing the wrapper therefore emits
//! **only its `Element`** (id + metadata) — a reference — exactly like any
//! other collection field. The value body lives in its own storage entity and
//! syncs as a per-entity `Update` action, verified at merge against the writer
//! set (the same path that guards a collection's child entries).
//!
//! This is what makes writer-set rotation *authenticated*: a rotation is not a
//! write to the cell at all, so no root-state delta can swap the writer set.
//!
//! # Where the current writer set comes from
//!
//! From the host, through [`env::shared_writers`]: the governance fold of the
//! cell's rotations at the run's causal cut (on a peer, the delta's parents).
//! A cell no rotation touched has the genesis set stored with its anchor. A cell
//! the host cannot resolve has no writers. A rotation this run asked for is read
//! back at once. The in-memory `Element` metadata is never consulted: it is
//! deserialized from the unverified root-state blob.
//!
//! # Merge semantics
//!
//! The wrapper carries no CRDT value to merge at root-state time: the value is a
//! separate entity (merged per-entity) and the writer set converges through the
//! governance fold on the node side, not an LWW on root-state bytes.
//! `frozen` is genesis-immutable (set in `new`, no setter) — it rides root-state
//! borsh so joiners see it, but the merge deliberately does **not** adopt the
//! peer's `frozen`, so a forged root-state delta cannot freeze rotation on an
//! honest node.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;

use super::crdt_meta::{CrdtMeta, CrdtType, MergeStrategy, Mergeable, StorageStrategy};
use super::{cell_id, cell_value_id, compute_collection_id, Collection, StoreError};
use crate::address::Id;
use crate::entities::{ChildInfo, Data, Element, OpMask, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::shared_writers::SharedRotation;
use crate::store::{Key, MainStorage, StorageAdaptor};

/// Fixed sub-key under which a `TeeOnly` cell's value entry is stored, at
/// `compute_id(wrapper_id, VALUE_KEY)`. Every other cell's value is at
/// [`cell_value_id`], which binds the id to its anchor.
pub(crate) const VALUE_KEY: &[u8] = b"__calimero_shared_value__";

/// Group-writable storage with an authenticated, mutable writer set.
///
/// A handle over a [`Collection`]: the wrapper entity is the `Shared` **anchor**
/// that names the genesis writer set, and the value is held as one
/// `SharedMember`-stamped child entry pointing back at that anchor. Borsh = the
/// inner collection's `Element` (a reference) plus the monotonic `frozen` flag;
/// the value body never rides root state.
///
/// When `T` is a **collection** (`UnorderedMap`, `UnorderedSet`, …), in-place
/// edits MUST go through [`get_mut`](WriterSetCell::get_mut): it re-establishes
/// the `SharedMember{anchor}` domain on the collection element so every entry
/// inserted through it is guarded at merge. Members carry no writer set — they
/// resolve the anchor's writers — so rotating the anchor retroactively revokes
/// the whole subtree without re-stamping any entry.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct WriterSetCell<
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor = MainStorage,
> {
    /// Holds the single value entry (`Shared`-stamped). The collection's own
    /// `Element` is the wrapper entity; its metadata carries the writer set.
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<T, S>,
    /// If true, `rotate_writers` is rejected. **Genesis-immutable**: set once in
    /// `new`, no setter. It rides root-state borsh inline (so cold-join peers
    /// learn it at genesis), but `Mergeable::merge` deliberately does NOT adopt a
    /// peer's `frozen` — so a forged root-state delta cannot freeze an existing
    /// honest node's rotation. The residual trust assumption is the **genesis
    /// blob**: a cold joiner that receives a forged genesis with `frozen=true`
    /// from a malicious provider would be locked (the accepted "minor DoS"). This
    /// is the same genesis-provider trust the writer set itself relies on (the
    /// initial sole writer controls genesis).
    frozen: bool,
    /// Lazy cache of the deserialized value entry (Root's pattern). Not part of
    /// borsh — the value is a separate entity loaded on first access.
    #[borsh(skip, bound(serialize = "", deserialize = ""))]
    value: core::cell::RefCell<Option<T>>,
    #[borsh(skip)]
    _adaptor: core::marker::PhantomData<S>,
}

impl<T, S> core::fmt::Debug for WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + core::fmt::Debug,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WriterSetCell")
            .field("inner", &self.inner)
            .field("frozen", &self.frozen)
            .field("value", &self.value)
            .finish()
    }
}

/// A fresh cell id bound to `writers`, for a cell no field names.
fn random_cell_id(writers: &BTreeMap<AccountId, OpMask>) -> Id {
    cell_id(Id::random(), writers)
}

impl<T> WriterSetCell<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
{
    /// Create a new WriterSetCell with a random ID and the given initial
    /// writer set. Use this for nested fields; the `#[app::state]` macro
    /// canonicalises the id via [`reassign_deterministic_id`] after `init`.
    ///
    /// The id is a [`cell_id`] of a random field id, so it is bound to
    /// `writers` like a field-derived one: merge refuses a `Shared` entity at
    /// any id that is not.
    ///
    /// [`reassign_deterministic_id`]: WriterSetCell::reassign_deterministic_id
    pub fn new(writers: BTreeSet<AccountId>, frozen: bool) -> Self {
        let id = random_cell_id(&crate::entities::full_mask(writers.clone()));
        let inner =
            Collection::new_shared(Some(id), None, CrdtType::SharedStorage, writers.clone());
        Self::from_inner(inner, writers, frozen)
    }

    /// Create a new WriterSetCell with a deterministic ID derived from
    /// `field_name` and `writers`. Use this for top-level state fields.
    pub fn new_with_field_name(
        field_name: &str,
        writers: BTreeSet<AccountId>,
        frozen: bool,
    ) -> Self {
        // Pass the deterministic id explicitly — `new_shared(None, ..)` would
        // mint a random one, breaking this method's documented contract for any
        // direct caller (the `#[app::state]` macro relocates `new()`'s random id
        // via `reassign_deterministic_id`, but this constructor must be
        // deterministic on its own).
        let id = cell_id(
            compute_collection_id(None, field_name),
            &crate::entities::full_mask(writers.clone()),
        );
        let inner = Collection::new_shared(
            Some(id),
            Some(field_name),
            CrdtType::SharedStorage,
            writers.clone(),
        );
        Self::from_inner(inner, writers, frozen)
    }

    /// A handle to a cell that stores **nothing** yet: no wrapper entity and no
    /// value entry. [`materialize`](Self::materialize) creates both on the first
    /// authorised write.
    ///
    /// This is for a writer set the creator is not in, such as `TeeOnly`'s
    /// `{TEE_AUTHORITY}`. An eager genesis there would be signed by the creator,
    /// and every peer would drop it because the creator is not a writer. Deferring
    /// genesis to the first write means a writer signs it, so peers accept it.
    /// Only the id rides root state (an `Element` serialises to its id alone).
    ///
    /// The id is bound to `writers`, the set [`materialize`](Self::materialize)
    /// will be given, as [`new`](Self::new)'s is.
    pub(crate) fn new_unmaterialized(writers: BTreeSet<AccountId>, frozen: bool) -> Self {
        let id = random_cell_id(&crate::entities::full_mask(writers));
        let mut storage = Element::new(Some(id));
        storage.metadata.crdt_type = Some(CrdtType::SharedStorage);
        Self {
            inner: Collection {
                children_ids: core::cell::RefCell::new(None),
                storage,
                // Genesis is deferred by `materialize` below, which a writer
                // must sign; the generic first-insert link must not pre-empt it.
                materialized: core::cell::Cell::new(true),
                slot_key: None,
                stamped_value: None,
                _priv: core::marker::PhantomData,
            },
            frozen,
            value: core::cell::RefCell::new(None),
            _adaptor: core::marker::PhantomData,
        }
    }

    /// Create the wrapper entity at this handle's id, stamped `Shared{writers}`,
    /// and its value entry with `T::default()`. The caller must have checked that
    /// the executor is in `writers`, so the genesis it emits is signed by a writer.
    pub(crate) fn materialize(&mut self, writers: BTreeSet<AccountId>) {
        let field_name = self.inner.storage.metadata.field_name.clone();
        let inner = Collection::new_shared(
            Some(self.inner.id()),
            field_name.as_deref(),
            CrdtType::SharedStorage,
            writers.clone(),
        );
        *self = Self::from_inner(inner, writers, self.frozen);
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
    fn from_inner(
        inner: Collection<T, MainStorage>,
        _writers: BTreeSet<AccountId>,
        frozen: bool,
    ) -> Self {
        Self::from_inner_with(inner, frozen, T::default())
    }

    /// A cell whose only writer is the calling account, holding `value` from
    /// genesis, and which nobody can ever change: that writer holds
    /// [`OpMask::WRITE_ONCE`] alone, so every node refuses a later write with
    /// different bytes, any delete (no `DELETE`) and any rotation (no
    /// `ADMIN`), and the cell is frozen. What backs [`Frozen`](super::Frozen).
    pub(crate) fn new_write_once(value: T) -> Self {
        let writer = AccountId::from(env::account_id());
        let writers = [(writer, OpMask::WRITE_ONCE)].into_iter().collect();
        let id = random_cell_id(&writers);
        let inner = Collection::new_shared_scoped(Some(id), None, CrdtType::SharedStorage, writers);
        Self::from_inner_with(inner, true, value)
    }

    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    fn from_inner_with(mut inner: Collection<T, MainStorage>, frozen: bool, initial: T) -> Self {
        // The wrapper entity is the `Shared` anchor (stamped by `new_shared`
        // with `writers`); the value entry — and, when `T` is a collection,
        // everything beneath it — is a `SharedMember` pointing back at the
        // wrapper. The member carries no writer set; it resolves the anchor's
        // writers at verify time.
        let anchor = inner.id();
        let value_id = cell_value_id(anchor);
        // A collection value's own id is derived from the value entry's, not
        // minted at random: two writers that each create a lazily created cell
        // before seeing the other's genesis (two TEE authorities, for a
        // `TeeOnly` cell) must name the same collection, or the entries one of
        // them wrote hang under a collection the merge drops.
        let mut initial = initial;
        crate::collections::rekey::RekeyTarget::rekey_relative_to(&mut initial, value_id);
        let value = inner
            .insert_with_storage_type(
                Some(value_id),
                initial,
                StorageType::SharedMember {
                    anchor,
                    signature_data: None,
                },
                None,
            )
            .expect("failed to write initial WriterSetCell value")
            .1;
        Self {
            inner,
            frozen,
            value: core::cell::RefCell::new(Some(value)),
            _adaptor: core::marker::PhantomData,
        }
    }

    /// Reassign the wrapper's ID to a deterministic one based on `field_name`
    /// and the writer set `init()` gave it. Called by the `#[app::state]` macro
    /// after `init()` returns so the same ID is produced across all nodes when
    /// the wrapper was created via `new()` (random ID). Runs before the state
    /// is broadcast, so relocating the value entry here ships no stale delta.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        let id = cell_id(
            compute_collection_id(None, field_name),
            &self.current_writers(),
        );
        self.reassign_deterministic_id_to(id, field_name);
    }

    /// [`reassign_deterministic_id`](Self::reassign_deterministic_id) to an id
    /// the caller derived from `field_name`.
    #[expect(clippy::expect_used, reason = "fatal error if cleanup fails")]
    pub(crate) fn reassign_deterministic_id_to(&mut self, new_id: Id, field_name: &str) {
        if self.inner.id() == new_id {
            return;
        }
        // A cell that stores nothing yet (see `new_unmaterialized`) has no entity
        // to relocate: only the handle moves, and genesis later lands at the new id.
        if !matches!(
            <Index<MainStorage>>::get_metadata(self.inner.id()),
            Ok(Some(_))
        ) {
            self.inner
                .storage
                .reassign_id_and_field_name(new_id, field_name);
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
        let old_value_id = cell_value_id(self.inner.id());
        let carried_value: T = match self
            .inner
            .get(old_value_id)
            .expect("read WriterSetCell value during reassign")
        {
            Some(value) => value,
            None => {
                // Eager construction guarantees the value entry exists, so a
                // miss here means storage corruption. Default to keep relocation
                // total (the new id always gets an entry), but make the anomaly
                // visible rather than silently papering over lost data.
                tracing::warn!(
                    target: "storage::shared",
                    old_value_id = %old_value_id,
                    "WriterSetCell value entry missing during reassign — \
                     relocating a default (possible storage corruption)"
                );
                T::default()
            }
        };
        let _ignored = MainStorage::storage_remove(Key::Entry(old_value_id));
        let _ignored = MainStorage::storage_remove(Key::Index(old_value_id));
        let _ = <Index<MainStorage>>::remove_child_reference_only(self.inner.id(), old_value_id);

        // Relocate the wrapper entity to its deterministic id, then re-stamp the
        // Shared domain (the reassign preserves storage_type, but re-set to be
        // explicit) and persist.
        self.inner
            .reassign_top_level_id(new_id, field_name, CrdtType::SharedStorage);
        self.inner
            .element_mut()
            .set_shared_domain_scoped(writers.clone());
        let _saved = <Interface<MainStorage>>::save(&mut self.inner)
            .expect("failed to persist relocated WriterSetCell wrapper");

        // Re-write the value entry under the new wrapper id (always), stamped as
        // a member anchored to the new wrapper id.
        let new_anchor = self.inner.id();
        let new_value_id = cell_value_id(new_anchor);
        // A collection value's id derives from the value id, which binds the
        // anchor, so it moves with the value.
        let mut carried_value = carried_value;
        crate::collections::rekey::RekeyTarget::rekey_relative_to(&mut carried_value, new_value_id);
        let value = self
            .inner
            .insert_with_storage_type(
                Some(new_value_id),
                carried_value,
                StorageType::SharedMember {
                    anchor: new_anchor,
                    signature_data: None,
                },
                None,
            )
            .expect("failed to relocate WriterSetCell value")
            .1;
        *self.value.borrow_mut() = Some(value);
    }
}

impl<T, S> WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    S: StorageAdaptor,
{
    /// Whether the wrapper entity exists in this node's storage. Always true for
    /// an eagerly created cell; false for an unmaterialized one until its first
    /// write, and on a peer until that write syncs.
    ///
    /// # Errors
    /// Returns any underlying index read error.
    pub(crate) fn is_materialized(&self) -> Result<bool, StoreError> {
        Ok(<Index<S>>::get_metadata(self.inner.id())
            .map_err(StoreError::StorageError)?
            .is_some())
    }

    /// The id of the value entry under this wrapper.
    pub(crate) fn value_id(&self) -> Id {
        cell_value_id(self.inner.id())
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
    #[expect(clippy::mut_from_ref, reason = "lazy cache, mirrors Root::get")]
    fn load_value(&self) -> Result<&mut T, StoreError> {
        let mut slot = self.value.borrow_mut();
        // The lazy initializer reads storage and is fallible, so this can't use
        // `Option::get_or_insert_with`. `load_value` runs on *every* `get`, so a
        // transient store error must propagate as a `Result` rather than panic.
        if slot.is_none() {
            let loaded = self.inner.get(self.value_id())?.unwrap_or_default();
            *slot = Some(loaded);
        }
        // The slot is `Some` by the line above, so `get_or_insert_with` never
        // calls `T::default` — it is just the panic-free way to obtain `&mut T`
        // from a now-populated slot (a borrow-checker-clean alternative to
        // `as_mut().unwrap()` that needs no `clippy::expect_used` waiver). A
        // bare `expect`/`unwrap` would reintroduce a panic on the hot read path
        // this change exists to remove.
        let value = slot.get_or_insert_with(T::default);
        #[expect(unsafe_code, reason = "necessary for caching, mirrors Root::get")]
        let value = unsafe { &mut *core::ptr::from_mut(value) };
        Ok(value)
    }
}

impl<T, S> WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + Data,
    S: StorageAdaptor,
{
    /// Mutable access to a collection value (`UnorderedMap`, `UnorderedSet`, …)
    /// for in-place editing.
    ///
    /// Re-establishes the member domain on the collection's element before
    /// handing out the reference, so every entry inserted through it inherits
    /// `SharedMember{anchor=wrapper}` and is guarded at merge — including after a
    /// reload. The entries carry no writer set; they resolve the anchor's
    /// writers at verify time, so a rotation revokes the whole subtree at once.
    /// Only collections (which implement [`Data`]) get this; a scalar value is
    /// edited via [`insert`](WriterSetCell::insert) instead.
    ///
    /// # Errors
    /// Returns any underlying storage error from loading the value entry.
    pub fn get_mut(&mut self) -> Result<&mut T, StoreError> {
        let anchor = self.inner.id();
        let value = self.load_value()?;
        // Stamp the value-collection element as a member anchored to the
        // wrapper, and put it in that anchor's domain. Every entry is stamped a
        // member of the SAME anchor, and so is every entry of a collection
        // nested in one, at any depth (see `crate::domain`): a flat domain whose
        // writers live once, at the wrapper.
        let already_current = matches!(
            &value.element().metadata.storage_type,
            StorageType::SharedMember { anchor: a, .. } if *a == anchor
        );
        if !already_current {
            value.element_mut().set_shared_member_domain(anchor);
        }
        Ok(value)
    }
}

impl<T, S> WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    S: StorageAdaptor,
{
    /// The anchor id every member of this cell names, and whose writers every
    /// node resolves when it checks a member's write.
    pub(crate) fn anchor(&self) -> Id {
        self.inner.id()
    }

    /// Get a reference to the current value (anyone can read).
    ///
    /// # Errors
    /// Returns any underlying storage error from loading the value entry.
    pub fn get(&self) -> Result<&T, StoreError> {
        self.load_value().map(|value| &*value)
    }

    /// Returns the value entry's stamped `schema_version`, or `None` if no value
    /// has been written or it was never stamped (legacy). Reads the
    /// Merkle-invisible `Metadata.schema_version`; used to skip an
    /// already-migrated shared value.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn value_schema_version(&self) -> Result<Option<u32>, StoreError> {
        let metadata =
            <Index<S>>::get_metadata(self.value_id()).map_err(StoreError::StorageError)?;
        Ok(metadata.and_then(|m| m.schema_version))
    }

    /// Returns whether the current executor is in the authoritative writer set.
    /// Gates whether `migrate_my_entries()` may re-write the shared value (a
    /// writer-signed update, not single-owner). Resolves via the same
    /// host-resolved path as the write gate.
    pub fn writable_by_me(&self) -> bool {
        // The GATE resolves the account, not the device. This is the whole point of
        // account-keying the writer set: a person with two devices is one principal
        // here, so granting them does not mean enumerating their machines.
        //
        // Only the gate moves. Owner STAMPS written into `UserStorage`/`AuthoredMap`/
        // `SharedStorage` stay device-shaped, because they are per-writer state — two
        // devices of one account acting concurrently must remain distinguishable or
        // they share a counter slot and an HLC seed and silently lose each other's
        // writes.
        let executor: AccountId = env::account_id().into();
        self.current_writers().contains_key(&executor)
    }

    /// The current writer set, as the host resolves it at this run's cut.
    ///
    /// The one resolver shared with the merge path
    /// (`Interface::resolve_anchor_writers`): a rotated set as the host gives it
    /// (including one this run asked for), else the genesis set stored with the
    /// anchor's index entry, else empty. It deliberately does **not** read the
    /// in-memory `Element` metadata, which comes from the unverified root-state
    /// blob, so a forged root-state delta cannot influence the local gate.
    fn current_writers(&self) -> BTreeMap<AccountId, OpMask> {
        crate::interface::Interface::<S>::resolve_anchor_writers(self.inner.id())
    }

    /// Read the current writer set (membership only). Public so callers can
    /// present a "members with edit rights" UI and compute incremental
    /// rotations. Use [`capabilities`](Self::capabilities) for the per-writer
    /// [`OpMask`]s.
    pub fn writers(&self) -> BTreeSet<AccountId> {
        self.current_writers().into_keys().collect()
    }

    /// The current writers with their [`OpMask`]s.
    pub fn capabilities(&self) -> BTreeMap<AccountId, OpMask> {
        self.current_writers()
    }

    /// Whether the writer set has been frozen. Once frozen, `rotate_writers`
    /// is permanently rejected.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Returns the signature on the wrapper entity's stamp (its genesis write;
    /// a rotation does not re-stamp it), if any. Reads the wrapper's index
    /// metadata first (the applied, verified state), then the in-memory `Element`.
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
    pub fn insert(&mut self, value: T) -> Result<Option<T>, StoreError> {
        let executor: AccountId = env::account_id().into();
        let writers = self.current_writers();
        if !writers.contains_key(&executor) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not a writer of this WriterSetCell".to_owned(),
            )));
        }

        let old = self.inner.get(self.value_id())?.unwrap_or_default();

        let value_id = self.value_id();
        // The value entry is a member anchored to the wrapper; it carries no
        // writer set, so there is nothing to keep consistent with a rotation —
        // the host is the single source. (This is why the
        // old value-entry `set_storage_type` writer-patch is gone: a member's
        // `update_signature_in_place` matches on the anchor, which never
        // changes on rotation.)
        let member = StorageType::SharedMember {
            anchor: self.inner.id(),
            signature_data: None,
        };
        let (_new_id, new) =
            self.inner
                .insert_with_storage_type(Some(value_id), value, member, None)?;
        *self.value.borrow_mut() = Some(new);
        Ok(Some(old))
    }

    /// Rotate the writer set. Must be called by a current admin; rejected if
    /// `frozen` or if the node could not publish it.
    ///
    /// This only records the request: a rotation takes effect when the node
    /// publishes it as a governance op, and every node reads the result from the
    /// governance fold. This run reads the new set at once, from the env.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `frozen`, if the executor does not hold `ADMIN` in the
    /// writer set, or if the node could not publish the rotation (`SharedRotation::refusal`).
    pub fn rotate_writers(&mut self, new_writers: BTreeSet<AccountId>) -> Result<(), StoreError> {
        // Convenience: every writer gets `OpMask::FULL` (today's behaviour).
        self.rotate_writers_scoped(new_writers.into_iter().map(|w| (w, OpMask::FULL)).collect())
    }

    /// Rotate the writer set with explicit per-writer [`OpMask`]s. Same rules as
    /// [`rotate_writers`](Self::rotate_writers).
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `frozen`, if the executor does not hold `ADMIN` in the
    /// writer set, or if the node could not publish the rotation (`SharedRotation::refusal`).
    pub fn rotate_writers_scoped(
        &mut self,
        new_writers: BTreeMap<AccountId, OpMask>,
    ) -> Result<(), StoreError> {
        if self.frozen {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Cannot rotate writers of frozen WriterSetCell".to_owned(),
            )));
        }
        let executor: AccountId = env::account_id().into();
        let prior = self.current_writers();
        // Fail fast; the governance op is what is checked when it is applied.
        if !prior
            .get(&executor)
            .is_some_and(|mask| mask.contains(OpMask::ADMIN))
        {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not an admin of the writer set".to_owned(),
            )));
        }

        let rotation = SharedRotation {
            cell: self.inner.id(),
            prior,
            new: new_writers,
        };
        if let Some(refusal) = rotation.refusal() {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                refusal.to_string(),
            )));
        }

        // Members and the wrapper are not re-stamped: their writers are read from
        // the host, so a rotation leaves every stored byte alone.
        env::record_shared_rotation(&rotation);

        // Invalidate the lazy cache so the next access reloads the value fresh.
        *self.value.borrow_mut() = None;
        Ok(())
    }
}

// Implement Data so WriterSetCell can be nested in #[app::state]; the wrapper
// entity is the inner collection's element.
impl<T, S> Data for WriterSetCell<T, S>
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

// Mergeable: invoked at root-state merge time. Nothing rides this merge — the
// value is a separate entity (merged per-entity), the writer set converges via
// the governance fold, and `frozen` is genesis-immutable and deliberately not
// adopted from the peer (so a forged root-state delta can't freeze rotation).
// RekeyTarget supertrait. The cell's value is a separate, per-entity
// synced entity and the wrapper id is deterministic at construction, so there
// is no value-path nested id to re-key — a no-op, like its merge.
impl<T, S> crate::collections::rekey::RekeyTarget for WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + 'static,
    S: StorageAdaptor,
{
    fn rekey_relative_to(&mut self, _parent_id: crate::address::Id) {}
}

#[diagnostic::do_not_recommend]
impl<T, S> Mergeable for WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn merge(&mut self, _other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        // Nothing to merge. The value is a separate entity (synced + merged
        // per-entity) and the writer set converges through the governance fold
        // on the node side, and neither rides this merge.
        //
        // `frozen` is intentionally NOT merged from `other`. It is set once at
        // construction and has no setter, so it is genesis-immutable; joiners
        // receive it via root-state deserialization at genesis, and there is no
        // post-genesis transition to propagate. Crucially, NOT adopting
        // `other.frozen` here means a forged root-state delta carrying
        // `frozen = true` cannot freeze an honest node's rotation capability —
        // the honest node keeps its own value. (A `frozen` change could only
        // come from genesis, which the initial sole writer is trusted for.)
        Ok(())
    }
}

impl<T, S> CrdtMeta for WriterSetCell<T, S>
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

/// Structural: the storage layer merges this by its `crdt_type` variant, so
/// there is no app rule to dispatch. See [`MergeStrategy`].
#[diagnostic::do_not_recommend]
impl<T, S> MergeStrategy for WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    const DISPATCHED: bool = false;
}

#[cfg(test)]
mod tests {
    use crate::collections::MergeStrategy;
    use std::collections::BTreeSet;

    use borsh::{BorshDeserialize, BorshSerialize};
    use serial_test::serial;

    use super::WriterSetCell;
    use crate::collections::crdt_meta::{MergeError, Mergeable};
    use crate::collections::Root;
    use crate::env;
    use calimero_account::AccountId;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];
    const CAROL: [u8; 32] = [0x33; 32];

    /// Mergeable test value — max-wins on merge so it's a valid CRDT.
    #[derive(BorshSerialize, BorshDeserialize, Default, Debug, PartialEq, Clone, Copy)]
    struct TestVal(u64);

    // RekeyTarget supertrait of Mergeable.
    impl crate::collections::rekey::RekeyTarget for TestVal {
        fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
            crate::rekey_field_if_supported!(
                &mut self.0,
                crate::collections::rekey::field_child_id(parent_id, "0")
            );
        }
    }

    // Structural: a test fixture, merged by the storage layer's own rules.
    #[diagnostic::do_not_recommend]
    impl MergeStrategy for TestVal {
        const DISPATCHED: bool = false;
    }

    impl Mergeable for TestVal {
        fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
            if other.0 > self.0 {
                self.0 = other.0;
            }
            Ok(())
        }
    }

    /// An account named by raw bytes. Called `pk` from when writer sets held
    /// keys; they name accounts now, and the signing key is a separate value.
    fn pk(bytes: [u8; 32]) -> AccountId {
        AccountId::from(bytes)
    }

    fn writers(keys: &[[u8; 32]]) -> BTreeSet<AccountId> {
        keys.iter().copied().map(pk).collect()
    }

    #[test]
    #[serial]
    fn new_with_field_name_is_deterministic() {
        // Regression: `new_with_field_name` must produce the field-derived
        // deterministic id directly (not a random one relocated later), so a
        // direct caller without the `#[app::state]` macro still converges.
        use crate::collections::{cell_id, compute_collection_id};
        use crate::entities::{full_mask, Data};

        env::reset_for_testing();
        env::set_account_id(ALICE);
        let _root: Root<TestVal> = Root::new(TestVal::default);

        let expected = cell_id(
            compute_collection_id(None, "doc"),
            &full_mask(writers(&[ALICE])),
        );
        let a = WriterSetCell::<TestVal>::new_with_field_name("doc", writers(&[ALICE]), false);
        assert_eq!(a.element().id(), expected);
        let b = WriterSetCell::<TestVal>::new_with_field_name("doc", writers(&[ALICE]), false);
        assert_eq!(
            b.element().id(),
            expected,
            "must be deterministic across constructions"
        );
    }

    #[test]
    #[serial]
    fn value_entry_is_member_anchored_and_untouched_by_rotation() {
        // The value entry is a `SharedMember` pointing at the wrapper (anchor).
        // It carries NO writer set and is NOT re-stamped on rotation: the writers
        // come from the host. This keeps a rotation from touching any stored byte,
        // so it cannot diverge the root hash. The newly-added writer can still
        // write afterward because authorization resolves from the anchor.
        use crate::collections::cell_value_id;
        use crate::entities::{Data, StorageType};
        use crate::index::Index;
        use crate::store::MainStorage;

        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE]), false));
        s.insert(TestVal(1)).unwrap();

        let wrapper_id = s.element().id();
        let value_id = cell_value_id(wrapper_id);
        let stored_of = |id| {
            <Index<MainStorage>>::get_metadata(id)
                .unwrap()
                .unwrap()
                .storage_type
        };
        let assert_member_of = |st: StorageType| match st {
            StorageType::SharedMember { anchor, .. } => assert_eq!(anchor, wrapper_id),
            other => panic!("value entry must be SharedMember, got {other:?}"),
        };
        let genesis = crate::entities::full_mask(writers(&[ALICE]));

        assert_member_of(stored_of(value_id));
        assert_eq!(
            stored_of(wrapper_id),
            StorageType::Shared {
                writers: genesis.clone(),
                signature_data: stored_signature(wrapper_id),
            }
        );

        s.rotate_writers(writers(&[ALICE, BOB])).unwrap();
        let stored_after = stored_of(wrapper_id);
        assert!(
            matches!(&stored_after, StorageType::Shared { writers, .. } if *writers == genesis),
            "a rotation is a request to the host: the stored anchor keeps its genesis set, got {stored_after:?}"
        );
        assert_member_of(stored_of(value_id));
        assert_eq!(s.writers(), writers(&[ALICE, BOB]));

        env::set_account_id(BOB);
        s.insert(TestVal(2)).unwrap();
        assert_member_of(stored_of(value_id));
    }

    fn stored_signature(id: crate::address::Id) -> Option<crate::entities::SignatureData> {
        match <crate::index::Index<crate::store::MainStorage>>::get_metadata(id)
            .unwrap()
            .unwrap()
            .storage_type
        {
            crate::entities::StorageType::Shared { signature_data, .. } => signature_data,
            other => panic!("expected a Shared anchor, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn a_rotation_is_recorded_for_the_host_with_the_set_it_started_from() {
        use crate::entities::{full_mask, Data};
        use crate::shared_writers::SharedRotation;

        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.rotate_writers(writers(&[BOB, CAROL])).unwrap();
        s.rotate_writers(writers(&[BOB])).unwrap_err();

        assert_eq!(
            env::take_recorded_rotations(),
            vec![SharedRotation {
                cell: s.element().id(),
                prior: full_mask(writers(&[ALICE, BOB])),
                new: full_mask(writers(&[BOB, CAROL])),
            }],
            "one request, stepping from the genesis set; the refused one records nothing"
        );
    }

    #[test]
    #[serial]
    fn a_rotation_writes_nothing_to_the_store() {
        use crate::entities::Data;
        use crate::store::{Key, MainStorage, StorageAdaptor as _};

        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(1)).unwrap();
        let id = s.element().id();
        let before = (
            MainStorage::storage_read(Key::Entry(id)),
            MainStorage::storage_read(Key::Index(id)),
            env::take_last_artifact(),
        );

        s.rotate_writers(writers(&[ALICE])).unwrap();

        assert_eq!(MainStorage::storage_read(Key::Entry(id)), before.0);
        assert_eq!(MainStorage::storage_read(Key::Index(id)), before.1);
        assert_eq!(
            env::take_last_artifact(),
            None,
            "no wrapper update rides the delta"
        );
    }

    #[test]
    #[serial]
    fn rotation_takes_an_admin_not_a_mere_writer() {
        use crate::entities::OpMask;

        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE]), false));
        s.rotate_writers_scoped(
            [(pk(ALICE), OpMask::FULL), (pk(BOB), OpMask::WRITE)]
                .into_iter()
                .collect(),
        )
        .unwrap();
        let _ = env::take_recorded_rotations();

        env::set_account_id(BOB);
        assert!(s.writable_by_me(), "bob writes");
        let err = s
            .rotate_writers(writers(&[BOB]))
            .expect_err("a writer without ADMIN may not rotate");
        assert!(
            err.to_string().to_lowercase().contains("admin"),
            "error should say admin, got: {err}"
        );
        assert!(env::take_recorded_rotations().is_empty());
    }

    #[test]
    #[serial]
    fn shared_collection_entries_inherit_writer_domain() {
        use crate::collections::{compute_id, LwwRegister, UnorderedMap};
        use crate::entities::{Data, StorageType};
        use crate::store::MainStorage;

        env::reset_for_testing();
        env::set_account_id([7u8; 32]);

        type Map = UnorderedMap<String, LwwRegister<String>>;

        let ws = writers(&[[7u8; 32]]);
        let mut guarded = Root::new(|| WriterSetCell::<Map>::new(ws.clone(), false));

        // Edit the inner map in place through the guarded `get_mut`, which
        // re-establishes the writer domain on the map element first.
        let _old = guarded
            .get_mut()
            .expect("get_mut")
            .insert("k".to_owned(), LwwRegister::new("v".to_owned()))
            .expect("insert");

        // The entry must be anchored to the wrapper — the whole subtree is
        // guarded at merge, not just the WriterSetCell wrapper entity. It
        // carries no inline writer set: the anchor pointer is the domain, and
        // writers resolve from the host.
        let wrapper_id = guarded.element().id();
        let map_id = <Map as Data>::id(guarded.get().expect("get"));
        let child = compute_id(map_id, "k".as_bytes());
        let metadata = crate::index::Index::<MainStorage>::get_metadata(child)
            .expect("load child")
            .expect("child exists");
        match metadata.storage_type {
            StorageType::SharedMember { anchor, .. } => assert_eq!(anchor, wrapper_id),
            other => panic!(
                "WriterSetCell<Map> entry must be a SharedMember anchored to the wrapper, \
                 got {other:?}"
            ),
        }
        crate::tests::common::assert_every_shared_entity_is_bound();
    }

    #[test]
    #[serial]
    fn get_returns_default_before_insert() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        assert_eq!(s.get().unwrap(), &TestVal::default());
    }

    #[test]
    #[serial]
    fn value_schema_version_and_writability_reflect_stored_metadata() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(42)).expect("writer inserts");

        // A writer's insert stamps the value at the binary's target schema version.
        assert_eq!(
            s.value_schema_version().unwrap(),
            Some(calimero_sdk::app::schema_version()),
        );

        // Writability tracks the writer set, not single ownership.
        assert!(s.writable_by_me()); // ALICE
        env::set_account_id(BOB);
        assert!(s.writable_by_me()); // BOB
        env::set_account_id(CAROL);
        assert!(!s.writable_by_me()); // CAROL is not a writer
    }

    #[test]
    #[serial]
    fn insert_by_writer_succeeds() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(42)).expect("alice (writer) inserts");
        assert_eq!(s.get().unwrap(), &TestVal(42));
    }

    #[test]
    #[serial]
    fn insert_by_non_writer_short_circuits() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[BOB, CAROL]), false));
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
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(1)).unwrap();

        s.rotate_writers(writers(&[BOB, CAROL]))
            .expect("alice (current writer) rotates");

        // Alice can no longer write.
        let err = s
            .insert(TestVal(99))
            .expect_err("alice removed from writer set must be rejected");
        assert!(err.to_string().to_lowercase().contains("writer"));

        // Bob (new writer) can.
        env::set_account_id(BOB);
        s.insert(TestVal(99)).expect("bob (new writer) inserts");
        assert_eq!(s.get().unwrap(), &TestVal(99));
        crate::tests::common::assert_every_shared_entity_is_bound();
    }

    #[test]
    #[serial]
    fn rotate_writers_by_non_writer_rejected() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE]), false));
        s.insert(TestVal(1)).unwrap();

        env::set_account_id(BOB);
        let err = s
            .rotate_writers(writers(&[BOB]))
            .expect_err("non-writer rotation must fail");
        assert!(err.to_string().to_lowercase().contains("writer"));
    }

    #[test]
    #[serial]
    fn rotate_to_empty_writer_set_rejected() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
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
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));

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
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE]), true));

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
    fn merge_does_not_adopt_peers_frozen() {
        // `frozen` is genesis-immutable and deliberately NOT merged from the
        // peer, so a forged root-state delta carrying `frozen = true` cannot
        // freeze an honest node's rotation. Merge leaves the local value intact
        // in both directions.
        env::reset_for_testing();
        env::set_account_id(ALICE);
        // Establish ROOT so the wrapper's `add_child_to(ROOT)` succeeds.
        let _root: Root<TestVal> = Root::new(TestVal::default);
        let mut a = WriterSetCell::<TestVal>::new(writers(&[ALICE]), false);
        let b = WriterSetCell::<TestVal>::new(writers(&[ALICE]), true);

        Mergeable::merge(&mut a, &b).unwrap();
        assert!(
            !a.frozen,
            "merge must NOT adopt the peer's frozen=true (forge resistance)"
        );

        // Reverse direction: a locally-frozen instance stays frozen (merge
        // doesn't clear it either — it just doesn't touch `frozen`).
        let _root2: Root<TestVal> = Root::new(TestVal::default);
        let mut a2 = WriterSetCell::<TestVal>::new(writers(&[ALICE]), true);
        let b2 = WriterSetCell::<TestVal>::new(writers(&[ALICE]), false);
        Mergeable::merge(&mut a2, &b2).unwrap();
        assert!(a2.frozen, "merge must not clear a locally-set frozen");
    }

    #[test]
    #[serial]
    fn frozen_at_construction_blocks_rotation() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), true));
        s.insert(TestVal(1)).unwrap();

        let err = s
            .rotate_writers(writers(&[ALICE]))
            .expect_err("rotation on frozen must fail");
        assert!(err.to_string().to_lowercase().contains("frozen"));
    }
}
