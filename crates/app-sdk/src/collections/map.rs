//! `UnorderedMap<K, V>`: keyed values at `(name, "m", key..)`, their count at `(name, "n")`.
//!
//! The store is ordered by key encoding, so besides point reads the map reads ranges,
//! prefixes, the first and last entries and pages in key order (v1's `SortedMap`).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::{clear_prefix, collection_key, key_after, read_count, read_page};
use crate::encoding::key::{self, FromKey, KeyPart, ToKey};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{check_key, DefaultStore, Store};
use crate::table::{FindOpts, Page};

/// A map from tuple keys to CBOR values, ordered by key.
pub struct UnorderedMap<K: ToKey, V, S: Store = DefaultStore> {
    name: String,
    store: S,
    _types: PhantomData<fn() -> (K, V)>,
}

impl<K: ToKey, V, S: Store> StateField for UnorderedMap<K, V, S> {
    const KIND: &'static str = "unorderedMap";
}

impl<K: ToKey, V: Serialize + DeserializeOwned, S: Store + Default> UnorderedMap<K, V, S> {
    /// A map on the default store; the name is part of every key.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<K: ToKey, V: Serialize + DeserializeOwned, S: Store> UnorderedMap<K, V, S> {
    /// A map on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
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

    /// The value at `k`.
    pub fn get(&self, k: &K) -> Result<Option<V>, StorageError> {
        self.store
            .get(&self.storage_key(k)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Whether `k` has a value.
    pub fn contains(&self, k: &K) -> Result<bool, StorageError> {
        Ok(self.store.get(&self.storage_key(k)?).is_some())
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
        self.store.put(&key, &bytes);
        if old.is_none() {
            self.store.add(&self.count_key()?, 1);
        }
        Ok(old)
    }

    /// Remove `k`, returning the value it held.
    pub fn remove(&mut self, k: &K) -> Result<Option<V>, StorageError> {
        let key = self.storage_key(k)?;
        let Some(bytes) = self.store.get(&key) else {
            return Ok(None);
        };
        let old = value::from_slice(&bytes)?;
        self.store.delete(&key);
        self.store.add(&self.count_key()?, -1);
        Ok(Some(old))
    }

    /// How many keys hold a value: one read of the stored count, never a scan.
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &self.count_key()?, &self.name)
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        Ok(self.len()? == 0)
    }

    /// Remove every entry, a batch at a time; returns how many there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        let base = self.base()?;
        let count = self.count_key()?;
        let removed = clear_prefix(&mut self.store, &base);
        self.store.delete(&count);
        Ok(removed)
    }
}

impl<K: ToKey + FromKey, V: Serialize + DeserializeOwned, S: Store> UnorderedMap<K, V, S> {
    fn page_under(&self, base: &[u8], opts: &FindOpts) -> Result<Page<(K, V)>, StorageError> {
        let map_base = self.base()?;
        let (entries, next) = read_page(&self.store, base, opts)?;
        let items = entries
            .into_iter()
            .map(|(k, v)| {
                let parts = key_after(&k, &map_base)?;
                let key = K::from_key(&parts)
                    .ok_or_else(|| StorageError::Decode("a key of another type".into()))?;
                Ok((key, value::from_slice(&v)?))
            })
            .collect::<Result<_, StorageError>>()?;
        Ok(Page { items, next })
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

    /// The entry with the smallest key.
    pub fn first(&self) -> Result<Option<(K, V)>, StorageError> {
        Ok(self.page(FindOpts::default().limit(1))?.items.pop())
    }

    /// The entry with the largest key.
    pub fn last(&self) -> Result<Option<(K, V)>, StorageError> {
        Ok(self
            .page(FindOpts::default().limit(1).reverse())?
            .items
            .pop())
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
