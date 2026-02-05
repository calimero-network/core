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

impl<V> UnorderedSet<V, MainStorage>
where
    V: BorshSerialize + BorshDeserialize,
{
    /// Create a new set collection with a random ID.
    ///
    /// Use this for nested collections stored as values in other maps.
    /// Merge happens by the parent map's key, so the nested collection's ID
    /// doesn't affect sync semantics.
    ///
    /// For top-level state fields, use `new_with_field_name` instead.
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
                CrdtType::UnorderedSet,
            ),
        }
    }

    /// Reassigns the set's ID to a deterministic ID based on field name.
    ///
    /// This is called by the `#[app::state]` macro after `init()` returns to ensure
    /// all top-level collections have deterministic IDs regardless of how they were
    /// created in `init()`.
    ///
    /// # Arguments
    /// * `field_name` - The name of the struct field containing this set
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.inner
            .reassign_deterministic_id_with_crdt_type(field_name, CrdtType::UnorderedSet);
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
        V: AsRef<[u8]> + PartialEq,
    {
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
        Ok(self.inner.entries()?.flatten().fuse())
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

impl<V> Eq for UnorderedSet<V> where V: Eq + BorshSerialize + BorshDeserialize {}

impl<V> PartialEq for UnorderedSet<V>
where
    V: PartialEq + BorshSerialize + BorshDeserialize,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn eq(&self, other: &Self) -> bool {
        let l = self.iter().unwrap();
        let r = other.iter().unwrap();

        l.eq(r)
    }
}

impl<V> Ord for UnorderedSet<V>
where
    V: Ord + BorshSerialize + BorshDeserialize,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let l = self.iter().unwrap();
        let r = other.iter().unwrap();

        l.cmp(r)
    }
}

impl<V> PartialOrd for UnorderedSet<V>
where
    V: PartialOrd + BorshSerialize + BorshDeserialize,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let l = self.iter().ok()?;
        let r = other.iter().ok()?;

        l.partial_cmp(r)
    }
}

impl<V> fmt::Debug for UnorderedSet<V>
where
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
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

impl<V> Default for UnorderedSet<V>
where
    V: BorshSerialize + BorshDeserialize,
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

    #[test]
    fn test_unordered_set_operations() {
        let mut set = Root::new(|| UnorderedSet::new());

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
        let mut set = Root::new(|| UnorderedSet::new());

        assert!(set.insert("value1".to_owned()).expect("insert failed"));
        assert!(set.insert("value2".to_owned()).expect("insert failed"));
        assert!(!set.insert("value2".to_owned()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        assert!(set.remove("value1").expect("remove failed"));

        assert_eq!(set.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_set_clear() {
        let mut set = Root::new(|| UnorderedSet::new());

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
        let mut set = Root::new(|| UnorderedSet::new());

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
