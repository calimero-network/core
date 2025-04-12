//! High-level data structures for storage.

use core::cell::RefCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::{fmt, ptr};
use std::collections::BTreeMap;
use std::sync::LazyLock;

use borsh::{BorshDeserialize, BorshSerialize};
use indexmap::IndexSet;
use sha2::{Digest, Sha256};

pub mod unordered_map;
pub use unordered_map::UnorderedMap;
pub mod unordered_set;
pub use unordered_set::UnorderedSet;
pub mod vector;
pub use vector::Vector;
mod root;
#[doc(hidden)]
pub use root::Root;
pub mod error;
pub use error::StoreError;

// fixme! macro expects `calimero_storage` to be in deps
use crate as calimero_storage;
use crate::address::{Id, Path};
use crate::entities::{ChildInfo, Data, Element};
use crate::interface::{Interface, StorageError};
use crate::store::{MainStorage, StorageAdaptor};
use crate::{AtomicUnit, Collection};

/// Compute the ID for a key.
fn compute_id(parent: Id, key: &[u8]) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(parent.as_bytes());
    hasher.update(key);
    Id::new(hasher.finalize().into())
}

mod compat {
    use std::collections::BTreeMap;

    use borsh::{BorshDeserialize, BorshSerialize};

    use crate::entities::{ChildInfo, Collection, Data, Element};

    /// Thing.
    #[derive(Copy, Clone, Debug)]
    pub(super) struct RootHandle;

    #[derive(BorshSerialize, BorshDeserialize)]
    pub(super) struct RootChildDud;

    impl Data for RootChildDud {
        fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
            unimplemented!()
        }

        fn element(&self) -> &Element {
            unimplemented!()
        }

        fn element_mut(&mut self) -> &mut Element {
            unimplemented!()
        }
    }

    impl Collection for RootHandle {
        type Child = RootChildDud;

        fn name(&self) -> &str {
            "no collection, remove this nonsense"
        }
    }
}

use compat::RootHandle;

#[derive(BorshSerialize, BorshDeserialize)]
struct Collection<T, S: StorageAdaptor = MainStorage> {
    storage: Element,

    #[borsh(skip)]
    children_ids: RefCell<Option<IndexSet<Id>>>,

    #[borsh(skip)]
    _priv: PhantomData<(T, S)>,
}

impl<T, S: StorageAdaptor> Data for Collection<T, S> {
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

/// A collection of entries in a map.
#[derive(Collection, Copy, Clone, Debug, Eq, PartialEq)]
#[children(Entry<T>)]
struct Entries<T> {
    /// Helper to associate the generic types with the collection.
    _priv: PhantomData<T>,
}

/// An entry in a map.
#[derive(AtomicUnit, BorshSerialize, BorshDeserialize, Clone, Debug)]
struct Entry<T> {
    /// The item in the entry.
    item: T,
    /// The storage element for the entry.
    #[storage]
    storage: Element,
}

#[expect(unused_qualifications, reason = "AtomicUnit macro is unsanitized")]
type StoreResult<T> = std::result::Result<T, StoreError>;

static ROOT_ID: LazyLock<Id> = LazyLock::new(|| Id::root());

impl<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> Collection<T, S> {
    /// Creates a new collection.
    #[expect(clippy::expect_used, reason = "fatal error if it happens")]
    fn new(id: Option<Id>) -> Self {
        let id = id.unwrap_or_else(|| Id::random());

        let mut this = Self {
            children_ids: RefCell::new(None),
            storage: Element::new(&Path::new("::unused").expect("valid path"), Some(id)),
            _priv: PhantomData,
        };

        if id.is_root() {
            let _ignored = <Interface<S>>::save(&mut this).expect("save");
        } else {
            let _ =
                <Interface<S>>::add_child_to(*ROOT_ID, &RootHandle, &mut this).expect("add child");
        }

        this
    }

    /// Inserts an item into the collection.
    fn insert(&mut self, id: Option<Id>, item: T) -> StoreResult<T> {
        let path = self.path();

        let mut collection = CollectionMut::new(self);

        let mut entry = Entry {
            item,
            storage: Element::new(&path, id),
        };

        collection.insert(&mut entry)?;

        Ok(entry.item)
    }

    #[inline(never)]
    fn get(&self, id: Id) -> StoreResult<Option<T>> {
        let entry = <Interface<S>>::find_by_id::<Entry<_>>(id)?;

        Ok(entry.map(|entry| entry.item))
    }

    fn contains(&self, id: Id) -> StoreResult<bool> {
        Ok(self.children_cache()?.contains(&id))
    }

    fn get_mut(&mut self, id: Id) -> StoreResult<Option<EntryMut<'_, T, S>>> {
        let entry = <Interface<S>>::find_by_id::<Entry<_>>(id)?;

        Ok(entry.map(|entry| EntryMut {
            collection: CollectionMut::new(self),
            entry,
        }))
    }

    fn len(&self) -> StoreResult<usize> {
        Ok(self.children_cache()?.len())
    }

    fn entries(
        &self,
    ) -> StoreResult<impl ExactSizeIterator<Item = StoreResult<T>> + DoubleEndedIterator + '_> {
        let iter = self.children_cache()?.iter().copied().map(|child| {
            let entry = <Interface<S>>::find_by_id::<Entry<_>>(child)?
                .ok_or(StoreError::StorageError(StorageError::NotFound(child)))?;

            Ok(entry.item)
        });

        Ok(iter)
    }

    fn nth(&self, index: usize) -> StoreResult<Option<Id>> {
        Ok(self.children_cache()?.get_index(index).copied())
    }

    fn last(&self) -> StoreResult<Option<Id>> {
        Ok(self.children_cache()?.last().copied())
    }

    fn clear(&mut self) -> StoreResult<()> {
        let mut collection = CollectionMut::new(self);

        collection.clear()?;

        Ok(())
    }

    #[expect(
        clippy::unwrap_in_result,
        clippy::expect_used,
        reason = "fatal error if it happens"
    )]
    fn children_cache(&self) -> StoreResult<&mut IndexSet<Id>> {
        let mut cache = self.children_ids.borrow_mut();

        if cache.is_none() {
            let children = <Interface<S>>::child_info_for(self.id(), &RootHandle)?;

            let children = children.into_iter().map(|c| c.id()).collect();

            *cache = Some(children);
        }

        let children = cache.as_mut().expect("children");

        #[expect(unsafe_code, reason = "necessary for caching")]
        let children = unsafe { &mut *ptr::from_mut(children) };

        Ok(children)
    }
}

#[derive(Debug)]
struct EntryMut<'a, T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> {
    collection: CollectionMut<'a, T, S>,
    entry: Entry<T>,
}

impl<T, S> EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn remove(self) -> StoreResult<T> {
        let old = self
            .collection
            .get(self.entry.id())?
            .ok_or(StoreError::StorageError(StorageError::NotFound(
                self.entry.id(),
            )))?;

        let _ =
            <Interface<S>>::remove_child_from(self.collection.id(), &RootHandle, self.entry.id())?;

        let _ = self
            .collection
            .children_cache()?
            .shift_remove(&self.entry.id());

        Ok(old)
    }
}

impl<T, S> Deref for EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.entry.item
    }
}

impl<T, S: StorageAdaptor> DerefMut for EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry.item
    }
}

impl<T, S> Drop for EntryMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn drop(&mut self) {
        self.entry.element_mut().update();
        let _ignored = <Interface<S>>::save(&mut self.entry);
    }
}

#[derive(Debug)]
struct CollectionMut<'a, T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> {
    collection: &'a mut Collection<T, S>,
}

impl<'a, T, S> CollectionMut<'a, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn new(collection: &'a mut Collection<T, S>) -> Self {
        Self { collection }
    }

    fn insert(&mut self, item: &mut Entry<T>) -> StoreResult<()> {
        let _ = <Interface<S>>::add_child_to(self.collection.id(), &RootHandle, item)?;

        let _ignored = self.collection.children_cache()?.insert(item.id());

        Ok(())
    }

    fn clear(&mut self) -> StoreResult<()> {
        let children = self.collection.children_cache()?;

        for child in children.drain(..) {
            let _ = <Interface<S>>::remove_child_from(self.collection.id(), &RootHandle, child)?;
        }

        Ok(())
    }
}

impl<T, S> Deref for CollectionMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Target = Collection<T, S>;

    fn deref(&self) -> &Self::Target {
        self.collection
    }
}

impl<T, S> DerefMut for CollectionMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.collection
    }
}

impl<T, S> Drop for CollectionMut<'_, T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn drop(&mut self) {
        self.collection.element_mut().update();
    }
}

impl<T, S: StorageAdaptor> fmt::Debug for Collection<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Collection")
            .field("element", &self.storage)
            .finish()
    }
}

impl<T, S> Default for Collection<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
        Self::new(None)
    }
}

impl<T: Eq + BorshSerialize + BorshDeserialize, S: StorageAdaptor> Eq for Collection<T, S> {}

impl<T: PartialEq + BorshSerialize + BorshDeserialize, S: StorageAdaptor> PartialEq
    for Collection<T, S>
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn eq(&self, other: &Self) -> bool {
        let l = self.entries().unwrap().flatten();
        let r = other.entries().unwrap().flatten();

        l.eq(r)
    }
}

impl<T: Ord + BorshSerialize + BorshDeserialize, S: StorageAdaptor> Ord for Collection<T, S> {
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let l = self.entries().unwrap().flatten();
        let r = other.entries().unwrap().flatten();

        l.cmp(r)
    }
}

impl<T: PartialOrd + BorshSerialize + BorshDeserialize, S: StorageAdaptor> PartialOrd
    for Collection<T, S>
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let l = self.entries().ok()?.flatten();
        let r = other.entries().ok()?.flatten();

        l.partial_cmp(r)
    }
}

impl<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> Extend<(Option<Id>, T)>
    for Collection<T, S>
{
    fn extend<I: IntoIterator<Item = (Option<Id>, T)>>(&mut self, iter: I) {
        let path = self.path();

        let mut collection = CollectionMut::new(self);

        for (id, item) in iter {
            let mut entry = Entry {
                item,
                storage: Element::new(&path, id),
            };

            collection
                .insert(&mut entry)
                .expect("collection extension failed");
        }
    }
}

impl<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor> FromIterator<(Option<Id>, T)>
    for Collection<T, S>
{
    fn from_iter<I: IntoIterator<Item = (Option<Id>, T)>>(iter: I) -> Self {
        let mut collection = Collection::new(None);
        collection.extend(iter);
        collection
    }
}
