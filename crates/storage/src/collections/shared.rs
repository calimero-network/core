//! Group-writable storage with a mutable writer set.
//!
//! `SharedStorage<T>` wraps a single value writable by any signer in `writers`.
//! The writer set itself is rotatable by a current writer (unless
//! `writers_frozen`). Trust mirrors `UserStorage<T>`: the runtime signs each
//! write, peers verify the signature against the stored writer set at merge
//! time.

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
    /// after signing.
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

    /// Replace the value. The executor must be in the current writer set.
    /// Returns the previous value.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the executor is not in `writers`.
    pub fn insert(&mut self, value: T) -> Result<Option<T>, StoreError> {
        let executor: PublicKey = env::executor_id().into();
        if !self.writers.contains(&executor) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not a writer of this SharedStorage".to_owned(),
            )));
        }
        let old = mem::replace(&mut self.value, value);
        self.writers_nonce = self.writers_nonce.saturating_add(1);
        self.storage.update();
        Ok(Some(old))
    }

    /// Rotate the writer set. Must be called by a current writer; rejected if
    /// `writers_frozen` was set at construction or if `new_writers` is empty.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `writers_frozen` is set, if `new_writers`
    /// is empty (which would permanently lock the storage), or if the executor
    /// is not currently in the writer set.
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
        self.writers = new_writers.clone();
        self.writers_nonce = self.writers_nonce.saturating_add(1);
        self.storage.set_shared_domain(new_writers);
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

// Mergeable: delegate to inner T (which the spec requires to implement Mergeable).
// Verifier-gated at merge time, so this is only reached after signature checks pass.
impl<T, S> Mergeable for SharedStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        self.value.merge(&other.value)
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
