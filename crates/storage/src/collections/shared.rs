//! Group-writable storage with an authenticated, mutable writer set.
//!
//! `WriterSetCell<T>` wraps a single value writable by any signer in the
//! current writer set. The writer set itself is rotatable by a current writer
//! (unless `frozen`). Trust mirrors `UserStorage<T>`: the runtime signs each
//! write, peers verify the signature against the writer set at merge time.
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
//! The wrapper carries no CRDT value to merge at root-state time: the value is a
//! separate entity (merged per-entity) and the writer set converges via the
//! rotation log + ADR 0001 on the node side, not an LWW on root-state bytes.
//! `frozen` is genesis-immutable (set in `new`, no setter) — it rides root-state
//! borsh so joiners see it, but the merge deliberately does **not** adopt the
//! peer's `frozen`, so a forged root-state delta cannot freeze rotation on an
//! honest node.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{compute_collection_id, compute_id, Collection, StoreError};
use crate::address::Id;
use crate::entities::{ChildInfo, Data, Element, OpMask, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::store::{Key, MainStorage, StorageAdaptor};

/// Fixed sub-key under which the wrapper's single value entry is stored.
/// The value entry's id is `compute_id(wrapper_id, VALUE_KEY)` so every node
/// derives the same id for the value of a given wrapper.
const VALUE_KEY: &[u8] = b"__calimero_shared_value__";

/// Group-writable storage with an authenticated, mutable writer set.
///
/// A handle over a [`Collection`]: the wrapper entity is the `Shared` **anchor**
/// that owns the writer set + rotation log, and the value is held as one
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

impl<T> WriterSetCell<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
{
    /// Create a new WriterSetCell with a random ID and the given initial
    /// writer set. Use this for nested fields; the `#[app::state]` macro
    /// canonicalises the id via [`reassign_deterministic_id`] after `init`.
    ///
    /// [`reassign_deterministic_id`]: WriterSetCell::reassign_deterministic_id
    pub fn new(writers: BTreeSet<PublicKey>, frozen: bool) -> Self {
        let inner = Collection::new_shared(None, None, CrdtType::SharedStorage, writers.clone());
        Self::from_inner(inner, writers, frozen)
    }

    /// Create a new WriterSetCell with a deterministic ID derived from
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
        _writers: BTreeSet<PublicKey>,
        frozen: bool,
    ) -> Self {
        // The wrapper entity is the `Shared` anchor (stamped by `new_shared`
        // with `writers`); the value entry — and, when `T` is a collection,
        // everything beneath it — is a `SharedMember` pointing back at the
        // wrapper. The member carries no writer set; it resolves the anchor's
        // writers at verify time.
        let anchor = inner.id();
        let value_id = compute_id(anchor, VALUE_KEY);
        let value = inner
            .insert_with_storage_type(
                Some(value_id),
                T::default(),
                StorageType::SharedMember {
                    anchor,
                    signature_data: None,
                },
            )
            .expect("failed to write initial WriterSetCell value");
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
            .reassign_deterministic_id_with_crdt_type(field_name, CrdtType::SharedStorage);
        self.inner
            .element_mut()
            .set_shared_domain_scoped(writers.clone());
        let _saved = <Interface<MainStorage>>::save(&mut self.inner)
            .expect("failed to persist relocated WriterSetCell wrapper");

        // Re-write the value entry under the new wrapper id (always), stamped as
        // a member anchored to the new wrapper id.
        let new_anchor = self.inner.id();
        let new_value_id = compute_id(new_anchor, VALUE_KEY);
        let value = self
            .inner
            .insert_with_storage_type(
                Some(new_value_id),
                carried_value,
                StorageType::SharedMember {
                    anchor: new_anchor,
                    signature_data: None,
                },
            )
            .expect("failed to relocate WriterSetCell value");
        *self.value.borrow_mut() = Some(value);
    }
}

impl<T, S> WriterSetCell<T, S>
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
                .expect("read WriterSetCell value")
                .unwrap_or_default()
        });
        #[expect(unsafe_code, reason = "necessary for caching, mirrors Root::get")]
        let value = unsafe { &mut *core::ptr::from_mut(value) };
        value
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
    /// Currently infallible; the `Result` is preserved for forward compatibility.
    pub fn get_mut(&mut self) -> Result<&mut T, StoreError> {
        let anchor = self.inner.id();
        let value = self.load_value();
        // Stamp the value-collection element as a member anchored to the
        // wrapper. `Collection::insert` clones this element's `storage_type`
        // onto every entry, so all entries (at any depth) inherit the SAME
        // anchor — a flat domain whose writers live once, at the wrapper.
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
    /// Get a reference to the current value (anyone can read).
    ///
    /// # Errors
    /// Currently infallible; the `Result` is preserved for forward compatibility.
    pub fn get(&self) -> Result<&T, StoreError> {
        Ok(self.load_value())
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
    /// rotation-log-aware path as the write gate.
    pub fn writable_by_me(&self) -> bool {
        let executor: PublicKey = env::executor_id().into();
        self.current_writers().contains_key(&executor)
    }

    /// The current writer set, resolved only from **verified** local sources.
    ///
    /// Resolution order:
    /// 1. **Rotation log** (`rotation_log::load`), resolved via
    ///    [`rotation_log::resolve_local`]. On a node that *received* rotations,
    ///    the apply path appends every signature-verified rotation here.
    ///    `resolve_local` picks the live entry that is **max by
    ///    `(delta_hlc, signer)`** (falling back to the compaction snapshot when
    ///    there are no live entries).
    ///
    ///    This is the local-execution gate (core#2673). It has no DAG context,
    ///    so it cannot run the full causal `writers_at(parents)` the merge-time
    ///    verifier uses — but because the HLC is causally monotonic since #2635
    ///    (a rotation made after applying another carries a greater HLC), the
    ///    `(delta_hlc, signer)` max coincides with the causal latest for any
    ///    well-formed log, and — unlike the old `entries.last()` — it is
    ///    **insertion-order invariant**, so two nodes that applied the same
    ///    concurrent rotations gate against the *same* set. The merge check
    ///    (`writers_at`) remains the security boundary for the pathological
    ///    HLC-skew case; this gate is never weaker than `entries.last()` was.
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
    fn current_writers(&self) -> BTreeMap<PublicKey, OpMask> {
        // Delegate to the SINGLE resolver shared with the merge/verify path
        // (`Interface::resolve_anchor_writers`): it unions the hashed child
        // collection AND the side store, then runs the order-invariant
        // `resolve_local`. Using one function here is load-bearing — when this
        // gate's resolve diverged from the merge-time resolver (this used
        // `or_else`/collection-first while the other unioned), two nodes
        // resolved DIFFERENT writer sets for the same anchor (collection on the
        // originator, side store on a reconcile-only receiver), so a value
        // signed against one set failed verification on the other ("Invalid
        // signature for user-owned data") and the cluster split-brained on the
        // value entry under concurrent rotation. One resolver ⇒ one writer set.
        crate::interface::Interface::<S>::resolve_anchor_writers(self.inner.id())
    }

    /// Read the current writer set (membership only). Public so callers can
    /// present a "members with edit rights" UI and compute incremental
    /// rotations. Use [`capabilities`](Self::capabilities) for the per-writer
    /// [`OpMask`]s.
    pub fn writers(&self) -> BTreeSet<PublicKey> {
        self.current_writers().into_keys().collect()
    }

    /// The current writers with their [`OpMask`]s.
    pub fn capabilities(&self) -> BTreeMap<PublicKey, OpMask> {
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
    pub fn insert(&mut self, value: T) -> Result<Option<T>, StoreError> {
        let executor: PublicKey = env::executor_id().into();
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
        // the anchor's rotation log is the single source. (This is why the
        // old value-entry `set_storage_type` writer-patch is gone: a member's
        // `update_signature_in_place` matches on the anchor, which never
        // changes on rotation.)
        let member = StorageType::SharedMember {
            anchor: self.inner.id(),
            signature_data: None,
        };
        let new = self
            .inner
            .insert_with_storage_type(Some(value_id), value, member)?;
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
        // Convenience: every writer gets `OpMask::FULL` (today's behaviour).
        self.rotate_writers_scoped(new_writers.into_iter().map(|w| (w, OpMask::FULL)).collect())
    }

    /// Rotate the writer set with explicit per-writer [`OpMask`]s. Same rules and
    /// merge semantics as [`rotate_writers`](Self::rotate_writers); the masks are
    /// committed into the signed rotation and enforced at merge.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `frozen`, if `new_writers` is empty, or if
    /// the executor is not currently in the writer set.
    pub fn rotate_writers_scoped(
        &mut self,
        new_writers: BTreeMap<PublicKey, OpMask>,
    ) -> Result<(), StoreError> {
        if self.frozen {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Cannot rotate writers of frozen WriterSetCell".to_owned(),
            )));
        }
        if new_writers.is_empty() {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Cannot rotate to an empty writer set".to_owned(),
            )));
        }
        let executor: PublicKey = env::executor_id().into();
        let writers = self.current_writers();
        if !writers.contains_key(&executor) {
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
            .set_shared_domain_scoped(new_writers);
        let _saved = <Interface<S>>::save(&mut self.inner)?;

        // Members are NOT re-stamped. The value entry and every collection
        // child are `SharedMember`s pointing at this wrapper; their writer set
        // is resolved from the wrapper's rotation log at verify time, so the
        // single append above retroactively revokes (and grants) access for the
        // entire subtree at once. No per-entity `Update` storm — every member's
        // bytes are unchanged — so the rotation cannot diverge the root hash.
        // This is exactly why the variant was split: rotation is O(1) and
        // split-brain-safe by construction.

        // Originating-node fallback: persist the new set on the wrapper's index
        // (the rotation log is only appended on receivers, so this node's own
        // log stays empty and `current_writers` reads the index).
        let _ignored = <Index<S>>::set_storage_type(wrapper_id, new_shared);

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
// the rotation log, and `frozen` is genesis-immutable and deliberately not
// adopted from the peer (so a forged root-state delta can't freeze rotation).
#[diagnostic::do_not_recommend]
impl<T, S> Mergeable for WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn merge(&mut self, _other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        // Nothing to merge. The value is a separate entity (synced + merged
        // per-entity) and the writer set converges via the verified rotation log
        // (ADR 0001) on the node side — neither rides this merge.
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use borsh::{BorshDeserialize, BorshSerialize};
    use calimero_primitives::identity::PublicKey;
    use serial_test::serial;

    use super::WriterSetCell;
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
        // It carries NO writer set and is NOT re-stamped on rotation — the
        // wrapper's rotation log is the single source. This is the invariant
        // that makes rotation O(1) and split-brain-safe: a member's bytes never
        // change, so a rotation can't diverge the root hash. (The newly-added
        // writer can still write afterward because authorization resolves from
        // the anchor, not from a stale inline copy.)
        use crate::collections::compute_id;
        use crate::entities::{Data, StorageType};
        use crate::index::Index;
        use crate::store::MainStorage;

        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE]), false));
        s.insert(TestVal(1)).unwrap();

        let wrapper_id = s.element().id();
        let value_id = compute_id(wrapper_id, super::VALUE_KEY);
        let storage_type_of = |id| {
            <Index<MainStorage>>::get_metadata(id)
                .unwrap()
                .unwrap()
                .storage_type
        };
        let assert_member_of = |st: StorageType| match st {
            StorageType::SharedMember { anchor, .. } => assert_eq!(anchor, wrapper_id),
            other => panic!("value entry must be SharedMember, got {other:?}"),
        };
        let anchor_writers = |st: StorageType| match st {
            StorageType::Shared { writers, .. } => writers,
            other => panic!("wrapper must be a Shared anchor, got {other:?}"),
        };

        // Value entry anchors to the wrapper; the wrapper (anchor) holds writers.
        assert_member_of(storage_type_of(value_id));
        assert_eq!(
            anchor_writers(storage_type_of(wrapper_id)),
            crate::entities::full_mask(writers(&[ALICE]))
        );

        // Rotation updates the anchor only; the value entry is byte-untouched.
        s.rotate_writers(writers(&[ALICE, BOB])).unwrap();
        assert_member_of(storage_type_of(value_id));
        assert_eq!(
            anchor_writers(storage_type_of(wrapper_id)),
            crate::entities::full_mask(writers(&[ALICE, BOB]))
        );

        // The newly-added writer can write — authorization resolves from the
        // anchor's (rotated) writer set, and the entry stays an anchored member.
        env::set_executor_id(BOB);
        s.insert(TestVal(2)).unwrap();
        assert_member_of(storage_type_of(value_id));
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
        // writers resolve from the anchor's rotation log.
        let wrapper_id = guarded.element().id();
        let map_id = <Map as Data>::id(guarded.get().expect("get"));
        let child = compute_id(map_id, "k".as_bytes());
        let entry = <Interface<MainStorage>>::find_by_id::<
            crate::collections::Entry<(String, LwwRegister<String>)>,
        >(child)
        .expect("load child")
        .expect("child exists");
        match entry.storage.metadata.storage_type {
            StorageType::SharedMember { anchor, .. } => assert_eq!(anchor, wrapper_id),
            other => panic!(
                "WriterSetCell<Map> entry must be a SharedMember anchored to the wrapper, \
                 got {other:?}"
            ),
        }
    }

    #[test]
    #[serial]
    fn get_returns_default_before_insert() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        assert_eq!(s.get().unwrap(), &TestVal::default());
    }

    #[test]
    #[serial]
    fn value_schema_version_and_writability_reflect_stored_metadata() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(42)).expect("writer inserts");

        // A writer's insert stamps the value at the binary's target schema version.
        assert_eq!(
            s.value_schema_version().unwrap(),
            Some(calimero_sdk::app::schema_version()),
        );

        // Writability tracks the writer set, not single ownership.
        assert!(s.writable_by_me()); // ALICE
        env::set_executor_id(BOB);
        assert!(s.writable_by_me()); // BOB
        env::set_executor_id(CAROL);
        assert!(!s.writable_by_me()); // CAROL is not a writer
    }

    #[test]
    #[serial]
    fn insert_by_writer_succeeds() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), false));
        s.insert(TestVal(42)).expect("alice (writer) inserts");
        assert_eq!(s.get().unwrap(), &TestVal(42));
    }

    #[test]
    #[serial]
    fn insert_by_non_writer_short_circuits() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

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
        env::set_executor_id(ALICE);

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
        env::set_executor_id(BOB);
        s.insert(TestVal(99)).expect("bob (new writer) inserts");
        assert_eq!(s.get().unwrap(), &TestVal(99));
    }

    #[test]
    #[serial]
    fn rotate_writers_by_non_writer_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE]), false));
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
        env::set_executor_id(ALICE);

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
        env::set_executor_id(ALICE);

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
        env::set_executor_id(ALICE);
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
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| WriterSetCell::<TestVal>::new(writers(&[ALICE, BOB]), true));
        s.insert(TestVal(1)).unwrap();

        let err = s
            .rotate_writers(writers(&[ALICE]))
            .expect_err("rotation on frozen must fail");
        assert!(err.to_string().to_lowercase().contains("frozen"));
    }
}
