//! Group-writable storage with a mutable writer set.
//!
//! `SharedStorage<T>` wraps a single value writable by any signer in `writers`.
//! The writer set itself is rotatable by a current writer (unless
//! `writers_frozen`). Trust mirrors `UserStorage<T>`: the runtime signs each
//! write, peers verify the signature against the stored writer set at merge
//! time.
//!
//! # Merge semantics
//!
//! `SharedStorage` implements [`Mergeable`](super::crdt_meta::Mergeable) on two
//! axes:
//!
//! - **Inner value** — delegates to `T`'s own `Mergeable` impl, so a wrapped
//!   CRDT keeps its convergence semantics (counter sums, LWW wins, etc.).
//! - **Writer-set metadata** — resolved by `(writers_nonce, lexical content)`:
//!   the side with the higher nonce wins, with a deterministic tie-break on
//!   the serialised writer set so concurrent rotations from different signers
//!   converge to the same outcome on all replicas.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{compute_collection_id, StoreError, ROOT_ID};
use crate::entities::{ChildInfo, Data, Element, SignatureData};
use crate::env;
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::store::{Key, MainStorage, StorageAdaptor};

/// Group-writable storage with a mutable writer set.
///
/// When `T` is a **collection** (`UnorderedMap`, `UnorderedSet`, …), in-place
/// edits MUST go through [`get_mut`](SharedStorage::get_mut): it re-establishes
/// the `Shared{writers}` domain on the collection element so every entry
/// inserted through it is guarded at merge. The inner value is private and
/// [`get`](SharedStorage::get) is read-only, so `get_mut` is the only mutation
/// path — the writer set (`writers`, a persisted field) is the source of truth
/// and the element domain is re-derived from it on each `get_mut`, so the
/// guarantee survives reload without the element domain itself being persisted.
#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct SharedStorage<
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor = MainStorage,
> {
    /// The current value.
    value: T,
    /// Public keys authorized to write or rotate.
    writers: BTreeSet<PublicKey>,
    /// If true, `rotate_writers` is rejected. Set at construction; never cleared.
    writers_frozen: bool,
    /// Monotonic counter; incremented on every write or rotation.
    writers_nonce: u64,
    /// Storage element for this entity.
    storage: Element,
    /// Signature attached at the runtime layer; mirrored from the metadata
    /// after signing. Per spec — currently always `None` in v2; populated
    /// by the runtime sign path in a future iteration. Kept serialized for
    /// wire-format stability across v2 → future versions.
    signature_data: Option<SignatureData>,
    #[borsh(skip)]
    _adaptor: core::marker::PhantomData<S>,
}

impl<T> SharedStorage<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
{
    /// Create a new SharedStorage with a random ID and the given initial
    /// writer set. Use this for nested fields.
    pub fn new(writers: BTreeSet<PublicKey>, frozen: bool) -> Self {
        let mut storage = Element::new(None);
        storage.set_shared_domain(writers.clone());
        Self {
            value: T::default(),
            writers,
            writers_frozen: frozen,
            writers_nonce: 0,
            storage,
            signature_data: None,
            _adaptor: core::marker::PhantomData,
        }
    }

    /// Create a new SharedStorage with a deterministic ID derived from
    /// `field_name`. Use this for top-level state fields (the `#[app::state]`
    /// macro arranges this automatically).
    ///
    /// Registers the wrapper as a child of root so the metadata
    /// (writer set, signature) is persisted to the index.
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    pub fn new_with_field_name(
        field_name: &str,
        writers: BTreeSet<PublicKey>,
        frozen: bool,
    ) -> Self {
        let id = compute_collection_id(None, field_name);
        let mut storage = Element::new_with_field_name(Some(id), Some(field_name.to_string()));
        storage.metadata.crdt_type = Some(CrdtType::SharedStorage);
        storage.set_shared_domain(writers.clone());
        let mut this = Self {
            value: T::default(),
            writers,
            writers_frozen: frozen,
            writers_nonce: 0,
            storage,
            signature_data: None,
            _adaptor: core::marker::PhantomData,
        };
        let _ = <Interface<MainStorage>>::add_child_to(*ROOT_ID, &mut this)
            .expect("failed to register SharedStorage with root");
        this
    }

    /// Reassign the wrapper's ID to a deterministic one based on `field_name`.
    /// Called by the `#[app::state]` macro after `init()` returns to ensure
    /// the same ID across all nodes when the wrapper was created via
    /// `new()` (random ID) instead of `new_with_field_name()`.
    #[expect(clippy::expect_used, reason = "fatal error if cleanup fails")]
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        let new_id = compute_collection_id(None, field_name);
        let old_id = self.storage.id();
        if old_id == new_id {
            return;
        }
        let _ignored = MainStorage::storage_remove(Key::Entry(old_id));
        let _ignored = MainStorage::storage_remove(Key::Index(old_id));
        let _ = <Index<MainStorage>>::remove_child_reference_only(*ROOT_ID, old_id);
        self.storage.reassign_id_and_field_name(new_id, field_name);
        self.storage.metadata.crdt_type = Some(CrdtType::SharedStorage);
        // Re-establish the Shared metadata at the new ID before saving.
        self.storage.set_shared_domain(self.writers.clone());
        let _ = <Interface<MainStorage>>::add_child_to(*ROOT_ID, self)
            .expect("failed to re-register SharedStorage with new id");
    }
}

impl<T, S> SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Data,
    S: StorageAdaptor,
{
    /// Mutable access to a collection value (`UnorderedMap`, `UnorderedSet`, …)
    /// for in-place editing.
    ///
    /// Re-establishes the writer domain on the collection's element before
    /// handing out the reference, so every entry inserted through it inherits
    /// `Shared{writers}` and is guarded at merge — including after a reload,
    /// where the collection element's own domain may not have been persisted.
    /// Only collections (which implement [`Data`]) get this; a scalar value is
    /// edited via the whole-value replace path instead.
    ///
    /// # Rotation semantics (current)
    /// Each entry is stamped with the writer set current at the time it is
    /// written, carried inline on that entry. So after `rotate_writers`, new
    /// entries are guarded by the new set, but entries written before the
    /// rotation keep their own stamp and remain verifiable against the old set —
    /// rotation is forward-only, it does not retroactively revoke write access to
    /// existing entries. Making every entry re-resolve the writer set from the
    /// wrapper's rotation log at the op's causal cut (so a rotation revokes the
    /// whole subtree) needs the DAG-causal rotation machinery; doing it by
    /// re-stamping entries eagerly diverges the root hash across peers (see the
    /// note on `rotate_writers`). Until then, treat a guarded collection's writer
    /// set as effectively fixed for already-written entries.
    ///
    /// # Errors
    /// Currently infallible; the `Result` is preserved for forward compatibility.
    pub fn get_mut(&mut self) -> Result<&mut T, StoreError> {
        // Re-establish the domain only when it isn't already current, so a
        // read-mostly `get_mut` doesn't spuriously mark the element dirty.
        let already_current = matches!(
            &self.value.element().metadata.storage_type,
            crate::entities::StorageType::Shared { writers, .. } if writers == &self.writers
        );
        if !already_current {
            self.value
                .element_mut()
                .set_shared_domain(self.writers.clone());
        }
        Ok(&mut self.value)
    }
}

impl<T, S> SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    /// Get a reference to the current value.
    ///
    /// # Errors
    /// Currently infallible; the `Result` is preserved for forward compatibility
    /// (e.g., a future variant could lazy-load the value from storage).
    pub fn get(&self) -> Result<&T, StoreError> {
        Ok(&self.value)
    }

    /// Read the current writer set. Public so callers can present a
    /// "members with edit rights" UI and compute incremental rotations
    /// (current set + new mod) without mirroring the set elsewhere.
    pub fn writers(&self) -> &BTreeSet<PublicKey> {
        &self.writers
    }

    /// Whether the writer set has been frozen. Once frozen, `rotate_writers`
    /// is permanently rejected.
    pub fn is_frozen(&self) -> bool {
        self.writers_frozen
    }

    /// Returns the signature attached to the most recently applied state of
    /// this entity, if any. Reads from the wrapper field first; if unset
    /// (e.g., the wrapper was just deserialized and the field hasn't been
    /// mirrored in this execution), falls back to the metadata copy populated
    /// by `find_by_id` from the index.
    pub fn signature(&self) -> Option<SignatureData> {
        if self.signature_data.is_some() {
            return self.signature_data;
        }
        match &self.storage.metadata.storage_type {
            crate::entities::StorageType::Shared { signature_data, .. } => *signature_data,
            _ => None,
        }
    }

    /// Replace the value. The executor must be in the current writer set.
    ///
    /// Returns the previous value. Note: on the first call, returns
    /// `Some(T::default())` (not `None`) because the wrapper is initialized
    /// with `T::default()` per the spec — the wrapper has no "uninitialized"
    /// state to distinguish.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the executor is not in `writers`, or
    /// `NonceOverflow` if `writers_nonce` would exceed `u64::MAX`.
    pub fn insert(&mut self, value: T) -> Result<Option<T>, StoreError> {
        let executor: PublicKey = env::executor_id().into();
        if !self.writers.contains(&executor) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not a writer of this SharedStorage".to_owned(),
            )));
        }
        let next_nonce = self.writers_nonce.checked_add(1).ok_or_else(|| {
            StoreError::StorageError(StorageError::ActionNotAllowed(
                "writers_nonce overflow".to_owned(),
            ))
        })?;
        let old = mem::replace(&mut self.value, value);
        self.writers_nonce = next_nonce;
        self.storage.update();
        // (v2 attempted to emit a per-entity Update action here so the
        // merge-time verifier on remote peers would run. Disabled: it
        // breaks cross-node sync because the wrapper also propagates inline
        // via root-state borsh, and the dual-write path causes a WASM trap
        // during `__calimero_sync_next`. Per-entity verification will become
        // live as part of the DAG-causal epic #2233 with a proper design.)
        Ok(Some(old))
    }

    /// Rotate the writer set. Must be called by a current writer; rejected if
    /// `writers_frozen` was set at construction or if `new_writers` is empty.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `writers_frozen` is set, if `new_writers`
    /// is empty (which would permanently lock the storage), if the executor is
    /// not currently in the writer set, or if `writers_nonce` would overflow.
    pub fn rotate_writers(&mut self, new_writers: BTreeSet<PublicKey>) -> Result<(), StoreError> {
        if self.writers_frozen {
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
        if !self.writers.contains(&executor) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not a current writer".to_owned(),
            )));
        }
        let next_nonce = self.writers_nonce.checked_add(1).ok_or_else(|| {
            StoreError::StorageError(StorageError::ActionNotAllowed(
                "writers_nonce overflow".to_owned(),
            ))
        })?;
        self.writers = new_writers.clone();
        self.writers_nonce = next_nonce;
        self.storage.set_shared_domain(new_writers);
        // (v2 attempted to emit a per-entity Update action here so the
        // merge-time verifier on remote peers would run against the rotation.
        // Disabled: the wrapper also propagates inline via root-state borsh,
        // and the dual-write path makes the receiver compute a different root
        // hash from the sender — every rotation produces a permanent
        // divergence. Per-entity live verification will become safe once the
        // DAG-causal epic #2233 lands and we can drop the root-state path.)
        Ok(())
    }
}

// Implement Data so SharedStorage can be nested in #[app::state].
impl<T, S> Data for SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
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

// Mergeable: invoked at root-state merge time when concurrent state versions
// must converge. Merges value via its own Mergeable, and resolves writer-set
// state by `writers_nonce` (higher wins, content as deterministic tiebreaker).
// `writers_frozen` is monotonic — once true on either side, stays true.
impl<T, S> Mergeable for SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        self.value.merge(&other.value)?;

        // Writer-set: higher writers_nonce wins. On tie, lexically smaller set
        // wins (deterministic across nodes).
        //
        // Guard against accepting an empty writer set from a peer — this would
        // permanently lock the storage (no one could write or rotate again).
        // The local API rejects empty rotations; mirror that here so a tampered
        // or buggy peer can't propagate a lockout via merge.
        //
        // Important: do NOT call `self.storage.set_shared_domain(...)` here.
        // That would mark the wrapper element dirty, which on the receiving
        // node makes `commit_root` emit a per-entity Update action that the
        // sender never produced — divergent DAG, divergent root hash.
        // The wrapper's `writers` field on the struct (borsh-serialized) is
        // the source of truth on the wire; metadata's storage_type only
        // matters for actions emitted by the originator, not by the merger.
        if !other.writers.is_empty()
            && (other.writers_nonce > self.writers_nonce
                || (other.writers_nonce == self.writers_nonce && other.writers < self.writers))
        {
            self.writers = other.writers.clone();
            self.writers_nonce = other.writers_nonce;
        }

        // Frozen is monotonic.
        if other.writers_frozen {
            self.writers_frozen = true;
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
    fn shared_collection_entries_inherit_writer_domain() {
        use crate::collections::{compute_id, LwwRegister, UnorderedMap};
        use crate::entities::{Data, StorageType};
        use crate::interface::Interface;
        use crate::store::MainStorage;

        env::reset_for_testing();

        type Map = UnorderedMap<String, LwwRegister<String>>;

        let ws = writers(&[[7u8; 32]]);
        let mut guarded: SharedStorage<Map> = SharedStorage::new(ws.clone(), false);

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
    fn shared_collection_guards_entries_after_reload() {
        use borsh::{from_slice, to_vec};

        use crate::collections::{compute_id, LwwRegister, UnorderedMap};
        use crate::entities::{Data, StorageType};
        use crate::interface::Interface;
        use crate::store::MainStorage;

        env::reset_for_testing();

        type Map = UnorderedMap<String, LwwRegister<String>>;

        let ws = writers(&[[7u8; 32]]);
        let mut guarded: SharedStorage<Map> = SharedStorage::new(ws.clone(), false);
        let _old = guarded
            .get_mut()
            .expect("get_mut")
            .insert("a".to_owned(), LwwRegister::new("1".to_owned()))
            .expect("insert a");

        // Simulate a reload: borsh round-trip the wrapper. The in-memory writer
        // domain on the collection element is dropped, but `writers` (a persisted
        // field) survives — it is the source of truth.
        let bytes = to_vec(&guarded).expect("serialize wrapper");
        let mut reloaded: SharedStorage<Map> = from_slice(&bytes).expect("deserialize wrapper");
        assert_eq!(reloaded.writers(), &ws, "writer set must survive reload");

        // After reload, `get_mut` re-derives the domain from the persisted writer
        // set, so a NEW entry is still guarded — the guarantee does not depend on
        // the element domain itself persisting.
        let _old = reloaded
            .get_mut()
            .expect("get_mut after reload")
            .insert("b".to_owned(), LwwRegister::new("2".to_owned()))
            .expect("insert b");

        let map_id = <Map as Data>::id(reloaded.get().expect("get"));
        let child_b = compute_id(map_id, "b".as_bytes());
        let entry = <Interface<MainStorage>>::find_by_id::<
            crate::collections::Entry<(String, LwwRegister<String>)>,
        >(child_b)
        .expect("load child b")
        .expect("child b exists");
        match entry.storage.metadata.storage_type {
            StorageType::Shared { writers: w, .. } => assert_eq!(w, ws),
            other => panic!("entry written after reload must be guarded, got {other:?}"),
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
        assert_eq!(s.writers(), &writers(&[ALICE, BOB]));
        assert!(!s.is_frozen());

        // After a rotation, the accessor sees the new set without the
        // caller having to mirror it elsewhere.
        s.rotate_writers(writers(&[ALICE, CAROL])).unwrap();
        assert_eq!(s.writers(), &writers(&[ALICE, CAROL]));
        assert!(!s.is_frozen());
    }

    #[test]
    #[serial]
    fn is_frozen_accessor_reflects_construction_and_blocks_rotation() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut s = Root::new(|| SharedStorage::<TestVal>::new(writers(&[ALICE]), true));

        assert!(s.is_frozen());
        assert_eq!(s.writers(), &writers(&[ALICE]));

        // A frozen instance must reject rotation regardless of caller.
        let err = s
            .rotate_writers(writers(&[ALICE, BOB]))
            .expect_err("rotation on frozen instance must fail");
        assert!(
            err.to_string().to_lowercase().contains("frozen"),
            "error should mention frozen, got: {err}"
        );

        // The set is unchanged after the rejected rotation.
        assert_eq!(s.writers(), &writers(&[ALICE]));
    }

    #[test]
    #[serial]
    fn merge_higher_writers_nonce_wins() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut a = SharedStorage::<TestVal>::new(writers(&[ALICE]), false);

        env::set_executor_id(BOB);
        let mut b = SharedStorage::<TestVal>::new(writers(&[BOB]), false);
        // Bump b's nonce by performing a rotation.
        b.rotate_writers(writers(&[BOB, CAROL])).unwrap();
        let bob_nonce = b.writers_nonce;
        assert!(bob_nonce > a.writers_nonce);

        Mergeable::merge(&mut a, &b).unwrap();
        assert_eq!(a.writers, writers(&[BOB, CAROL]));
        assert_eq!(a.writers_nonce, bob_nonce);
    }

    #[test]
    #[serial]
    fn merge_frozen_is_monotonic() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut a = SharedStorage::<TestVal>::new(writers(&[ALICE]), false);
        let b = SharedStorage::<TestVal>::new(writers(&[ALICE]), true);

        Mergeable::merge(&mut a, &b).unwrap();
        assert!(a.writers_frozen, "frozen=true on b should propagate to a");

        // Reverse direction: frozen stays once set.
        let mut a2 = SharedStorage::<TestVal>::new(writers(&[ALICE]), true);
        let b2 = SharedStorage::<TestVal>::new(writers(&[ALICE]), false);
        Mergeable::merge(&mut a2, &b2).unwrap();
        assert!(
            a2.writers_frozen,
            "frozen=true on a2 must not be cleared by merge"
        );
    }

    #[test]
    #[serial]
    fn merge_tiebreak_lexically_smaller_wins() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        // Force equal nonces and different writer sets to exercise the tiebreaker.
        let mut a = SharedStorage::<TestVal>::new(writers(&[BOB]), false);
        let b = SharedStorage::<TestVal>::new(writers(&[ALICE]), false);
        // Both at nonce=0, different content. ALICE's pubkey < BOB's pubkey.
        Mergeable::merge(&mut a, &b).unwrap();
        assert_eq!(a.writers, writers(&[ALICE]));
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
