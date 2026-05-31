//! This module provides functionality for the unordered set data structure.

use core::borrow::Borrow;
use core::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeSeq;
use serde::Serialize;

use super::{compute_id, Collection, CrdtType};
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
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        let new_id = super::compute_collection_id(Some(parent_id), "__set");
        if self.inner.id() == new_id {
            return; // already deterministic — idempotent
        }
        let elements: Vec<V> = self.iter().expect("read set elements for re-key").collect();
        self.inner.clear().expect("clear set for re-key");
        self.inner.reassign_deterministic_id_under(
            Some(parent_id),
            "__set",
            CrdtType::unordered_set(std::any::type_name::<V>()),
        );
        for v in elements {
            let _ = self.insert(v).expect("re-insert set element during re-key");
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

        // Collect all elements before migration (must do this before clearing)
        let elements: Vec<V> = self
            .iter()
            .expect("failed to read elements for migration")
            .collect();

        // Clear the collection (removes old entries with old IDs)
        self.inner.clear().expect("failed to clear for migration");

        // Now reassign the collection's ID
        self.inner.reassign_deterministic_id_with_crdt_type(
            field_name,
            CrdtType::unordered_set(std::any::type_name::<V>()),
        );

        // Re-insert all elements (they will get new IDs based on new parent ID)
        for value in elements {
            self.insert(value)
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
        V: AsRef<[u8]> + PartialEq + 'static,
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
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
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
    use crate::store::MainStorage;

    #[test]
    fn test_unordered_set_operations() {
        let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

        assert!(set.insert("value1".to_owned()).expect("insert failed"));

        assert_eq!(
            set.contains(&"value1".to_owned()).expect("contains failed"),
            true
        );

        assert!(!set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));

        assert_eq!(set.contains("value3").expect("get failed"), false);
        assert_eq!(set.contains("value2").expect("get failed"), true);

        assert_eq!(
            set.remove("value1").expect("error while removing key"),
            true
        );
        assert_eq!(
            set.remove("value3").expect("error while removing key"),
            false
        );
    }

    #[test]
    fn test_unordered_set_len() {
        let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

        assert!(set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));
        assert!(!set.insert("value2".to_owned()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        assert!(set.remove("value1").expect("remove failed"));

        assert_eq!(set.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_set_clear() {
        let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

        assert!(set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        set.clear().expect("clear failed");

        assert_eq!(set.len().expect("len failed"), 0);
        assert_eq!(set.contains("value1").expect("contains failed"), false);
        assert_eq!(set.contains("value2").expect("contains failed"), false);
    }

    #[test]
    fn test_unordered_set_items() {
        let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

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
}
