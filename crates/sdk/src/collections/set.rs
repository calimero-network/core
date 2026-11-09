//! `Set<T>`: elements at `(name, "s", element..)`, each holding CBOR `null`, and their count at
//! `(name, "n")` (v1's `UnorderedSet`, and `SortedSet` through its ordered reads).

use core::marker::PhantomData;

use crate::app::StateField;
use crate::commons::{clear_prefix, collection_key, key_after, read_count, read_page};
use crate::encoding::key::{self, FromKey, KeyPart, ToKey};
use crate::error::StorageError;
use crate::store::{check_key, DefaultStore, Store};
use crate::table::{FindOpts, Page};

/// CBOR `null`: what every element's key holds.
const PRESENT: &[u8] = &[0xF6];

/// A set of key-encodable elements, ordered by their key encoding.
pub struct Set<T: ToKey, S: Store = DefaultStore> {
    name: String,
    store: S,
    _type: PhantomData<fn() -> T>,
}

impl<T: ToKey, S: Store> StateField for Set<T, S> {
    const KIND: &'static str = "set";
}

impl<T: ToKey, S: Store + Default> Set<T, S> {
    /// A set on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T: ToKey, S: Store> Set<T, S> {
    /// A set on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
            _type: PhantomData,
        }
    }

    fn storage_key(&self, v: &T) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "s", &v.to_key())
    }

    fn base(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "s", &[])
    }

    fn count_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "n", &[])
    }

    /// Add `v`; `true` when it was not there yet.
    pub fn insert(&mut self, v: &T) -> Result<bool, StorageError> {
        let key = self.storage_key(v)?;
        if self.store.get(&key).is_some() {
            return Ok(false);
        }
        self.store.put(&key, PRESENT);
        self.store.add(&self.count_key()?, 1);
        Ok(true)
    }

    /// Remove `v`; `true` when it was there.
    pub fn remove(&mut self, v: &T) -> Result<bool, StorageError> {
        let key = self.storage_key(v)?;
        if self.store.get(&key).is_none() {
            return Ok(false);
        }
        self.store.delete(&key);
        self.store.add(&self.count_key()?, -1);
        Ok(true)
    }

    /// Whether `v` is in the set.
    pub fn contains(&self, v: &T) -> Result<bool, StorageError> {
        Ok(self.store.get(&self.storage_key(v)?).is_some())
    }

    /// How many elements there are: one read of the stored count, never a scan.
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &self.count_key()?, &self.name)
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        Ok(self.len()? == 0)
    }

    /// Remove every element, a batch at a time; returns how many there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        let base = self.base()?;
        let count = self.count_key()?;
        let removed = clear_prefix(&mut self.store, &base);
        self.store.delete(&count);
        Ok(removed)
    }
}

impl<T: ToKey + FromKey, S: Store> Set<T, S> {
    fn page_under(&self, base: &[u8], opts: &FindOpts) -> Result<Page<T>, StorageError> {
        let set_base = self.base()?;
        let (entries, next) = read_page(&self.store, base, opts)?;
        let items = entries
            .into_iter()
            .map(|(k, _)| {
                T::from_key(&key_after(&k, &set_base)?)
                    .ok_or_else(|| StorageError::Decode("an element of another type".into()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Page { items, next })
    }

    /// Elements in order (largest first with `reverse`), one page of at most 1,000; `from`
    /// and `to` bound them (inclusive, exclusive) as key parts.
    pub fn page(&self, opts: FindOpts) -> Result<Page<T>, StorageError> {
        self.page_under(&self.base()?, &opts)
    }

    /// Elements in `[start, end)`, in order, one page at a time.
    pub fn range(&self, start: &T, end: &T, opts: FindOpts) -> Result<Page<T>, StorageError> {
        self.page(opts.from(start.to_key()).to(end.to_key()))
    }

    /// Elements whose tuple starts with the whole parts `prefix`, in order, one page at a time.
    pub fn prefix(&self, prefix: &[KeyPart], opts: FindOpts) -> Result<Page<T>, StorageError> {
        let base = [self.base()?, key::encode(prefix)].concat();
        check_key(&base)?;
        self.page_under(&base, &opts)
    }

    /// Elements starting with `prefix`, whose last string or bytes part matches as a prefix
    /// of that part: `starts_with(&["ap".to_key_part()], ..)` finds `"apple"` and `"apricot"`.
    /// In order, one page at a time; `from` and `to` do not apply.
    pub fn starts_with(&self, prefix: &[KeyPart], opts: FindOpts) -> Result<Page<T>, StorageError> {
        let base = [self.base()?, key::encode_prefix(prefix)].concat();
        check_key(&base)?;
        let opts = FindOpts {
            from: None,
            to: None,
            ..opts
        };
        self.page_under(&base, &opts)
    }

    /// The smallest element.
    pub fn first(&self) -> Result<Option<T>, StorageError> {
        Ok(self.page(FindOpts::default().limit(1))?.items.pop())
    }

    /// The largest element.
    pub fn last(&self) -> Result<Option<T>, StorageError> {
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

    fn set() -> (Set<String, MemStore>, MemStore) {
        let store = MemStore::new();
        (Set::with_store("s", store.clone()), store)
    }

    #[test]
    fn insert_remove_contains_len() {
        let (mut s, _) = set();
        assert!(s.insert(&"b".into()).unwrap());
        assert!(!s.insert(&"b".into()).unwrap());
        assert!(s.insert(&"a".into()).unwrap());
        assert_eq!(s.len().unwrap(), 2);
        assert!(s.contains(&"a".into()).unwrap());
        assert!(s.remove(&"a".into()).unwrap());
        assert!(!s.remove(&"a".into()).unwrap());
        assert_eq!(s.len().unwrap(), 1);
        assert!(!s.contains(&"a".into()).unwrap());
    }

    #[test]
    fn len_is_one_read_and_the_count_is_kept_with_add() {
        let (mut s, store) = set();
        for i in 0..50 {
            let _ = s.insert(&format!("{i}")).unwrap();
        }
        store.reset_pulled();
        assert_eq!(s.len().unwrap(), 50);
        assert_eq!(store.pulled(), 0, "no scan");
        assert_eq!(store.op_counts(&s.count_key().unwrap()), (0, 50));
    }

    #[test]
    fn ordered_reads() {
        let (mut s, _) = set();
        for v in ["banana", "apple", "cherry", "apricot"] {
            let _ = s.insert(&v.into()).unwrap();
        }
        assert_eq!(
            s.page(FindOpts::default()).unwrap().items,
            ["apple", "apricot", "banana", "cherry"]
        );
        assert_eq!(
            s.range(&"apricot".into(), &"cherry".into(), FindOpts::default())
                .unwrap()
                .items,
            ["apricot", "banana"]
        );
        assert_eq!(
            s.starts_with(&["ap".to_key_part()], FindOpts::default())
                .unwrap()
                .items,
            ["apple", "apricot"]
        );
        assert_eq!(s.first().unwrap().as_deref(), Some("apple"));
        assert_eq!(s.last().unwrap().as_deref(), Some("cherry"));
        let page = s.page(FindOpts::default().limit(3)).unwrap();
        let rest = s
            .page(FindOpts::default().cursor(page.next.unwrap()))
            .unwrap();
        assert_eq!(rest.items, ["cherry"]);
        assert!(rest.next.is_none());
    }

    #[test]
    fn integer_elements_sort_numerically() {
        let mut s: Set<i64, MemStore> = Set::with_store("ints", MemStore::new());
        for v in [10, -3, 2, 1000] {
            let _ = s.insert(&v).unwrap();
        }
        assert_eq!(
            s.page(FindOpts::default()).unwrap().items,
            [-3, 2, 10, 1000]
        );
    }

    #[test]
    fn clear_empties_the_set() {
        let (mut s, store) = set();
        for i in 0..1_500 {
            let _ = s.insert(&format!("{i:04}")).unwrap();
        }
        assert_eq!(s.clear().unwrap(), 1_500);
        assert_eq!(s.len().unwrap(), 0);
        assert!(s.first().unwrap().is_none());
        assert!(store.is_empty());
    }
}
