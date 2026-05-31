//! An ordered set supporting range and prefix queries.
//!
//! [`SortedSet`] is to [`UnorderedSet`](super::UnorderedSet) what
//! [`SortedMap`](super::SortedMap) is to [`UnorderedMap`](super::UnorderedMap):
//! the same add-wins union CRDT and on-wire layout (a [`Collection`] of unique
//! `V` elements keyed by `compute_id(parent, v)`), plus a **node-local, derived,
//! non-synced ordered index** that makes range/prefix/pagination and min/max
//! sub-linear instead of a full scan + sort. See [`SortedMap`](super::SortedMap)
//! for the index mechanism and CRDT-safety argument — they are identical here,
//! with the element playing the role of both key and value.
//!
//! # Complexity (on a node, with the index-backing `MainStorage`)
//!
//! | Operation | Cost |
//! |---|---|
//! | [`range`](SortedSet::range) / [`prefix`](SortedSet::prefix) | `O(log n + k)` |
//! | [`page`](SortedSet::page)`(offset, limit)` | `O(offset + limit)` |
//! | [`first`](SortedSet::first) / [`last`](SortedSet::last) | `O(log n)` |
//! | [`insert`](SortedSet::insert) / [`remove`](SortedSet::remove) / [`contains`](SortedSet::contains) | `O(1)` point op **+ an index write + a marker read/write** |
//! | [`iter`](SortedSet::iter) | `O(n)` (returns everything, ascending) |
//!
//! # When to use `SortedSet` vs [`UnorderedSet`](super::UnorderedSet)
//!
//! **Default to [`UnorderedSet`](super::UnorderedSet)** — it has no per-write
//! index overhead. Use `SortedSet` only when you need elements in order:
//! `range(a..b)`, `prefix("user:")`, pagination, sorted iteration, or min/max.
//! It is the `BTreeSet` to `UnorderedSet`'s `HashSet`.

use core::borrow::Borrow;
use core::fmt;
use core::ops::{Bound, RangeBounds};
use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeSeq;
use serde::Serialize;

use super::{compute_id, Collection, CrdtType};
use crate::address::Id;
use crate::collections::error::StoreError;
use crate::entities::Data;
use crate::index::Index;
use crate::store::{Key, MainStorage, StorageAdaptor};

/// A set collection that keeps its elements ordered, enabling range and prefix
/// queries plus pagination. See the [module docs](self) and
/// [`SortedMap`](super::SortedMap) for the storage model.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct SortedSet<V, S: StorageAdaptor = MainStorage> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<V, S>,
}

/// Convert a `RangeBounds` endpoint into the byte-bound the ordered index
/// speaks, using `V`'s order-preserving `AsRef<[u8]>` form.
fn bound_bytes<V: AsRef<[u8]>>(bound: Bound<&V>) -> Bound<Vec<u8>> {
    match bound {
        Bound::Included(v) => Bound::Included(v.as_ref().to_vec()),
        Bound::Excluded(v) => Bound::Excluded(v.as_ref().to_vec()),
        Bound::Unbounded => Bound::Unbounded,
    }
}

/// Re-key the set's inner collection relative to its storage parent, so
/// independently-created nested sets converge. See [`super::rekey`].
impl<V, S> super::rekey::RekeyTarget for SortedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    S: StorageAdaptor,
{
    #[expect(clippy::expect_used, reason = "fatal error if re-key migration fails")]
    fn rekey_relative_to(&mut self, parent_id: Id) {
        let new_id = super::compute_collection_id(Some(parent_id), "__sorted_set");
        if self.inner.id() == new_id {
            return; // already deterministic — idempotent
        }
        let elements: Vec<V> = self
            .iter_unordered()
            .expect("read set elements for re-key")
            .collect();
        self.inner.clear().expect("clear set for re-key");
        self.inner.reassign_deterministic_id_under(
            Some(parent_id),
            "__sorted_set",
            CrdtType::sorted_set(std::any::type_name::<V>()),
        );
        for v in elements {
            let _ = self.insert(v).expect("re-insert set element during re-key");
        }
    }
}

impl<V, S> SortedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new sorted set with a random ID (for nested collections).
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new sorted set with a deterministic ID derived from `field_name`
    /// (for top-level state fields — the `#[app::state]` macro does this).
    ///
    /// # Example
    /// ```ignore
    /// let tags = SortedSet::<String>::new_with_field_name("tags");
    /// ```
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

impl<V, S> SortedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn new_internal() -> Self {
        Self {
            inner: Collection::new(None),
        }
    }

    pub(super) fn new_with_field_name_internal(parent_id: Option<Id>, field_name: &str) -> Self {
        Self {
            inner: Collection::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::sorted_set(std::any::type_name::<V>()),
            ),
        }
    }

    /// Reassigns the set's ID to a deterministic ID based on field name,
    /// migrating elements. Called by the `#[app::state]` macro after `init()`.
    #[expect(clippy::expect_used, reason = "fatal error if migration fails")]
    pub fn reassign_deterministic_id(&mut self, field_name: &str)
    where
        V: AsRef<[u8]> + PartialEq + 'static,
    {
        let new_id = super::compute_collection_id(None, field_name);
        if self.inner.id() == new_id {
            return;
        }
        let elements: Vec<V> = self
            .iter_unordered()
            .expect("failed to read elements for migration")
            .collect();
        self.inner.clear().expect("failed to clear for migration");
        self.inner.reassign_deterministic_id_with_crdt_type(
            field_name,
            CrdtType::sorted_set(std::any::type_name::<V>()),
        );
        for value in elements {
            self.insert(value)
                .expect("failed to re-insert element during migration");
        }
    }

    /// Insert an element. Returns `true` if it was newly inserted, `false` if it
    /// was already present.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn insert(&mut self, value: V) -> Result<bool, StoreError>
    where
        V: AsRef<[u8]> + PartialEq + 'static,
    {
        super::rekey::register_rekey::<Self>();
        let collection = self.inner.id();
        let id = compute_id(collection, value.as_ref());

        if self.inner.get_mut(id)?.is_some() {
            return Ok(false);
        }

        // Warm the ordered index for the new element (after the write, so the
        // collection's full_hash already reflects it when we stamp the marker).
        let order_key = S::index_supported().then(|| value.as_ref().to_vec());

        let _ignored = self.inner.insert(Some(id), value)?;

        if let Some(order_key) = order_key {
            S::index_put(collection, &order_key, id);
            self.stamp_index_marker();
        }

        Ok(true)
    }

    /// The number of elements.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.inner.len()
    }

    /// Returns `true` if the set is empty.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Whether the set contains `value`.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn contains<Q>(&self, value: &Q) -> Result<bool, StoreError>
    where
        V: Borrow<Q>,
        Q: PartialEq + ?Sized + AsRef<[u8]>,
    {
        let id = compute_id(self.inner.id(), value.as_ref());
        self.inner.contains(id)
    }

    /// Remove `value`, returning `true` if it was present.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
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

        if S::index_supported() {
            S::index_remove(self.inner.id(), value.as_ref());
            self.stamp_index_marker();
        }

        Ok(true)
    }

    /// Clear the set.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.inner.clear()?;
        if S::index_supported() {
            S::index_clear(self.inner.id());
            self.stamp_index_marker();
        }
        Ok(())
    }

    /// Iterate elements in storage (hash) order — *not* element order. The
    /// building block the ordered readers sort; kept private.
    fn iter_unordered(&self) -> Result<impl Iterator<Item = V> + '_, StoreError> {
        let collection_id = self.inner.id();
        Ok(self.inner.entries()?.filter_map(move |result| match result {
            Ok(item) => Some(item),
            Err(error) => {
                tracing::error!(
                    target: "calimero_storage::iter_drop",
                    %collection_id,
                    %error,
                    collection_type = "SortedSet",
                    "ITER_DROP: parent's child list advertises an id whose entry could not be loaded — \
                     likely entry-before-parent ordering race or storage inconsistency. \
                     Caller will see a truncated iteration."
                );
                None
            }
        }).fuse())
    }

    // === Ordered secondary index plumbing (mirrors SortedMap) ===

    /// The collection's current `full_hash` (the index validity signal).
    fn current_full_hash(&self) -> [u8; 32] {
        Index::<S>::get_hashes_for(self.inner.id())
            .ok()
            .flatten()
            .map(|(full, _own)| full)
            .unwrap_or([0u8; 32])
    }

    /// Stamp the index validity marker with the current `full_hash`.
    fn stamp_index_marker(&self) {
        let _ = S::storage_write(
            Key::SortedIndexMeta(self.inner.id()),
            &self.current_full_hash(),
        );
    }

    /// `true` if the stamped marker equals the current `full_hash`.
    fn index_marker_current(&self) -> bool {
        S::storage_read(Key::SortedIndexMeta(self.inner.id())).as_deref()
            == Some(&self.current_full_hash()[..])
    }

    /// Reconcile the index with the authoritative element set, then stamp the
    /// marker — writes only the diff (`O(changed)` writes; the entry read is
    /// `O(n)`). Used when a remote sync left the index stale.
    fn rebuild_index(&self) -> Result<(), StoreError>
    where
        V: AsRef<[u8]>,
    {
        let collection = self.inner.id();
        let desired: BTreeSet<Vec<u8>> = self
            .iter_unordered()?
            .map(|v| v.as_ref().to_vec())
            .collect();
        let existing: BTreeSet<Vec<u8>> =
            S::index_range(collection, Bound::Unbounded, Bound::Unbounded, 0, None)
                .into_iter()
                .map(|(order_key, _id)| order_key)
                .collect();
        for order_key in existing.difference(&desired) {
            S::index_remove(collection, order_key);
        }
        for order_key in desired.difference(&existing) {
            S::index_put(collection, order_key, compute_id(collection, order_key));
        }
        self.stamp_index_marker();
        Ok(())
    }

    /// Ensure the index is usable; returns `true` when the adaptor backs it
    /// (rebuilding if stale), `false` → caller uses the in-memory sort fallback.
    fn ensure_index(&self) -> Result<bool, StoreError>
    where
        V: AsRef<[u8]>,
    {
        if !S::index_supported() {
            return Ok(false);
        }
        if !self.index_marker_current() {
            self.rebuild_index()?;
        }
        Ok(true)
    }

    /// Resolve index hits (`order_key, entry_id`) back to elements, in order.
    fn resolve_hits(&self, hits: Vec<(Vec<u8>, Id)>) -> Result<Vec<V>, StoreError> {
        let mut out = Vec::with_capacity(hits.len());
        for (_order_key, id) in hits {
            if let Some(v) = self.inner.get(id)? {
                out.push(v);
            }
        }
        Ok(out)
    }
}

impl<V, S> SortedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + Ord + AsRef<[u8]>,
    S: StorageAdaptor,
{
    /// All elements in their order-defined cache view. The shared sorted helper.
    fn sorted_elements(&self) -> Result<Vec<V>, StoreError> {
        let mut elems: Vec<V> = self.iter_unordered()?.collect();
        elems.sort();
        Ok(elems)
    }

    /// Iterate all elements in ascending order.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn iter(&self) -> Result<impl Iterator<Item = V>, StoreError> {
        if self.ensure_index()? {
            let hits = S::index_range(self.inner.id(), Bound::Unbounded, Bound::Unbounded, 0, None);
            return Ok(self.resolve_hits(hits)?.into_iter());
        }
        Ok(self.sorted_elements()?.into_iter())
    }

    /// Iterate the elements within `range`, ascending.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn range<R>(&self, range: R) -> Result<impl Iterator<Item = V>, StoreError>
    where
        R: RangeBounds<V>,
    {
        if self.ensure_index()? {
            let hits = S::index_range(
                self.inner.id(),
                bound_bytes(range.start_bound()),
                bound_bytes(range.end_bound()),
                0,
                None,
            );
            return Ok(self.resolve_hits(hits)?.into_iter());
        }
        let mut elems: Vec<V> = self
            .iter_unordered()?
            .filter(|v| range.contains(v))
            .collect();
        elems.sort();
        Ok(elems.into_iter())
    }

    /// Iterate the elements whose bytes start with `prefix`, ascending.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn prefix(&self, prefix: &[u8]) -> Result<impl Iterator<Item = V>, StoreError> {
        if self.ensure_index()? {
            let hits = S::index_prefix(self.inner.id(), prefix, 0, None);
            return Ok(self.resolve_hits(hits)?.into_iter());
        }
        let prefix = prefix.to_vec();
        let mut elems: Vec<V> = self
            .iter_unordered()?
            .filter(|v| v.as_ref().starts_with(&prefix))
            .collect();
        elems.sort();
        Ok(elems.into_iter())
    }

    /// A page of `limit` elements starting at `offset`, ascending.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn page(&self, offset: usize, limit: usize) -> Result<Vec<V>, StoreError> {
        if self.ensure_index()? {
            let hits = S::index_range(
                self.inner.id(),
                Bound::Unbounded,
                Bound::Unbounded,
                offset,
                Some(limit),
            );
            return self.resolve_hits(hits);
        }
        Ok(self
            .sorted_elements()?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    /// The smallest element, if any (`O(log n)` seek).
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn first(&self) -> Result<Option<V>, StoreError> {
        if self.ensure_index()? {
            let hits = S::index_range(
                self.inner.id(),
                Bound::Unbounded,
                Bound::Unbounded,
                0,
                Some(1),
            );
            return Ok(self.resolve_hits(hits)?.into_iter().next());
        }
        Ok(self.sorted_elements()?.into_iter().next())
    }

    /// The largest element, if any (`O(log n)` reverse seek).
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn last(&self) -> Result<Option<V>, StoreError> {
        if self.ensure_index()? {
            return match S::index_last(self.inner.id()) {
                Some((_order_key, id)) => self.inner.get(id),
                None => Ok(None),
            };
        }
        Ok(self.sorted_elements()?.into_iter().next_back())
    }
}

impl<V, S> Eq for SortedSet<V, S>
where
    V: Eq + Ord + AsRef<[u8]> + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
}

impl<V, S> PartialEq for SortedSet<V, S>
where
    V: Ord + AsRef<[u8]> + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn eq(&self, other: &Self) -> bool {
        self.iter().unwrap().eq(other.iter().unwrap())
    }
}

impl<V, S> Ord for SortedSet<V, S>
where
    V: Ord + AsRef<[u8]> + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.iter().unwrap().cmp(other.iter().unwrap())
    }
}

impl<V, S> PartialOrd for SortedSet<V, S>
where
    V: Ord + AsRef<[u8]> + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<V, S> fmt::Debug for SortedSet<V, S>
where
    V: Ord + AsRef<[u8]> + fmt::Debug + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, clippy::unwrap_in_result, reason = "'tis fine")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("SortedSet")
                .field("items", &self.inner)
                .finish()
        } else {
            f.debug_set().entries(self.iter().unwrap()).finish()
        }
    }
}

impl<V, S> Default for SortedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
        Self::new_internal()
    }
}

impl<V, S> Serialize for SortedSet<V, S>
where
    V: Ord + AsRef<[u8]> + BorshSerialize + BorshDeserialize + Serialize,
    S: StorageAdaptor,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        let len = self.len().map_err(serde::ser::Error::custom)?;
        let mut seq = serializer.serialize_seq(Some(len))?;
        // Elements are emitted in ascending order.
        for v in self.iter().map_err(serde::ser::Error::custom)? {
            seq.serialize_element(&v)?;
        }
        seq.end()
    }
}

impl<V, S> Extend<V> for SortedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    S: StorageAdaptor,
{
    fn extend<I: IntoIterator<Item = V>>(&mut self, iter: I) {
        for v in iter {
            // Go through `insert` so the ordered index is maintained.
            let _ = self.insert(v);
        }
    }
}

impl<V, S> FromIterator<V> for SortedSet<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    S: StorageAdaptor,
{
    fn from_iter<I: IntoIterator<Item = V>>(iter: I) -> Self {
        let mut set = SortedSet::new_internal();
        set.extend(iter);
        set
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::{Root, SortedSet};
    use crate::store::MainStorage;

    #[test]
    fn test_sorted_set_basic_and_order() {
        let mut set = Root::new(|| SortedSet::<_, MainStorage>::new());

        for v in ["delta", "alpha", "charlie", "bravo"] {
            assert!(set.insert(v.to_owned()).expect("insert failed"));
        }
        // duplicate
        assert!(!set.insert("alpha".to_owned()).expect("insert failed"));

        assert!(set.contains("bravo").expect("contains failed"));
        assert!(!set.contains("zulu").expect("contains failed"));
        assert_eq!(set.len().unwrap(), 4);

        let items: Vec<String> = set.iter().expect("iter failed").collect();
        assert_eq!(items, vec!["alpha", "bravo", "charlie", "delta"]);
    }

    #[test]
    fn test_sorted_set_range_prefix_page_first_last() {
        let mut set = Root::new(|| SortedSet::<_, MainStorage>::new());
        for v in ["user:alice", "user:bob", "post:1", "user:carol", "post:2"] {
            set.insert(v.to_owned()).unwrap();
        }

        let users: Vec<String> = set.prefix(b"user:").unwrap().collect();
        assert_eq!(users, vec!["user:alice", "user:bob", "user:carol"]);

        let range: Vec<String> = set
            .range("post:2".to_owned().."user:bob".to_owned())
            .unwrap()
            .collect();
        assert_eq!(range, vec!["post:2", "user:alice"]);

        let page = set.page(1, 2).unwrap();
        assert_eq!(page, vec!["post:2", "user:alice"]);

        assert_eq!(set.first().unwrap().unwrap(), "post:1");
        assert_eq!(set.last().unwrap().unwrap(), "user:carol");
    }

    #[test]
    fn test_sorted_set_remove_updates_order() {
        let mut set = Root::new(|| SortedSet::<_, MainStorage>::new());
        for v in ["a", "b", "c", "d"] {
            set.insert(v.to_owned()).unwrap();
        }
        assert!(set.remove("b").unwrap());
        assert!(!set.remove("zzz").unwrap());

        let items: Vec<String> = set.iter().unwrap().collect();
        assert_eq!(items, vec!["a", "c", "d"]);
    }

    #[test]
    fn test_sorted_set_clear() {
        let mut set = Root::new(|| SortedSet::<_, MainStorage>::new());
        set.insert("x".to_owned()).unwrap();
        set.insert("y".to_owned()).unwrap();
        set.clear().unwrap();
        assert_eq!(set.len().unwrap(), 0);
        assert!(!set.contains("x").unwrap());
    }
}
