//! Ordered shared-keyspace vector with per-entry ownership.
//!
//! `AuthoredVector<V>` exposes a `Vector<V>` whose entries each carry a
//! `StorageType::User { owner }` stamp set to the pusher's public key at
//! `push` time. Any context member can push a new entry at the end; only the
//! entry's author can update or tombstone it. There is intentionally no
//! physical `remove` — shifting indices would complicate concurrent-push
//! merge semantics. Use `tombstone(idx)` to mark a slot as retracted.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{StoreError, Vector};
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::{MainStorage, StorageAdaptor};

/// A vector where each position is owned by the public key that pushed it.
///
/// Internally a `Vector<V>`. Each entry's `StorageType` is `User { owner }`
/// set at push time from `env::executor_id()`. Only the owner can `update`
/// or `tombstone` their entry.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct AuthoredVector<V, S: StorageAdaptor = MainStorage>
where
    V: BorshSerialize + BorshDeserialize,
{
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Vector<V, S>,
    storage: Element,
}

impl<V, S> core::fmt::Debug for AuthoredVector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AuthoredVector")
            .field("element", &self.storage)
            .finish()
    }
}

impl<V> AuthoredVector<V, MainStorage>
where
    V: BorshSerialize + BorshDeserialize,
{
    /// Creates a new, empty `AuthoredVector` with a random ID.
    pub fn new() -> Self {
        Self {
            inner: Vector::new(),
            storage: Element::new(None),
        }
    }

    /// Creates a new, empty `AuthoredVector` with a deterministic ID derived
    /// from `field_name`.
    pub fn new_with_field_name(field_name: &str) -> Self {
        let mut storage = Element::new_with_field_name(None, Some(field_name.to_string()));
        storage.metadata.crdt_type = Some(CrdtType::UserStorage);
        Self {
            inner: Vector::new_with_field_name(&format!("__authored_vector_{field_name}")),
            storage,
        }
    }

    /// Reassigns the collection's ID deterministically based on `field_name`.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        use super::compute_collection_id;
        let new_id = compute_collection_id(None, field_name);
        self.storage.reassign_id_and_field_name(new_id, field_name);
        self.storage.metadata.crdt_type = Some(CrdtType::UserStorage);
        self.inner
            .reassign_deterministic_id(&format!("__authored_vector_{field_name}"));
    }
}

impl<V> Default for AuthoredVector<V, MainStorage>
where
    V: BorshSerialize + BorshDeserialize,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<V, S> AuthoredVector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Pushes a new value at the end, stamping the current executor as owner.
    ///
    /// Returns the index of the new entry.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn push(&mut self, value: V) -> Result<usize, StoreError> {
        let owner: PublicKey = env::executor_id().into();
        let storage_type = StorageType::User {
            owner,
            signature_data: None,
        };
        self.inner.push_with_storage_type(value, storage_type)?;
        // Index of the pushed entry is len-1.
        let len = self.inner.len()?;
        Ok(len.saturating_sub(1))
    }

    /// Replaces the value at `index`. Only the entry's owner may call this.
    ///
    /// Fails with `NotFound` if `index` is out of bounds, or `ActionNotAllowed`
    /// if the current executor is not the owner of record.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `index` is out of bounds or the current
    /// executor is not the stored owner; `NotFound` if the underlying entry
    /// disappears mid-call; or any underlying storage error.
    pub fn update(&mut self, index: usize, value: V) -> Result<(), StoreError> {
        let (entry_id, stored_owner) = self.require_owner(index)?;

        let executor: PublicKey = env::executor_id().into();
        if stored_owner != executor {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredVector::update: not entry owner".to_owned(),
            )));
        }

        let _old = self
            .inner
            .update(index, value)?
            .ok_or(StoreError::StorageError(StorageError::NotFound(entry_id)))?;
        Ok(())
    }

    /// Tombstones the entry at `index` by overwriting it with `V::default()`.
    ///
    /// Only the entry's owner may call this. The entry's position is
    /// preserved — readers can filter tombstones out via an app-level check.
    ///
    /// # Errors
    /// Same as [`update`](Self::update).
    pub fn tombstone(&mut self, index: usize) -> Result<(), StoreError>
    where
        V: Default,
    {
        self.update(index, V::default())
    }

    /// Returns the value at `index`, if any.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn get(&self, index: usize) -> Result<Option<V>, StoreError> {
        self.inner.get(index)
    }

    /// Returns the public key of the owner at `index`, if the slot exists.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn owner_of(&self, index: usize) -> Result<Option<PublicKey>, StoreError> {
        let Some(id) = self.inner.entry_id_at(index)? else {
            return Ok(None);
        };
        let metadata = <Index<S>>::get_metadata(id).map_err(StoreError::StorageError)?;
        Ok(metadata.and_then(|m| match m.storage_type {
            StorageType::User { owner, .. } => Some(owner),
            _ => None,
        }))
    }

    /// Iterates over all values in insertion order.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn iter(&self) -> Result<impl Iterator<Item = V> + '_, StoreError> {
        self.inner.iter()
    }

    /// Returns the number of entries (including tombstoned slots).
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.inner.len()
    }

    #[cfg(test)]
    pub(crate) fn entry_id_at(
        &self,
        index: usize,
    ) -> Result<Option<crate::address::Id>, StoreError> {
        self.inner.entry_id_at(index)
    }

    fn require_owner(&self, index: usize) -> Result<(crate::address::Id, PublicKey), StoreError> {
        let id = self.inner.entry_id_at(index)?.ok_or_else(|| {
            // Fabricate a zero-id here is ugly; reuse a placeholder via NotFound with
            // a derived id if we had one. Index out of bounds → no id. Surface as
            // ActionNotAllowed with a clear message instead.
            StoreError::StorageError(StorageError::ActionNotAllowed(format!(
                "AuthoredVector: index {index} out of bounds",
            )))
        })?;
        let metadata = <Index<S>>::get_metadata(id).map_err(StoreError::StorageError)?;
        let owner = match metadata {
            Some(m) => match m.storage_type {
                StorageType::User { owner, .. } => owner,
                _ => {
                    return Err(StoreError::StorageError(StorageError::InvalidData(
                        "AuthoredVector entry missing User stamp".to_owned(),
                    )));
                }
            },
            None => return Err(StoreError::StorageError(StorageError::NotFound(id))),
        };
        Ok((id, owner))
    }
}

impl<V, S> Data for AuthoredVector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
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

impl<V, S> Mergeable for AuthoredVector<V, S>
where
    V: BorshSerialize + BorshDeserialize + Mergeable + Clone,
    S: StorageAdaptor,
{
    fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        self.inner.merge(&other.inner)
    }
}

impl<V, S> CrdtMeta for AuthoredVector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::UserStorage
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
    use calimero_primitives::identity::PublicKey;
    use serial_test::serial;

    use super::AuthoredVector;
    use crate::collections::Root;
    use crate::env;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];

    fn pk(bytes: [u8; 32]) -> PublicKey {
        bytes.into()
    }

    #[test]
    #[serial]
    fn push_stamps_current_executor_as_owner() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut v = Root::new(|| AuthoredVector::<u64>::new());
        let idx = v.push(7).expect("push");
        assert_eq!(idx, 0);
        assert_eq!(v.get(0).unwrap(), Some(7));
        assert_eq!(v.owner_of(0).unwrap(), Some(pk(ALICE)));
        assert_eq!(v.len().unwrap(), 1);
    }

    #[test]
    #[serial]
    fn concurrent_pushes_from_two_users_preserve_per_entry_owner() {
        env::reset_for_testing();

        let mut v = Root::new(|| AuthoredVector::<u64>::new());

        env::set_executor_id(ALICE);
        let a = v.push(1).unwrap();
        env::set_executor_id(BOB);
        let b = v.push(2).unwrap();
        env::set_executor_id(ALICE);
        let c = v.push(3).unwrap();

        assert_eq!((a, b, c), (0, 1, 2));
        assert_eq!(v.owner_of(0).unwrap(), Some(pk(ALICE)));
        assert_eq!(v.owner_of(1).unwrap(), Some(pk(BOB)));
        assert_eq!(v.owner_of(2).unwrap(), Some(pk(ALICE)));
    }

    #[test]
    #[serial]
    fn update_by_owner_succeeds() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut v = Root::new(|| AuthoredVector::<u64>::new());
        v.push(7).unwrap();
        v.update(0, 42).expect("owner update");
        assert_eq!(v.get(0).unwrap(), Some(42));
        assert_eq!(v.owner_of(0).unwrap(), Some(pk(ALICE)));
    }

    #[test]
    #[serial]
    fn update_by_non_owner_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut v = Root::new(|| AuthoredVector::<u64>::new());
        v.push(7).unwrap();

        env::set_executor_id(BOB);
        let err = v.update(0, 99).expect_err("non-owner update must fail");
        assert!(err.to_string().to_lowercase().contains("owner"));
        assert_eq!(v.get(0).unwrap(), Some(7));
        assert_eq!(v.owner_of(0).unwrap(), Some(pk(ALICE)));
    }

    #[test]
    #[serial]
    fn update_out_of_bounds_errors() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut v = Root::new(|| AuthoredVector::<u64>::new());
        let err = v.update(5, 1).expect_err("out-of-bounds update must fail");
        assert!(err.to_string().to_lowercase().contains("out of bounds"));
    }

    #[test]
    #[serial]
    fn tombstone_by_owner_writes_default() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut v = Root::new(|| AuthoredVector::<u64>::new());
        v.push(7).unwrap();
        v.tombstone(0).expect("owner tombstone");
        // u64::default() == 0
        assert_eq!(v.get(0).unwrap(), Some(0));
        // Position and owner are preserved.
        assert_eq!(v.len().unwrap(), 1);
        assert_eq!(v.owner_of(0).unwrap(), Some(pk(ALICE)));
    }

    #[test]
    #[serial]
    fn tombstone_by_non_owner_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut v = Root::new(|| AuthoredVector::<u64>::new());
        v.push(7).unwrap();

        env::set_executor_id(BOB);
        let err = v.tombstone(0).expect_err("non-owner tombstone must fail");
        assert!(err.to_string().to_lowercase().contains("owner"));
        assert_eq!(v.get(0).unwrap(), Some(7));
    }

    #[test]
    #[serial]
    fn iter_yields_all_values_in_insertion_order() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut v = Root::new(|| AuthoredVector::<u64>::new());
        v.push(10).unwrap();
        v.push(20).unwrap();
        env::set_executor_id(BOB);
        v.push(30).unwrap();

        let items: Vec<u64> = v.iter().unwrap().collect();
        assert_eq!(items, vec![10, 20, 30]);
    }

    #[test]
    #[serial]
    fn owner_of_out_of_bounds_is_none() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let v = Root::new(|| AuthoredVector::<u64>::new());
        assert_eq!(v.owner_of(0).unwrap(), None);
    }
}
