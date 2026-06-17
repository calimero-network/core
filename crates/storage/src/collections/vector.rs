//! This module provides functionality for the vector data structure.

use core::borrow::Borrow;
use core::fmt;
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeSeq;
use serde::Serialize;

use super::{Collection, CrdtType, ValueRef};
use crate::collections::error::StoreError;
use crate::entities::Data;
use crate::store::{MainStorage, StorageAdaptor};

/// Validates that an index is safe for iterator arithmetic.
///
/// This function ensures that the index won't cause issues when used with
/// iterator methods that may perform internal arithmetic. Out-of-bounds
/// indices are handled by iterator methods returning `None`.
#[inline]
fn validate_index_bounds(index: usize) -> Result<(), StoreError> {
    // First check for potential overflow: index + 1 must not overflow
    // This is checked regardless of bounds since it's a safety invariant
    let _ = index.checked_add(1).ok_or_else(|| {
        StoreError::ArithmeticOverflow(format!(
            "addition overflow: {} + {} exceeds usize::MAX",
            index, 1
        ))
    })?;
    Ok(())
}

/// A vector collection that stores key-value pairs.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Vector<V, S: StorageAdaptor = MainStorage> {
    // Borrow/ToOwned
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<V, S>,
}

/// Re-key the vector's inner collection (and its index-keyed children) relative
/// to its storage parent so a vector stored as a collection value converges.
/// See [`super::rekey`].
impl<V, S> super::rekey::RekeyTarget for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.inner
            .reassign_deterministic_id_with_indexed_children_under(
                Some(parent_id),
                "__vector",
                CrdtType::vector(std::any::type_name::<V>()),
            );
    }
}

impl<V, S> Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new vector collection with a random ID.
    ///
    /// Use this for nested collections stored as values in other maps.
    /// Merge happens by the parent map's key, so the nested collection's ID
    /// doesn't affect sync semantics.
    ///
    /// For top-level state fields, use `new_with_field_name` instead.
    ///
    /// `S` is inferred from the binding context; default-generic is
    /// `MainStorage`. Inside `#[app::private]`, the macro substitutes
    /// `PrivateStorage` as `S` on the field type, and this constructor
    /// infers `S = PrivateStorage` at the assignment site.
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new vector collection with a deterministic ID.
    ///
    /// The `field_name` is used to generate a deterministic collection ID,
    /// ensuring the same code produces the same ID across all nodes.
    ///
    /// Use this for top-level state fields (the `#[app::state]` macro does this
    /// automatically).
    ///
    /// # Example
    /// ```ignore
    /// let items = Vector::<String>::new_with_field_name("items");
    /// ```
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

impl<V, S> Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new vector collection (internal, shared with decompose).
    pub(super) fn new_internal() -> Self {
        Self {
            inner: Collection::new(None),
        }
    }

    /// Create a new vector collection with deterministic ID (internal)
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            inner: Collection::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::vector(std::any::type_name::<V>()),
            ),
        }
    }

    /// Reassigns the vector's ID to a deterministic ID based on field name.
    ///
    /// This is called by the `#[app::state]` macro after `init()` returns to ensure
    /// all top-level collections have deterministic IDs regardless of how they were
    /// created in `init()`.
    ///
    /// # Arguments
    /// * `field_name` - The name of the struct field containing this vector
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        // `_with_indexed_children` (not the plain `_with_crdt_type`): vector
        // elements are inserted with `Id::random()` at `push` time, so the
        // generic reassign — which only relocates the collection's own id —
        // would leave per-node-random element ids behind and diverge when a
        // migration re-runs the population independently on each node. The
        // indexed variant re-keys each element by its append position.
        self.inner.reassign_deterministic_id_with_indexed_children(
            field_name,
            CrdtType::vector(std::any::type_name::<V>()),
        );
    }

    /// Add a value to the end of the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn push(&mut self, value: V) -> Result<(), StoreError>
    where
        V: 'static,
    {
        // Register this vector type's nested-id re-key thunk so a vector stored
        // as a collection value is re-keyed when the outer collection is stored.
        super::rekey::register_rekey::<Self>();
        let _ignored = self.inner.insert(None, value)?;

        Ok(())
    }

    /// Push a value with an explicit `StorageType` on the new entry's element.
    ///
    /// Used by `AuthoredVector` to stamp each push with the executor as owner.
    /// Returns the index of the newly inserted entry.
    pub(crate) fn push_with_storage_type(
        &mut self,
        value: V,
        storage_type: crate::entities::StorageType,
    ) -> Result<usize, StoreError> {
        let _ignored = self
            .inner
            .insert_with_storage_type(None, value, storage_type)?;
        let len = self.inner.len()?;
        debug_assert!(
            len >= 1,
            "Vector::push_with_storage_type: len must be >= 1 after a successful push",
        );
        Ok(len - 1)
    }

    /// Returns the storage id of the entry at `index`, or `None` if out of bounds.
    ///
    /// Used by `AuthoredVector` to look up per-entry metadata for authorization.
    pub(crate) fn entry_id_at(
        &self,
        index: usize,
    ) -> Result<Option<crate::address::Id>, StoreError> {
        validate_index_bounds(index)?;
        self.inner.nth(index)
    }

    /// Remove and return the last value from the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn pop(&mut self) -> Result<Option<V>, StoreError> {
        let Some(last) = self.inner.last()? else {
            return Ok(None);
        };

        let Some(entry) = self.inner.get_mut(last)? else {
            return Ok(None);
        };

        let last = entry.remove()?;

        Ok(Some(last))
    }

    /// Get the value at a specific index in the vector.
    ///
    /// Returns a read-only [`ValueRef`] guard (an owned copy that derefs to
    /// `&V`). To *mutate* the stored value use [`update`](Self::update) or
    /// [`get_mut`](Self::get_mut), or `.clone()` the guard for an owned copy when
    /// `V: Clone`.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned. Returns an error if the index would cause arithmetic overflow.
    ///
    pub fn get(&self, index: usize) -> Result<Option<ValueRef<V>>, StoreError> {
        validate_index_bounds(index)?;
        Ok(self
            .inner
            .entries()?
            .nth(index)
            .transpose()?
            .map(ValueRef::new))
    }

    /// Update the value at a specific index in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned. Returns an error if the index would cause arithmetic overflow.
    ///
    pub fn update(&mut self, index: usize, mut value: V) -> Result<Option<V>, StoreError>
    where
        V: 'static,
    {
        validate_index_bounds(index)?;

        let Some(id) = self.inner.nth(index)? else {
            return Ok(None);
        };

        // Re-key nested collections in the replacement value relative to the
        // existing element id (which is shared across nodes). Two nodes that
        // concurrently `update` the same positional element with a freshly-built
        // nested CRDT would otherwise mint divergent random internal ids. `push`
        // needs no such re-key: it mints a fresh RANDOM element id that rides
        // along in the sync delta (single-writer append, never independently
        // re-created), so the value's nested ids are shipped, not re-derived.
        super::rekey::rekey_nested_value(&mut value, id);

        let Some(mut entry) = self.inner.get_mut(id)? else {
            return Ok(None);
        };

        let old = mem::replace(&mut *entry, value);

        Ok(Some(old))
    }

    /// Get an iterator over the items in the vector.
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
                    collection_type = "Vector",
                    "ITER_DROP: parent's child list advertises an id whose entry could not be loaded — \
                     likely entry-before-parent ordering race or storage inconsistency. \
                     Caller will see a truncated iteration."
                );
                None
            }
        }).fuse())
    }

    /// Get the last value in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn last(&self) -> Result<Option<V>, StoreError> {
        let Some(last) = self.inner.last()? else {
            return Ok(None);
        };

        self.inner.get(last)
    }

    /// Get the number of items in the vector.
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

    /// Returns `true` if the vector holds no elements.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Get the value for a key in the vector.
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
        Q: PartialEq,
    {
        for entry in self.iter()? {
            if value == entry.borrow() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Clear the vector, removing all items.
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

    /// Find the first element that matches the predicate and return an iterator to it.
    ///
    /// Returns an iterator that yields at most one element (the first match).
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn find<F>(&self, mut predicate: F) -> Result<impl Iterator<Item = V>, StoreError>
    where
        F: FnMut(&V) -> bool,
    {
        let found = self.iter()?.find(|item| predicate(item));
        Ok(found.into_iter())
    }

    /// Filter elements that match the predicate and return an iterator over all matches.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn filter<'a, F>(&'a self, predicate: F) -> Result<impl Iterator<Item = V> + 'a, StoreError>
    where
        F: FnMut(&V) -> bool + 'a,
    {
        Ok(self.iter()?.filter(predicate))
    }
}

impl<V, S> Eq for Vector<V, S>
where
    V: Eq + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
}

impl<V, S> PartialEq for Vector<V, S>
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

impl<V, S> Ord for Vector<V, S>
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

impl<V, S> PartialOrd for Vector<V, S>
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

impl<V, S> fmt::Debug for Vector<V, S>
where
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, clippy::unwrap_in_result, reason = "'tis fine")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("Vector")
                .field("items", &self.inner)
                .finish()
        } else {
            f.debug_list().entries(self.iter().unwrap()).finish()
        }
    }
}

impl<V, S> Default for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn default() -> Self {
        // Register the nested-id re-key thunk at construction so a vector first
        // created via `default()` (e.g. `entry(k).or_default()` on a
        // `Map<_, Vector<..>>`) is re-keyed deterministically by its parent
        // rather than keeping a per-node random id. See `UnorderedMap`'s `Default`.
        super::rekey::register_rekey::<Self>();
        Self::new_internal()
    }
}

impl<V, S> Serialize for Vector<V, S>
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

        for entry in self.iter().map_err(serde::ser::Error::custom)? {
            seq.serialize_element(&entry)?;
        }

        seq.end()
    }
}

impl<V, S> Extend<V> for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    S: StorageAdaptor,
{
    fn extend<I: IntoIterator<Item = V>>(&mut self, iter: I) {
        let iter = iter.into_iter().map(|v| (None, v));

        self.inner.extend(iter);
    }
}

impl<V, S> FromIterator<V> for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    S: StorageAdaptor,
{
    fn from_iter<I: IntoIterator<Item = V>>(iter: I) -> Self {
        let mut map = Vector::new_internal();

        map.extend(iter);

        map
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::{Root, Vector};
    use crate::store::MainStorage;

    #[test]
    fn test_vector_push() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let value = "test_data".to_owned();
        let result = vector.push(value.clone());
        assert!(result.is_ok());
        assert_eq!(vector.len().unwrap(), 1);
    }

    #[test]
    fn test_vector_get() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        let retrieved_value = vector.get(0).unwrap().map(|v| v.into_inner());
        assert_eq!(retrieved_value, Some(value));
    }

    #[test]
    fn test_vector_update() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let value1 = "test_data1".to_owned();
        let value2 = "test_data2".to_owned();
        let _ = vector.push(value1.clone()).unwrap();
        let old = vector.update(0, value2.clone()).unwrap();
        let retrieved_value = vector.get(0).unwrap().map(|v| v.into_inner());
        assert_eq!(retrieved_value, Some(value2));
        assert_eq!(old, Some(value1));
    }

    #[test]
    fn test_vector_get_non_existent() {
        let vector = Root::new(|| Vector::<String>::new());

        match vector.get(0) {
            Ok(retrieved_value) => assert_eq!(retrieved_value, None),
            Err(e) => panic!("Error occurred: {:?}", e),
        }
    }

    #[test]
    fn test_vector_pop() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        let popped_value = vector.pop().unwrap();
        assert_eq!(popped_value, Some(value));
        assert_eq!(vector.len().unwrap(), 0);
    }

    #[test]
    fn test_vector_items() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let value1 = "test_data1".to_owned();
        let value2 = "test_data2".to_owned();
        let _ = vector.push(value1.clone()).unwrap();
        let _ = vector.push(value2.clone()).unwrap();
        let items: Vec<String> = vector.iter().unwrap().collect();
        assert_eq!(items, vec![value1, value2]);
    }

    #[test]
    fn test_vector_contains() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        assert!(vector.contains(&value).unwrap());
        let non_existent_value = "non_existent".to_owned();
        assert!(!vector.contains(&non_existent_value).unwrap());
    }

    #[test]
    fn test_vector_clear() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        vector.clear().unwrap();
        assert_eq!(vector.len().unwrap(), 0);
    }

    #[test]
    fn test_vector_find() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let _ = vector.push("apple".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();
        let _ = vector.push("cherry".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();

        // Find first element that starts with 'b'
        let result: Vec<String> = vector.find(|s| s.starts_with('b')).unwrap().collect();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "banana");

        // Find element that doesn't exist
        let result: Vec<String> = vector.find(|s| s.starts_with('z')).unwrap().collect();
        assert_eq!(result.len(), 0);

        // Find first element (any)
        let result: Vec<String> = vector.find(|_| true).unwrap().collect();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "apple");
    }

    #[test]
    fn test_vector_filter() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let _ = vector.push("apple".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();
        let _ = vector.push("cherry".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();
        let _ = vector.push("apricot".to_owned()).unwrap();

        // Filter all elements that start with 'a'
        let result: Vec<String> = vector.filter(|s| s.starts_with('a')).unwrap().collect();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "apple");
        assert_eq!(result[1], "apricot");

        // Filter all 'banana' elements
        let result: Vec<String> = vector.filter(|s| s == "banana").unwrap().collect();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|s| s == "banana"));

        // Filter elements that don't exist
        let result: Vec<String> = vector.filter(|s| s.starts_with('z')).unwrap().collect();
        assert_eq!(result.len(), 0);

        // Filter all elements
        let result: Vec<String> = vector.filter(|_| true).unwrap().collect();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn test_vector_find_with_numbers() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let _ = vector.push(1u32).unwrap();
        let _ = vector.push(5u32).unwrap();
        let _ = vector.push(10u32).unwrap();
        let _ = vector.push(15u32).unwrap();

        // Find first number > 7
        let result: Vec<u32> = vector.find(|&n| n > 7).unwrap().collect();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], 10);
    }

    #[test]
    fn test_vector_filter_with_numbers() {
        let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

        let _ = vector.push(1u32).unwrap();
        let _ = vector.push(5u32).unwrap();
        let _ = vector.push(10u32).unwrap();
        let _ = vector.push(15u32).unwrap();
        let _ = vector.push(20u32).unwrap();

        // Filter all numbers > 7
        let result: Vec<u32> = vector.filter(|&n| n > 7).unwrap().collect();
        assert_eq!(result.len(), 3);
        assert_eq!(result, vec![10, 15, 20]);

        // Filter even numbers
        let result: Vec<u32> = vector.filter(|&n| n % 2 == 0).unwrap().collect();
        assert_eq!(result.len(), 2);
        assert_eq!(result, vec![10, 20]);
    }

    #[test]
    fn test_vector_get_with_max_index() {
        let vector = Root::new(|| Vector::<String>::new());

        // Accessing with usize::MAX should return error due to overflow protection
        let result = vector.get(usize::MAX);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("overflow"),
            "Error message should contain 'overflow'"
        );
    }

    #[test]
    fn test_vector_update_with_max_index() {
        let mut vector = Root::new(|| Vector::<String>::new());

        // Updating with usize::MAX should return error due to overflow protection
        let result = vector.update(usize::MAX, "test".to_owned());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("overflow"),
            "Error message should contain 'overflow'"
        );
    }

    #[test]
    fn test_validate_index_bounds() {
        // Test that validate_index_bounds catches usize::MAX (overflow case)
        let result = super::validate_index_bounds(usize::MAX);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("overflow"),
            "Error message should contain 'overflow'"
        );

        // Test normal index validation passes
        let result = super::validate_index_bounds(5);
        assert!(result.is_ok());

        // Test out-of-bounds index (should pass validation, handled by returning None)
        let result = super::validate_index_bounds(15);
        assert!(result.is_ok());

        // Test large but safe index
        let result = super::validate_index_bounds(usize::MAX - 1);
        assert!(result.is_ok());
    }

    #[test]
    fn reassign_makes_vector_element_ids_deterministic() {
        use crate::collections::{compute_collection_id, compute_id};
        crate::env::reset_for_testing();

        // `new()` => random collection id; each `push` => random element id.
        let mut v = Vector::<String>::new();
        v.push("a".to_owned()).unwrap();
        v.push("b".to_owned()).unwrap();
        v.push("c".to_owned()).unwrap();

        let random_ids: Vec<_> = v.inner.children_cache().unwrap().iter().copied().collect();

        v.reassign_deterministic_id("tags");

        // After reassign every element id is a pure function of
        // (field_name, append index) — no random/timestamp/executor input —
        // so two replicas migrating byte-identical input converge on
        // identical ids (CIP Invariant I9).
        let parent = compute_collection_id(None, "tags");
        let det_ids: Vec<_> = v.inner.children_cache().unwrap().iter().copied().collect();
        assert_eq!(det_ids.len(), 3);
        for (i, id) in det_ids.iter().enumerate() {
            assert_eq!(
                *id,
                compute_id(parent, &(i as u64).to_le_bytes()),
                "element {i} must be re-keyed to its index-derived id"
            );
        }
        assert_ne!(random_ids, det_ids, "re-key must replace the random ids");
        // Values and order survive the re-key.
        assert_eq!(v.iter().unwrap().collect::<Vec<_>>(), vec!["a", "b", "c"]);
    }

    #[test]
    fn reassign_on_empty_vector_relocates_id_without_data_loss() {
        use crate::collections::compute_collection_id;
        use crate::entities::Data;
        crate::env::reset_for_testing();

        // Empty vector: the indexed-children reassign must take the
        // snapshot-empty fast path (relocate the collection id only) and
        // never run the destructive clear+reinsert.
        let mut v = Vector::<String>::new();
        v.reassign_deterministic_id("empties");

        assert_eq!(
            v.inner.id(),
            compute_collection_id(None, "empties"),
            "empty vector should still get its deterministic collection id"
        );
        assert_eq!(v.len().unwrap(), 0);
        // Still usable afterwards.
        v.push("a".to_owned()).unwrap();
        assert_eq!(v.len().unwrap(), 1);
    }

    #[test]
    fn reassign_preserves_vector_element_storage_type() {
        use crate::collections::{compute_collection_id, compute_id, Entry};
        use crate::entities::StorageType;
        use crate::interface::Interface;
        crate::env::reset_for_testing();

        let mut v = Vector::<String>::new();
        // `Frozen` stands in for `AuthoredVector`'s `User { owner }` stamp:
        // a non-Public storage_type that must survive the migrate re-key,
        // or per-entry ownership/authorization would be silently dropped.
        let _ = v
            .push_with_storage_type("x".to_owned(), StorageType::Frozen)
            .unwrap();

        v.reassign_deterministic_id("notes");

        let parent = compute_collection_id(None, "notes");
        let id = compute_id(parent, &0u64.to_le_bytes());
        let entry = <Interface<MainStorage>>::find_by_id::<Entry<String>>(id)
            .unwrap()
            .expect("re-keyed entry must exist");
        assert_eq!(entry.item, "x");
        assert!(matches!(
            entry.storage.metadata.storage_type,
            StorageType::Frozen
        ));
    }
}
