//! `UnorderedMap<K, V>`: keyed values at `(name, "m", key..)`, their count at `(name, "n")`.
//!
//! The store is ordered by key encoding, so besides point reads the map reads ranges,
//! prefixes, the first and last entries and pages in key order (v1's `SortedMap`).
//!
//! A value can also be a collection (`UnorderedMap<String, Vector<T>>`): `get` then answers
//! that key's collection, stored under its own keys (see [`MapValue`]).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::{bounds, collection_key, iter_entries, read_count, read_page, Entry, KeyPath};
use crate::encoding::key::{self, FromKey, KeyPart, ToKey};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{check_key, DefaultStore, RangeCursor, Store, MAX_RANGE_BATCH};
use crate::table::{FindOpts, Iter, Page, Progress};

use super::slots::decode_value;
pub use super::slots::MapValue;

/// A map from tuple keys to CBOR values, ordered by key.
pub struct UnorderedMap<K: ToKey, V, S: Store = DefaultStore> {
    name: KeyPath,
    store: S,
    _types: PhantomData<fn() -> (K, V)>,
}

impl<K: ToKey, V, S: Store> StateField for UnorderedMap<K, V, S> {
    const KIND: &'static str = "unorderedMap";
}

impl<K: ToKey, V, S: Store + Default> UnorderedMap<K, V, S> {
    /// A map on the default store; the name is part of every key.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<K: ToKey, V, S: Store> UnorderedMap<K, V, S> {
    /// A map on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: KeyPath::root(name),
            store,
            _types: PhantomData,
        }
    }

    fn storage_key(&self, k: &K) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "m", &k.to_key())
    }

    fn base(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "m", &[])
    }

    fn count_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "n", &[])
    }
}

impl<K: ToKey, V: MapValue<S>, S: Store> UnorderedMap<K, V, S> {
    /// What `k` holds: `Option<V>` for a plain value; for a collection value, `k`'s
    /// collection (a handle that reads nothing until used; an absent key's is empty).
    pub fn get(&self, k: &K) -> Result<V::Get, StorageError> {
        V::get(&self.store, &self.name, &k.to_key())
    }

    /// Whether `k` holds a value, or a collection with anything in it (one cursor read).
    pub fn contains(&self, k: &K) -> Result<bool, StorageError> {
        V::contains(&self.store, &self.name, &k.to_key())
    }

    /// Remove `k`: a plain value is returned; a collection is removed whole, a batch at a
    /// time, and the answer is how many stored entries it had.
    pub fn remove(&mut self, k: &K) -> Result<V::Removed, StorageError> {
        V::remove(&mut self.store, &self.name, &k.to_key())
    }

    /// Whether no key holds anything: one read either way.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        V::is_empty(&self.store, &self.name)
    }

    /// Remove every entry (or every key's collection), a batch at a time; returns how many
    /// stored entries there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        V::clear(&mut self.store, &self.name)
    }
}

impl<K: ToKey + FromKey, V: MapValue<S>, S: Store> UnorderedMap<K, V, S> {
    /// Every key with its value (or its collection), in key order, through one cursor as the
    /// loop reaches it. For collection values it skips over each key's rows: two host calls
    /// per key, however large its collection.
    pub fn iter(&self) -> Iter<'_, (K, V::Item)> {
        V::entries(&self.store, &self.name)
    }

    /// Every key, in order.
    pub fn keys(&self) -> impl Iterator<Item = Result<K, StorageError>> + '_ {
        self.iter().map(|e| e.map(|(k, _)| k))
    }
}

impl<K: ToKey, V: Serialize + DeserializeOwned, S: Store> UnorderedMap<K, V, S> {
    /// The plain value at `k`.
    fn read(&self, k: &K) -> Result<Option<V>, StorageError> {
        self.store
            .get(&self.storage_key(k)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Set the value at `k`, returning the value it replaces.
    pub fn insert(&mut self, k: &K, v: &V) -> Result<Option<V>, StorageError> {
        let key = self.storage_key(k)?;
        let bytes = value::to_vec(v)?;
        let old = self
            .store
            .get(&key)
            .map(|b| value::from_slice(&b))
            .transpose()?;
        self.store.put(&key, &bytes)?;
        if old.is_none() {
            self.store.add(&self.count_key()?, 1)?;
        }
        Ok(old)
    }

    /// The value at `k`, inserting `make()` first when there is none.
    pub fn get_or_insert_with(
        &mut self,
        k: &K,
        make: impl FnOnce() -> V,
    ) -> Result<V, StorageError> {
        if let Some(v) = self.read(k)? {
            return Ok(v);
        }
        // Absent, as just read: write it and count it, without reading it again.
        let v = make();
        self.store.put(&self.storage_key(k)?, &value::to_vec(&v)?)?;
        self.store.add(&self.count_key()?, 1)?;
        Ok(v)
    }

    /// Set every pair, as [`UnorderedMap::insert`] does; returns how many keys were new.
    pub fn extend<'e>(
        &mut self,
        entries: impl IntoIterator<Item = (&'e K, &'e V)>,
    ) -> Result<u64, StorageError>
    where
        K: 'e,
        V: 'e,
    {
        // A read and a write per pair, and one `add` for the new keys together (an
        // `insert` per pair adds one write per new key, against the run's write limit).
        let mut new = 0;
        let mut failed = None;
        for (k, v) in entries {
            let written = self
                .storage_key(k)
                .and_then(|key| Ok((key, value::to_vec(v)?)));
            match written {
                Ok((key, bytes)) => {
                    if self.store.get(&key).is_none() {
                        new += 1;
                    }
                    self.store.put(&key, &bytes)?;
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        // The count covers what was written even when a pair failed to encode.
        if new > 0 {
            self.store.add(&self.count_key()?, new)?;
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(new as u64),
        }
    }

    /// Set `k` to `f` of its value (`None` when absent): `Some` stores the result, `None`
    /// removes the key. Returns the new value.
    pub fn update(
        &mut self,
        k: &K,
        f: impl FnOnce(Option<V>) -> Option<V>,
    ) -> Result<Option<V>, StorageError> {
        // One read; the write and the count follow from what it found.
        let key = self.storage_key(k)?;
        let old: Option<V> = self
            .store
            .get(&key)
            .map(|b| value::from_slice(&b))
            .transpose()?;
        let existed = old.is_some();
        match f(old) {
            Some(v) => {
                self.store.put(&key, &value::to_vec(&v)?)?;
                if !existed {
                    self.store.add(&self.count_key()?, 1)?;
                }
                Ok(Some(v))
            }
            None => {
                if existed {
                    self.store.delete(&key)?;
                    self.store.add(&self.count_key()?, -1)?;
                }
                Ok(None)
            }
        }
    }

    /// Remove at most `max` entries, in key order, and the count by one `add`; `done` says
    /// whether the map is empty. Call it from a timer until `done` to clear a map too large
    /// for one run's 10,000 writes.
    pub fn clear_some(&mut self, max: u64) -> Result<Progress, StorageError> {
        let base = self.base()?;
        let (removed, done) = crate::commons::delete_some(&mut self.store, &base, max)?;
        if removed > 0 {
            self.store.add(&self.count_key()?, -(removed as i64))?;
        }
        Ok(Progress { removed, done })
    }

    /// How many keys hold a value: one read of the stored count, never a scan. (A map of
    /// collections keeps no count: count its `keys()`.)
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &self.count_key()?, self.name.label())
    }
}

impl<K: ToKey + FromKey, V: Serialize + DeserializeOwned, S: Store> UnorderedMap<K, V, S> {
    /// A stored entry of the map whose keys start after `map_base`.
    fn decode(map_base: &[u8], entry: Entry) -> Result<(K, V), StorageError> {
        decode_value(map_base, entry)
    }

    fn page_under(&self, base: &[u8], opts: &FindOpts) -> Result<Page<(K, V)>, StorageError> {
        let map_base = self.base()?;
        let (entries, next) = read_page(&self.store, base, opts)?;
        let items = entries
            .into_iter()
            .map(|e| Self::decode(&map_base, e))
            .collect::<Result<_, StorageError>>()?;
        Ok(Page { items, next })
    }

    /// Every entry under `under` (encoded key bytes after the map's base), through one
    /// cursor.
    fn iter_under(&self, under: &[u8], opts: FindOpts) -> Iter<'_, (K, V)> {
        let map_base = match self.base() {
            Ok(b) => b,
            Err(e) => return Iter::failed(e),
        };
        let base = [map_base.as_slice(), under].concat();
        iter_entries(&self.store, &base, &opts, move |e| {
            Self::decode(&map_base, e)
        })
    }

    /// Entries in key order (largest key first with `reverse`), one page of at most 1,000;
    /// `from` and `to` bound the keys (inclusive, exclusive) as key parts.
    pub fn page(&self, opts: FindOpts) -> Result<Page<(K, V)>, StorageError> {
        self.page_under(&self.base()?, &opts)
    }

    /// Entries with keys in `[start, end)`, in key order, one page at a time.
    pub fn range(&self, start: &K, end: &K, opts: FindOpts) -> Result<Page<(K, V)>, StorageError> {
        self.page(opts.from(start.to_key()).to(end.to_key()))
    }

    /// Entries whose key tuple starts with the whole parts `prefix`, in key order, one page
    /// at a time: with `(account, key)` keys, `prefix(&["alice".to_key_part()], ..)` is
    /// alice's entries. `from` and `to` are further parts after the prefix.
    pub fn prefix(&self, prefix: &[KeyPart], opts: FindOpts) -> Result<Page<(K, V)>, StorageError> {
        let base = [self.base()?, key::encode(prefix)].concat();
        check_key(&base)?;
        self.page_under(&base, &opts)
    }

    /// Entries whose key starts with `prefix` where its last part, a string or bytes, is a
    /// prefix of that part of the key: `starts_with(&["user:".to_key_part()], ..)` finds
    /// `"user:1"` and `"user:22"`. In key order, one page at a time; `from` and `to` do not
    /// apply.
    pub fn starts_with(
        &self,
        prefix: &[KeyPart],
        opts: FindOpts,
    ) -> Result<Page<(K, V)>, StorageError> {
        let base = [self.base()?, key::encode_prefix(prefix)].concat();
        check_key(&base)?;
        let opts = FindOpts {
            from: None,
            to: None,
            ..opts
        };
        self.page_under(&base, &opts)
    }

    /// [`UnorderedMap::page`] over every page from `opts` on (`from`, `to`, `reverse`).
    pub fn iter_with(&self, opts: FindOpts) -> Iter<'_, (K, V)> {
        self.iter_under(&[], opts)
    }

    /// [`UnorderedMap::prefix`] over every page.
    pub fn prefix_iter(&self, prefix: &[KeyPart], opts: FindOpts) -> Iter<'_, (K, V)> {
        self.iter_under(&key::encode(prefix), opts)
    }

    /// [`UnorderedMap::starts_with`] over every page, through one cursor as the loop reaches it:
    /// one range open, then one host call per batch of `opts.limit` (1,000 unset), never
    /// more than walking its pages by `next` (each page opens a range of its own), and
    /// nothing read ahead of the batch. `from` and `to` do not apply.
    pub fn starts_with_iter(&self, prefix: &[KeyPart], opts: FindOpts) -> Iter<'_, (K, V)> {
        let opts = FindOpts {
            from: None,
            to: None,
            ..opts
        };
        self.iter_under(&key::encode_prefix(prefix), opts)
    }

    /// Keep only the entries `keep` accepts; returns how many were removed. It reads every
    /// entry a page at a time and removes the rejected ones after each page.
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) -> Result<u64, StorageError> {
        // One cursor reads every entry once; a rejected one is deleted as it is met (the
        // cursor never meets a key again), and the count takes one `add` at the end.
        let map_base = self.base()?;
        let (start, end) = bounds(&map_base);
        let mut all = self.store.range(&start, &end, false);
        let mut removed: i64 = 0;
        let mut failed = None;
        'read: loop {
            let batch = all.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                break;
            }
            for (stored, bytes) in batch {
                match Self::decode(&map_base, (stored.clone(), bytes)) {
                    Ok((k, v)) if !keep(&k, &v) => {
                        self.store.delete(&stored)?;
                        removed += 1;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        failed = Some(e);
                        break 'read;
                    }
                }
            }
        }
        if removed > 0 {
            self.store.add(&self.count_key()?, -removed)?;
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(removed as u64),
        }
    }

    /// Every value, in key order.
    pub fn values(&self) -> impl Iterator<Item = Result<V, StorageError>> + '_ {
        self.iter_with(FindOpts::default())
            .map(|e| e.map(|(_, v)| v))
    }

    /// The entry with the smallest key.
    pub fn first(&self) -> Result<Option<(K, V)>, StorageError> {
        // One entry through a cursor; a page would read one more to know of a next.
        self.iter_with(FindOpts::default().limit(1))
            .next()
            .transpose()
    }

    /// The entry with the largest key.
    pub fn last(&self) -> Result<Option<(K, V)>, StorageError> {
        self.iter_with(FindOpts::default().limit(1).reverse())
            .next()
            .transpose()
    }
}

impl<K: ToKey, V, S: Store> crate::collections::Nestable<S> for UnorderedMap<K, V, S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self {
            name: path,
            store,
            _types: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::key::ToKeyPart;
    use crate::store::MemStore;

    fn map() -> (UnorderedMap<String, u32, MemStore>, MemStore) {
        let store = MemStore::new();
        (UnorderedMap::with_store("m", store.clone()), store)
    }

    fn keys(page: &Page<(String, u32)>) -> Vec<&str> {
        page.items.iter().map(|(k, _)| k.as_str()).collect()
    }

    #[test]
    fn insert_get_remove_keep_the_count() {
        let (mut m, _) = map();
        assert_eq!(m.insert(&"a".into(), &1).unwrap(), None);
        assert_eq!(m.insert(&"a".into(), &2).unwrap(), Some(1));
        assert_eq!(m.insert(&"b".into(), &3).unwrap(), None);
        assert_eq!(m.len().unwrap(), 2);
        assert_eq!(m.get(&"a".into()).unwrap(), Some(2));
        assert_eq!(m.remove(&"a".into()).unwrap(), Some(2));
        assert_eq!(m.remove(&"a".into()).unwrap(), None);
        assert_eq!(m.len().unwrap(), 1);
        assert!(!m.contains(&"a".into()).unwrap());
        assert!(!m.is_empty().unwrap());
    }

    #[test]
    fn the_count_is_kept_with_add() {
        let (mut m, store) = map();
        let _ = m.insert(&"a".into(), &1).unwrap();
        let _ = m.insert(&"b".into(), &1).unwrap();
        assert_eq!(store.op_counts(&m.count_key().unwrap()), (0, 2));
    }

    #[test]
    fn ranges_prefixes_and_ends_read_in_key_order() {
        let (mut m, _) = map();
        for (i, k) in ["user:2", "b", "user:10", "a", "user:1", "z"]
            .iter()
            .enumerate()
        {
            let _ = m.insert(&(*k).into(), &(i as u32)).unwrap();
        }
        let all = m.page(FindOpts::default()).unwrap();
        assert_eq!(keys(&all), ["a", "b", "user:1", "user:10", "user:2", "z"]);
        let r = m
            .range(&"b".into(), &"user:2".into(), FindOpts::default())
            .unwrap();
        assert_eq!(keys(&r), ["b", "user:1", "user:10"]);
        let p = m
            .starts_with(&["user:".to_key_part()], FindOpts::default())
            .unwrap();
        assert_eq!(keys(&p), ["user:1", "user:10", "user:2"]);
        let p = m
            .starts_with(&["user:1".to_key_part()], FindOpts::default().reverse())
            .unwrap();
        assert_eq!(keys(&p), ["user:10", "user:1"]);
        let exact = m
            .prefix(&["user:1".to_key_part()], FindOpts::default())
            .unwrap();
        assert_eq!(keys(&exact), ["user:1"]);
        assert_eq!(m.first().unwrap().unwrap().0, "a");
        assert_eq!(m.last().unwrap().unwrap().0, "z");
    }

    #[test]
    fn pages_follow_the_cursor() {
        let (mut m, _) = map();
        for i in 0..25u32 {
            let _ = m.insert(&format!("k{i:02}"), &i).unwrap();
        }
        let mut seen = Vec::new();
        let mut opts = FindOpts::default().limit(10);
        loop {
            let page = m.page(opts.clone()).unwrap();
            assert!(page.items.len() <= 10);
            seen.extend(page.items.into_iter().map(|(_, v)| v));
            match page.next {
                Some(next) => opts = opts.cursor(next),
                None => break,
            }
        }
        assert_eq!(seen, (0..25).collect::<Vec<_>>());
    }

    #[test]
    fn a_page_is_at_most_a_thousand_entries() {
        let (mut m, store) = map();
        for i in 0..1_200u32 {
            let _ = m.insert(&format!("{i:05}"), &i).unwrap();
        }
        store.reset_pulled();
        let page = m.page(FindOpts::default().limit(5_000)).unwrap();
        assert_eq!(page.items.len(), 1_000);
        assert!(page.next.is_some());
        assert_eq!(store.pulled(), 1_001, "limit + 1 entries and no more");
    }

    #[test]
    fn a_cursor_from_another_prefix_is_refused() {
        let (mut m, _) = map();
        for k in ["a1", "a2", "b1", "b2"] {
            let _ = m.insert(&k.into(), &0).unwrap();
        }
        let a = m
            .starts_with(&["a".to_key_part()], FindOpts::default().limit(1))
            .unwrap();
        let err = m
            .starts_with(
                &["b".to_key_part()],
                FindOpts::default().cursor(a.next.unwrap()),
            )
            .unwrap_err();
        assert_eq!(err, StorageError::BadCursor);
    }

    #[test]
    fn clear_removes_every_entry_in_batches() {
        let (mut m, store) = map();
        let mut other: UnorderedMap<String, u32, MemStore> =
            UnorderedMap::with_store("m2", store.clone());
        let _ = other.insert(&"keep".into(), &1).unwrap();
        for i in 0..2_500u32 {
            let _ = m.insert(&format!("{i:05}"), &i).unwrap();
        }
        assert_eq!(m.clear().unwrap(), 2_500);
        assert_eq!(m.len().unwrap(), 0);
        assert!(m.first().unwrap().is_none());
        assert_eq!(other.get(&"keep".into()).unwrap(), Some(1));
        assert_eq!(store.len(), 2, "the other map's entry and count stay");
    }

    #[test]
    fn tuple_keys_cover_nested_maps() {
        let store = MemStore::new();
        let mut m: UnorderedMap<(String, u64), String, MemStore> =
            UnorderedMap::with_store("nested", store);
        let _ = m.insert(&("alice".into(), 2), &"b".into()).unwrap();
        let _ = m.insert(&("alice".into(), 1), &"a".into()).unwrap();
        let _ = m.insert(&("alicea".into(), 1), &"x".into()).unwrap();
        let _ = m.insert(&("bob".into(), 1), &"c".into()).unwrap();
        let alice = m
            .prefix(&["alice".to_key_part()], FindOpts::default())
            .unwrap();
        assert_eq!(
            alice.items,
            [
                (("alice".into(), 1), "a".to_owned()),
                (("alice".into(), 2), "b".to_owned())
            ]
        );
    }
}
