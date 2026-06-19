//! This module provides functionality for the unordered set data structure.
//!
//! [`UnorderedSet`] is the **default** set collection: membership and full-scan
//! only, with **no iteration-order guarantee** (elements come back in hashed
//! entity-id order, not value order). Add-wins union CRDT — elements are never
//! lost once added.
//!
//! # Complexity (on a node)
//!
//! | Operation | Cost |
//! |---|---|
//! | `insert` / `contains` / `remove` | `O(1)` point lookup |
//! | `len` | `O(1)` |
//! | `iter` | `O(n)`, **unordered** |
//!
//! There is no separate index to maintain, so writes are as cheap as the
//! storage engine allows and no extra disk is used per element.
//!
//! # `UnorderedSet` vs [`SortedSet`](super::SortedSet)
//!
//! **Default to `UnorderedSet`.** Reach for [`SortedSet`](super::SortedSet)
//! *only* when you need elements in order — `range(a..b)`, `prefix("user:")`,
//! pagination, sorted iteration, or min/max. `SortedSet` answers those in
//! `O(log n + k)` via a maintained on-disk index, but pays for it on every write
//! (an extra index write + a validity-marker read/write), in extra disk per
//! element, and with an `O(n)` index rebuild on the first ordered read after a
//! sync. If you only ever test membership, that index is pure overhead — use
//! `UnorderedSet`. It is the `HashSet` to `SortedSet`'s `BTreeSet`.

use core::borrow::Borrow;
use core::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeSeq;
use serde::Serialize;

use super::{compute_id, Collection, CrdtType, StorageKey};
use crate::collections::error::StoreError;
use crate::entities::Data;
use crate::store::{MainStorage, StorageAdaptor};

/// A set collection that stores unqiue values once.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct UnorderedSet<V, S: StorageAdaptor = MainStorage> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<V, S>,
}

/// Re-key the set's inner collection (and its content-addressed members)
/// relative to its storage parent, so independently-created sets converge.
/// See [`super::rekey`].
impl<V, S> super::rekey::RekeyTarget for UnorderedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    S: StorageAdaptor,
{
    #[expect(clippy::expect_used, reason = "fatal error if re-key migration fails")]
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        let new_id = super::compute_collection_id(Some(parent_id), "__set");
        if self.inner.id() == new_id {
            return; // already deterministic — idempotent
        }
        // Snapshot `(value, storage_type)` so a guarded set's per-entry writer
        // stamp survives the clear+reinsert, and re-insert through the same
        // storage-type-preserving path the map uses (no divergent hand-rolled
        // re-inherit).
        let elements = self
            .inner
            .entries_with_storage_type()
            .expect("read set elements for re-key");
        self.inner.clear().expect("clear set for re-key");
        self.inner.reassign_deterministic_id_under(
            Some(parent_id),
            "__set",
            CrdtType::unordered_set(std::any::type_name::<V>()),
        );
        let parent = self.inner.id();
        for (v, storage_type) in elements {
            let id = super::compute_id(parent, v.as_ref());
            let _ = self
                .inner
                .insert_with_storage_type(Some(id), v, storage_type)
                .expect("re-insert set element during re-key");
        }
    }
}

impl<V, S> UnorderedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new set collection with a random ID.
    ///
    /// Use this for nested collections stored as values in other maps.
    /// Merge happens by the parent map's key, so the nested collection's ID
    /// doesn't affect sync semantics.
    ///
    /// For top-level state fields, use `new_with_field_name` instead.
    ///
    /// `S` is inferred from the binding context; default-generic is
    /// `MainStorage`. Inside `#[app::private]`, the macro substitutes
    /// `PrivateStorage` as `S` on the field type.
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new set collection with a deterministic ID.
    ///
    /// The `field_name` is used to generate a deterministic collection ID,
    /// ensuring the same code produces the same ID across all nodes.
    ///
    /// Use this for top-level state fields (the `#[app::state]` macro does this
    /// automatically).
    ///
    /// # Example
    /// ```ignore
    /// let tags = UnorderedSet::<String>::new_with_field_name("tags");
    /// ```
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

impl<V, S> UnorderedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new set collection.
    fn new_internal() -> Self {
        Self {
            inner: Collection::new(None),
        }
    }

    /// Create a new set collection with deterministic ID (internal)
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            inner: Collection::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::unordered_set(std::any::type_name::<V>()),
            ),
        }
    }

    /// Reassigns the set's ID to a deterministic ID based on field name.
    ///
    /// This is called by the `#[app::state]` macro after `init()` returns to ensure
    /// all top-level collections have deterministic IDs regardless of how they were
    /// created in `init()`.
    ///
    /// This method also migrates all existing elements to use the new parent ID,
    /// ensuring that elements inserted during `init()` remain accessible.
    ///
    /// # Arguments
    /// * `field_name` - The name of the struct field containing this set
    #[expect(clippy::expect_used, reason = "fatal error if migration fails")]
    pub fn reassign_deterministic_id(&mut self, field_name: &str)
    where
        V: AsRef<[u8]> + PartialEq + 'static,
    {
        use super::compute_collection_id;

        let new_id = compute_collection_id(None, field_name);
        let old_id = self.inner.id();

        // If already has the correct ID, nothing to do
        if old_id == new_id {
            return;
        }

        // Snapshot `(value, storage_type)` before clearing so a guarded set's
        // per-entry writer stamp survives the re-key. Mirrors `rekey_relative_to`
        // (the nested path); a plain `insert` would re-stamp every entry with the
        // collection's current domain and drop per-entry writer identity.
        let elements = self
            .inner
            .entries_with_storage_type()
            .expect("failed to read elements for migration");

        // Clear the collection (removes old entries with old IDs)
        self.inner.clear().expect("failed to clear for migration");

        // Now reassign the collection's ID
        self.inner.reassign_deterministic_id_with_crdt_type(
            field_name,
            CrdtType::unordered_set(std::any::type_name::<V>()),
        );

        // Re-insert all elements under the new parent ID, preserving each
        // entry's original storage type.
        let parent = self.inner.id();
        for (value, storage_type) in elements {
            let id = super::compute_id(parent, value.as_ref());
            let _ = self
                .inner
                .insert_with_storage_type(Some(id), value, storage_type)
                .expect("failed to re-insert element during migration");
        }
    }

    /// Insert a value pair into the set collection if the element does not already exist.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn insert(&mut self, value: V) -> Result<bool, StoreError>
    where
        V: StorageKey,
    {
        // Register this set type's nested-id re-key thunk so that when the set
        // is itself stored as a map/set/vector value, `insert`'s re-key path
        // can find it (see `super::rekey`).
        super::rekey::register_rekey::<Self>();
        let id = compute_id(self.inner.id(), value.as_ref());

        if self.inner.get_mut(id)?.is_some() {
            return Ok(false);
        };

        let _ignored = self.inner.insert(Some(id), value)?;

        Ok(true)
    }

    /// Get an iterator over the items in the set.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn iter(&self) -> Result<impl Iterator<Item = V> + '_, StoreError> {
        // See the matching ITER_DROP diagnostic on
        // `UnorderedMap::entries` — surfaces silent NotFound drops from
        // the inner iterator instead of swallowing them via
        // `.flatten().fuse()`.
        let collection_id = self.inner.id();
        Ok(self.inner.entries()?.filter_map(move |result| match result {
            Ok(item) => Some(item),
            Err(error) => {
                tracing::error!(
                    target: "calimero_storage::iter_drop",
                    %collection_id,
                    %error,
                    collection_type = "UnorderedSet",
                    "ITER_DROP: parent's child list advertises an id whose entry could not be loaded — \
                     likely entry-before-parent ordering race or storage inconsistency. \
                     Caller will see a truncated iteration."
                );
                None
            }
        }).fuse())
    }

    /// Get the number of items in the set.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn len(&self) -> Result<usize, StoreError> {
        self.inner.len()
    }

    /// Returns `true` if there are no entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Get the value for a key in the set.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn contains<Q>(&self, value: &Q) -> Result<bool, StoreError>
    where
        V: Borrow<Q>,
        Q: PartialEq + ?Sized + AsRef<[u8]>,
    {
        let id = compute_id(self.inner.id(), value.as_ref());

        self.inner.contains(id)
    }

    /// Remove a key from the set, returning the value at the key if it previously existed.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn remove<Q>(&mut self, value: &Q) -> Result<bool, StoreError>
    where
        V: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), value.as_ref());

        let Some(entry) = self.inner.get_mut(id)? else {
            return Ok(false);
        };

        let _ignored = entry.remove()?;

        Ok(true)
    }

    /// Clear the set, removing all items.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.inner.clear()
    }
}

impl<V, S> Eq for UnorderedSet<V, S>
where
    V: Eq + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
}

impl<V, S> PartialEq for UnorderedSet<V, S>
where
    V: PartialEq + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn eq(&self, other: &Self) -> bool {
        let l = self.iter().unwrap();
        let r = other.iter().unwrap();

        l.eq(r)
    }
}

impl<V, S> Ord for UnorderedSet<V, S>
where
    V: Ord + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let l = self.iter().unwrap();
        let r = other.iter().unwrap();

        l.cmp(r)
    }
}

impl<V, S> PartialOrd for UnorderedSet<V, S>
where
    V: PartialOrd + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let l = self.iter().ok()?;
        let r = other.iter().ok()?;

        l.partial_cmp(r)
    }
}

impl<V, S> fmt::Debug for UnorderedSet<V, S>
where
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, clippy::unwrap_in_result, reason = "'tis fine")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("UnorderedSet")
                .field("items", &self.inner)
                .finish()
        } else {
            f.debug_set().entries(self.iter().unwrap()).finish()
        }
    }
}

impl<V, S> Default for UnorderedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    S: StorageAdaptor,
{
    fn default() -> Self {
        // Register the nested-id re-key thunk at construction so a set first
        // created via `default()` (e.g. `entry(k).or_default()` on a
        // `Map<_, Set<..>>`) is re-keyed deterministically by its parent rather
        // than keeping a per-node random id. See `UnorderedMap`'s `Default`.
        super::rekey::register_rekey::<Self>();
        Self::new_internal()
    }
}

impl<V, S> Serialize for UnorderedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + Serialize,
    S: StorageAdaptor,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        let len = self.len().map_err(serde::ser::Error::custom)?;

        let mut seq = serializer.serialize_seq(Some(len))?;

        for v in self.iter().map_err(serde::ser::Error::custom)? {
            seq.serialize_element(&v)?;
        }

        seq.end()
    }
}

impl<V, S> Extend<V> for UnorderedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    S: StorageAdaptor,
{
    fn extend<I: IntoIterator<Item = V>>(&mut self, iter: I) {
        let parent = self.inner.id();

        let iter = iter.into_iter().map(|v| {
            let id = compute_id(parent, v.as_ref());

            (Some(id), v)
        });

        self.inner.extend(iter);
    }
}

impl<V, S> FromIterator<V> for UnorderedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    S: StorageAdaptor,
{
    fn from_iter<I: IntoIterator<Item = V>>(iter: I) -> Self {
        let mut map = UnorderedSet::new_internal();

        map.extend(iter);

        map
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::{Root, UnorderedSet};
    use crate::entities::Data;
    use crate::store::MainStorage;

    #[test]
    fn test_new_plus_reassign_matches_new_with_field_name() {
        // CIP I9 safety lock for dropping `new_with_field_name("x")` in favour of
        // plain `::new()`: `reassign_deterministic_id("x")` (run by the
        // `#[app::state]` post-init pass) MUST derive the same id
        // `new_with_field_name("x")` produces, or converted apps split-brain.
        crate::env::reset_for_testing();
        let explicit: UnorderedSet<String> = UnorderedSet::new_with_field_name("items");
        let mut via: UnorderedSet<String> = UnorderedSet::new();
        via.reassign_deterministic_id("items");
        assert_eq!(explicit.inner.id(), via.inner.id());
    }

    #[test]
    fn test_reassign_preserves_entries() {
        // Entries seeded before the post-init pass must survive the reassignment.
        crate::env::reset_for_testing();
        let mut set: UnorderedSet<String> = UnorderedSet::new();
        set.insert("alpha".to_owned()).expect("insert alpha");
        set.insert("beta".to_owned()).expect("insert beta");
        let old_id = set.inner.id();
        set.reassign_deterministic_id("items");
        assert_ne!(old_id, set.inner.id());
        assert!(set.contains("alpha").expect("contains alpha"));
        assert!(set.contains("beta").expect("contains beta"));
    }

    #[test]
    fn test_unordered_set_operations() {
        let mut set = Root::new(UnorderedSet::<_, MainStorage>::new);

        assert!(set.insert("value1".to_owned()).expect("insert failed"));

        assert!(set.contains(&"value1".to_owned()).expect("contains failed"));

        assert!(!set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));

        assert!(!set.contains("value3").expect("get failed"));
        assert!(set.contains("value2").expect("get failed"));

        assert!(set.remove("value1").expect("error while removing key"));
        assert!(!set.remove("value3").expect("error while removing key"));
    }

    #[test]
    fn test_unordered_set_len() {
        let mut set = Root::new(UnorderedSet::<_, MainStorage>::new);

        assert!(set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));
        assert!(!set.insert("value2".to_owned()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        assert!(set.remove("value1").expect("remove failed"));

        assert_eq!(set.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_set_clear() {
        let mut set = Root::new(UnorderedSet::<_, MainStorage>::new);

        assert!(set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        set.clear().expect("clear failed");

        assert_eq!(set.len().expect("len failed"), 0);
        assert!(!set.contains("value1").expect("contains failed"));
        assert!(!set.contains("value2").expect("contains failed"));
    }

    #[test]
    fn test_unordered_set_items() {
        let mut set = Root::new(UnorderedSet::<_, MainStorage>::new);

        assert!(set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));

        let items: Vec<String> = set.iter().expect("items failed").collect();

        assert_eq!(items.len(), 2);
        assert!(items.contains(&"value1".to_owned()));
        assert!(items.contains(&"value2".to_owned()));

        assert!(set.remove("value1").expect("remove failed"));
        let items: Vec<String> = set.iter().expect("items failed").collect();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn insert_inherits_collection_storage_domain() {
        use std::collections::BTreeSet;

        use calimero_primitives::identity::PublicKey;

        use crate::collections::compute_id;
        use crate::entities::StorageType;
        use crate::interface::Interface;
        use crate::store::MainStorage;

        crate::env::reset_for_testing();

        // A set inserts entries via the bare `Collection::insert`, so guarding the
        // set element propagates `Shared{writers}` to every member entity.
        let mut guarded = UnorderedSet::<String>::new();
        let writers: BTreeSet<PublicKey> = std::iter::once(PublicKey::from([7u8; 32])).collect();
        guarded
            .inner
            .element_mut()
            .set_shared_domain(writers.clone());
        let _ignored = guarded.insert("x".to_owned()).expect("insert");

        let set_id = guarded.inner.id();
        let child = compute_id(set_id, "x".as_bytes());
        let entry =
            <Interface<MainStorage>>::find_by_id::<crate::collections::Entry<String>>(child)
                .expect("load member entry")
                .expect("member entry exists");
        match entry.storage.metadata.storage_type {
            StorageType::Shared { writers: w, .. } => {
                assert_eq!(w, crate::entities::full_mask(writers.clone()))
            }
            other => panic!("set member must inherit Shared, got {other:?}"),
        }
    }

    // The top-level `#[app::state]` re-key path (`reassign_deterministic_id`)
    // must preserve each entry's per-entry `StorageType`, like the nested
    // `rekey_relative_to` path. Seed an entry with an explicit `Shared` stamp
    // while the collection domain stays `Main`, so the old `iter()` + `insert()`
    // path (which re-stamps with the collection's current Main domain) would
    // downgrade it — only the storage-type-preserving path keeps it `Shared`.
    #[test]
    fn reassign_preserves_per_entry_storage_type() {
        use std::collections::BTreeSet;

        use calimero_primitives::identity::PublicKey;

        use crate::collections::{compute_collection_id, compute_id};
        use crate::entities::{full_mask, StorageType};
        use crate::interface::Interface;
        use crate::store::MainStorage;

        crate::env::reset_for_testing();

        // Random inner id => the real clear+reinsert path (not the no-op).
        let mut set = UnorderedSet::<String>::new();
        let writers: BTreeSet<PublicKey> = std::iter::once(PublicKey::from([9u8; 32])).collect();
        let shared = StorageType::Shared {
            writers: full_mask(writers.clone()),
            signature_data: None,
        };
        let pre_id = compute_id(set.inner.id(), "x".as_bytes());
        let _ignored = set
            .inner
            .insert_with_storage_type(Some(pre_id), "x".to_owned(), shared)
            .expect("seed shared entry");

        set.reassign_deterministic_id("tags");

        // The re-keyed child lives under the deterministic parent id.
        let new_parent = compute_collection_id(None, "tags");
        let child = compute_id(new_parent, "x".as_bytes());
        let entry =
            <Interface<MainStorage>>::find_by_id::<crate::collections::Entry<String>>(child)
                .expect("load re-keyed entry")
                .expect("re-keyed entry exists");
        match entry.storage.metadata.storage_type {
            StorageType::Shared { writers: w, .. } => assert_eq!(w, full_mask(writers)),
            other => panic!("re-keyed set member must retain Shared, got {other:?}"),
        }
    }
}
