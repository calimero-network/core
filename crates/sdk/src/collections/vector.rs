//! `Vector<T>`: elements at `(name, "e", position)` for positions `0..len`, the length at
//! `(name, "n")` (v1's `Vector`).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::{collection_key, iter_entries, read_count, read_page, KeyPath};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, RangeCursor, Store, MAX_RANGE_BATCH};
use crate::table::{FindOpts, Iter, Page, Progress};

use super::slots::VecElement;

/// A list of values by position; it grows and shrinks at the end.
pub struct Vector<T, S: Store = DefaultStore> {
    name: KeyPath,
    store: S,
    _type: PhantomData<fn() -> T>,
}

impl<T, S: Store> StateField for Vector<T, S> {
    const KIND: &'static str = "vector";
}

impl<T, S: Store + Default> Vector<T, S> {
    /// A vector on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T, S: Store> Vector<T, S> {
    /// A vector on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: KeyPath::root(name),
            store,
            _type: PhantomData,
        }
    }

    fn storage_key(&self, index: u64) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "e", &[KeyPart::Int(index.into())])
    }

    fn base(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "e", &[])
    }

    fn len_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "n", &[])
    }

    /// How many elements there are: one read of the stored length.
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &self.len_key()?, self.name.label())
    }

    /// Whether the vector is empty.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        Ok(self.len()? == 0)
    }
}

impl<T: VecElement<S>, S: Store> Vector<T, S> {
    /// The element at `index` (`Option<T>`), or for a vector of collections the collection
    /// at `index` (`Option<C>`, a handle that reads nothing until used); `None` past the end.
    pub fn get(&self, index: u64) -> Result<T::Get, StorageError> {
        T::get(&self.store, &self.name, index)
    }

    /// Remove the last element and return it; for a vector of collections, remove the last
    /// collection whole and answer how many stored entries it had.
    pub fn pop(&mut self) -> Result<Option<T::Popped>, StorageError> {
        T::pop(&mut self.store, &self.name)
    }

    /// Every element (or collection) in position order, through one cursor as the loop
    /// reaches it; a vector of collections reads only its length.
    pub fn iter(&self) -> Iter<'_, T::Item> {
        T::entries(&self.store, &self.name)
    }

    /// Remove every element (or collection), a batch at a time; returns how many stored
    /// entries there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        T::clear(&mut self.store, &self.name)
    }
}

impl<C: crate::collections::Nestable<S>, S: Store + Clone> Vector<C, S> {
    /// For a vector of collections: append an empty collection and return its position and
    /// the collection. One write (the length); the collection writes as it is filled.
    pub fn push_new(&mut self) -> Result<(u64, C), StorageError> {
        let index = self.len()?;
        let path = self.name.child(&[KeyPart::Int(index.into())]);
        let _fits = crate::commons::encode(path.parts())?;
        self.store.add(&self.len_key()?, 1);
        Ok((index, C::at(path, self.store.clone())))
    }
}

impl<T: Serialize + DeserializeOwned, S: Store> Vector<T, S> {
    /// The plain element at `index`; `None` past the end.
    fn read(&self, index: u64) -> Result<Option<T>, StorageError> {
        self.store
            .get(&self.storage_key(index)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Append `v`; returns its position.
    pub fn push(&mut self, v: &T) -> Result<u64, StorageError> {
        let index = self.len()?;
        let key = self.storage_key(index)?;
        self.store.put(&key, &value::to_vec(v)?);
        self.store.add(&self.len_key()?, 1);
        Ok(index)
    }

    /// Replace the element at `index`, returning the old one; `OutOfBounds` past the end.
    /// One read and one write: positions are dense, so a missing element is past the end
    /// (the length is read only to name it in the error).
    pub fn set(&mut self, index: u64, v: &T) -> Result<T, StorageError> {
        let key = self.storage_key(index)?;
        let old = self.read_at(&key, index)?;
        self.store.put(&key, &value::to_vec(v)?);
        Ok(old)
    }

    /// The element stored at `key`, position `index`; `OutOfBounds` when there is none.
    fn read_at(&self, key: &[u8], index: u64) -> Result<T, StorageError> {
        match self.store.get(key) {
            Some(bytes) => value::from_slice(&bytes),
            None => Err(StorageError::OutOfBounds {
                index,
                len: self.len()?,
            }),
        }
    }

    /// One cursor over the stored elements at positions `[from, to)`.
    fn positions(&self, from: u64, to: u64, reverse: bool) -> Result<S::Range, StorageError> {
        Ok(self
            .store
            .range(&self.storage_key(from)?, &self.storage_key(to)?, reverse))
    }

    /// Elements in position order (last first with `reverse`), one page of at most 1,000;
    /// `from` and `to` take positions as `KeyPart::Int`.
    pub fn page(&self, opts: FindOpts) -> Result<Page<T>, StorageError> {
        let (entries, next) = read_page(&self.store, &self.base()?, &opts)?;
        let items = entries
            .iter()
            .map(|(_, bytes)| value::from_slice(bytes))
            .collect::<Result<_, _>>()?;
        Ok(Page { items, next })
    }

    /// [`Vector::page`] over every page from `opts` on: `.reverse()` walks from the end,
    /// `.from(vec![KeyPart::Int(i)])` starts at position `i`.
    pub fn iter_with(&self, opts: FindOpts) -> Iter<'_, T> {
        match self.base() {
            Ok(base) => iter_entries(&self.store, &base, &opts, |(_, v)| value::from_slice(&v)),
            Err(e) => Iter::failed(e),
        }
    }

    /// The first element.
    pub fn first(&self) -> Result<Option<T>, StorageError> {
        self.read(0)
    }

    /// The last element.
    pub fn last(&self) -> Result<Option<T>, StorageError> {
        match self.len()?.checked_sub(1) {
            Some(last) => self.read(last),
            None => Ok(None),
        }
    }

    /// Append every value in order; returns how many were appended. One read of the length
    /// and one `add` for the whole batch, plus a write per value (`push` in a loop adds
    /// one more write per value, against the run's write limit).
    pub fn extend<'v>(
        &mut self,
        values: impl IntoIterator<Item = &'v T>,
    ) -> Result<u64, StorageError>
    where
        T: 'v,
    {
        let start = self.len()?;
        let mut n = 0;
        let mut failed = None;
        for v in values {
            let written = self
                .storage_key(start + n)
                .and_then(|key| Ok((key, value::to_vec(v)?)));
            match written {
                Ok((key, bytes)) => {
                    self.store.put(&key, &bytes);
                    n += 1;
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        // The length covers what was written even when a value failed to encode.
        if n > 0 {
            self.store.add(&self.len_key()?, n as i64);
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(n),
        }
    }

    /// Remove the element at `index` and return it, moving the last element into its place:
    /// at most three reads and three writes whatever the length; `OutOfBounds` past the end.
    pub fn swap_remove(&mut self, index: u64) -> Result<T, StorageError> {
        let len = self.len()?;
        if index >= len {
            return Err(StorageError::OutOfBounds { index, len });
        }
        let key = self.storage_key(index)?;
        let removed = self.read_at(&key, index)?;
        let last = len - 1;
        if index != last {
            let last_key = self.storage_key(last)?;
            let bytes = self.store.get(&last_key).ok_or(StorageError::Dangling)?;
            self.store.put(&key, &bytes);
            self.store.delete(&last_key);
        } else {
            self.store.delete(&key);
        }
        self.store.add(&self.len_key()?, -1);
        Ok(removed)
    }

    /// Remove at most `max` elements from the end (a delete each and one `add`); `done` says
    /// whether the vector is empty. Call it from a timer until `done` to clear a vector too
    /// large for one run's 10,000 writes.
    pub fn clear_some(&mut self, max: u64) -> Result<Progress, StorageError> {
        let len = self.len()?;
        let removed = self.truncate(len.saturating_sub(max))?;
        Ok(Progress {
            removed,
            done: removed == len,
        })
    }

    /// Keep the first `len` elements and remove the rest, last first; returns how many were
    /// removed. A delete per removed element and one `add`; nothing happens when the vector
    /// is not longer than `len`.
    pub fn truncate(&mut self, len: u64) -> Result<u64, StorageError> {
        let have = self.len()?;
        for index in (len..have).rev() {
            self.store.delete(&self.storage_key(index)?);
        }
        let removed = have.saturating_sub(len);
        if removed > 0 {
            self.store.add(&self.len_key()?, -(removed as i64));
        }
        Ok(removed)
    }

    /// Set the element at `index` to `f` of it, returning the new value: one read and one
    /// write; `OutOfBounds` past the end.
    pub fn update(&mut self, index: u64, f: impl FnOnce(T) -> T) -> Result<T, StorageError> {
        let key = self.storage_key(index)?;
        let new = f(self.read_at(&key, index)?);
        self.store.put(&key, &value::to_vec(&new)?);
        Ok(new)
    }

    /// Swap the elements at `a` and `b`: two reads and two writes; `OutOfBounds` when either
    /// is past the end.
    pub fn swap(&mut self, a: u64, b: u64) -> Result<(), StorageError> {
        let key_a = self.storage_key(a)?;
        let key_b = self.storage_key(b)?;
        let (Some(at_a), Some(at_b)) = (self.store.get(&key_a), self.store.get(&key_b)) else {
            let len = self.len()?;
            let index = if a >= len { a } else { b };
            return Err(StorageError::OutOfBounds { index, len });
        };
        if a != b {
            self.store.put(&key_a, &at_b);
            self.store.put(&key_b, &at_a);
        }
        Ok(())
    }

    /// Insert `v` at `index`, moving every later element one place up: the tail is read
    /// through one cursor (a host call per 1,000) and each element written once, so it
    /// costs a write per element after `index`. `index == len` pushes; `OutOfBounds` past
    /// that.
    pub fn insert(&mut self, index: u64, v: &T) -> Result<(), StorageError> {
        let len = self.len()?;
        if index > len {
            return Err(StorageError::OutOfBounds { index, len });
        }
        let bytes = value::to_vec(v)?;
        // Last first: each write lands above every position not yet read.
        let mut tail = self.positions(index, len, true)?;
        let mut position = len;
        loop {
            let batch = tail.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                break;
            }
            for (_, moved) in batch {
                self.store.put(&self.storage_key(position)?, &moved);
                position -= 1;
            }
        }
        if position != index {
            return Err(StorageError::Dangling);
        }
        self.store.put(&self.storage_key(index)?, &bytes);
        self.store.add(&self.len_key()?, 1);
        Ok(())
    }

    /// Remove and return the element at `index`, moving every later element one place
    /// down: the tail is read through one cursor and each element written once, so it costs
    /// a write per element after `index` ([`Vector::swap_remove`] costs three);
    /// `OutOfBounds` past the end.
    pub fn remove(&mut self, index: u64) -> Result<T, StorageError> {
        let len = self.len()?;
        if index >= len {
            return Err(StorageError::OutOfBounds { index, len });
        }
        // First first: each write lands below every position not yet read.
        let mut tail = self.positions(index, len, false)?;
        let mut removed = None;
        let mut position = index;
        loop {
            let batch = tail.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                break;
            }
            for (_, bytes) in batch {
                if position == index {
                    removed = Some(value::from_slice(&bytes)?);
                } else {
                    self.store.put(&self.storage_key(position - 1)?, &bytes);
                }
                position += 1;
            }
        }
        let removed = removed.ok_or(StorageError::Dangling)?;
        self.store.delete(&self.storage_key(len - 1)?);
        self.store.add(&self.len_key()?, -1);
        Ok(removed)
    }

    /// Keep only the elements `keep` accepts, in order, and return how many were removed.
    /// Every element is read once through one cursor; a kept element is written only when
    /// it moves down, a removed one costs a delete, and the length one `add`.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) -> Result<u64, StorageError> {
        let len = self.len()?;
        let mut all = self.positions(0, len, false)?;
        let mut read = 0;
        let mut kept = 0;
        loop {
            let batch = all.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                break;
            }
            for (_, bytes) in batch {
                if keep(&value::from_slice(&bytes)?) {
                    if kept != read {
                        self.store.put(&self.storage_key(kept)?, &bytes);
                    }
                    kept += 1;
                }
                read += 1;
            }
        }
        for index in kept..len {
            self.store.delete(&self.storage_key(index)?);
        }
        let removed = len - kept;
        if removed > 0 {
            self.store.add(&self.len_key()?, -(removed as i64));
        }
        Ok(removed)
    }

    /// Whether any element equals `v`: a scan through one cursor that stops at the first.
    pub fn contains(&self, v: &T) -> Result<bool, StorageError>
    where
        T: PartialEq,
    {
        for item in self.iter_with(FindOpts::default()) {
            if item? == *v {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl<T, S: Store> crate::collections::Nestable<S> for Vector<T, S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self {
            name: path,
            store,
            _type: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    fn vector() -> (Vector<String, MemStore>, MemStore) {
        let store = MemStore::new();
        (Vector::with_store("v", store.clone()), store)
    }

    #[test]
    fn push_get_set_pop() {
        let (mut v, _) = vector();
        assert_eq!(v.pop().unwrap(), None);
        assert_eq!(v.push(&"a".into()).unwrap(), 0);
        assert_eq!(v.push(&"b".into()).unwrap(), 1);
        assert_eq!(v.len().unwrap(), 2);
        assert_eq!(v.get(1).unwrap().as_deref(), Some("b"));
        assert_eq!(v.get(2).unwrap(), None);
        assert_eq!(v.set(0, &"A".into()).unwrap(), "a");
        assert_eq!(
            v.set(2, &"c".into()).unwrap_err(),
            StorageError::OutOfBounds { index: 2, len: 2 }
        );
        assert_eq!(v.pop().unwrap().as_deref(), Some("b"));
        assert_eq!(v.len().unwrap(), 1);
        assert_eq!(v.push(&"c".into()).unwrap(), 1);
        assert_eq!(v.page(FindOpts::default()).unwrap().items, ["A", "c"]);
    }

    #[test]
    fn positions_sort_numerically_past_nine() {
        let (mut v, _) = vector();
        for i in 0..12 {
            let _ = v.push(&format!("{i}")).unwrap();
        }
        let items = v.page(FindOpts::default()).unwrap().items;
        assert_eq!(items[10], "10");
        let tail = v
            .page(FindOpts::default().from(vec![KeyPart::Int(9)]))
            .unwrap()
            .items;
        assert_eq!(tail, ["9", "10", "11"]);
        let back = v.page(FindOpts::default().reverse().limit(2)).unwrap();
        assert_eq!(back.items, ["11", "10"]);
    }

    #[test]
    fn the_length_is_kept_with_add_and_clear_resets_it() {
        let (mut v, store) = vector();
        for i in 0..1_100 {
            let _ = v.push(&format!("{i}")).unwrap();
        }
        assert_eq!(store.op_counts(&v.len_key().unwrap()), (0, 1_100));
        assert_eq!(v.clear().unwrap(), 1_100);
        assert_eq!(v.len().unwrap(), 0);
        assert!(store.is_empty());
        assert_eq!(v.push(&"again".into()).unwrap(), 0);
    }
}
