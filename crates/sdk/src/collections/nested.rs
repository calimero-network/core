//! `Nested<K, C>`: a collection `C` under each key `K`, at `(name, "c", key, ..)`.
//!
//! Each key's collection is an ordinary one whose keys start at the parent's path, then
//! `"c"` and the key encoded as one bytes part (`KeyPath::child`). Nothing is stored for the
//! key itself: a key exists while its collection holds anything, and reading an absent one
//! gives an empty collection.

use core::marker::PhantomData;

use crate::app::StateField;
use crate::collections::Nestable;
use crate::commons::{bounds, clear_prefix, encode, key_after, KeyPath, NESTED};
use crate::encoding::key::{self, FromKey, KeyPart, ToKey};
use crate::error::StorageError;
use crate::store::{DefaultStore, RangeCursor, Store};
use crate::table::Iter;

/// A collection per key, each as cheap as a state field's own: a `Vector` per account, a
/// `Set` per tag, a map of maps.
///
/// ```ignore
/// #[app::state(version = 1)]
/// pub struct Feed {
///     posts: Nested<String, Vector<Post>>,
/// }
///
/// let mut mine = self.posts.get(&account)?; // Vector<Post> under `account`
/// mine.push(&post)?;                         // one row and the length, as any Vector
/// let n = mine.len()?;                       // one read: this account's length
/// ```
///
/// A handle reads and writes the store directly, so it holds no copy of the data: getting
/// one reads nothing, and two handles for one key see the same rows. A view's handle can only
/// read: the host refuses a write outside a logged run, as for any collection.
///
/// `C` is any collection, a [`Table`](crate::Table), or `Nested` itself for another level.
pub struct Nested<K, C, S: Store = DefaultStore> {
    path: KeyPath,
    store: S,
    _types: PhantomData<fn() -> (K, C)>,
}

impl<K, C, S: Store> StateField for Nested<K, C, S> {
    const KIND: &'static str = "nested";
}

impl<K, C, S: Store> Nestable<S> for Nested<K, C, S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self {
            path,
            store,
            _types: PhantomData,
        }
    }
}

impl<K: ToKey, C: Nestable<S>, S: Store + Clone + Default> Nested<K, C, S> {
    /// A nested collection on the default store; the name is part of every key.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<K: ToKey, C: Nestable<S>, S: Store + Clone> Nested<K, C, S> {
    /// A nested collection on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self::at(KeyPath::root(name), store)
    }

    /// The encoded prefix every child's key starts with: `(path.., "c")`.
    fn children(&self) -> Result<Vec<u8>, StorageError> {
        let mut parts = self.path.parts().to_vec();
        parts.push(KeyPart::Str(NESTED.into()));
        encode(&parts)
    }

    /// The path of `k`'s collection, refused (`KeyTooLong`) when its keys could not fit.
    fn child(&self, k: &K) -> Result<KeyPath, StorageError> {
        let path = self.child_path(&k.to_key());
        let _fits = encode(path.parts())?;
        Ok(path)
    }

    fn child_path(&self, key: &[KeyPart]) -> KeyPath {
        self.path.child(key)
    }

    /// The collection under `k`: a handle, read and written like a state field's collection.
    /// Getting it reads nothing; an absent key's collection is empty.
    pub fn get(&self, k: &K) -> Result<C, StorageError> {
        Ok(C::at(self.child(k)?, self.store.clone()))
    }

    /// Whether `k`'s collection holds anything: one cursor read of one entry.
    pub fn contains(&self, k: &K) -> Result<bool, StorageError> {
        let prefix = encode(self.child(k)?.parts())?;
        let (start, end) = bounds(&prefix);
        Ok(!self.store.range(&start, &end, false).next(1).is_empty())
    }

    /// Whether no key holds anything: one cursor read of one entry.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        let (start, end) = bounds(&self.children()?);
        Ok(self.store.range(&start, &end, false).next(1).is_empty())
    }

    /// Remove `k`'s whole collection, a batch at a time; returns how many stored entries it
    /// held (its rows and its length, if it keeps one).
    pub fn remove(&mut self, k: &K) -> Result<u64, StorageError> {
        let prefix = encode(self.child(k)?.parts())?;
        Ok(clear_prefix(&mut self.store, &prefix))
    }

    /// Remove every key's collection, a batch at a time; returns how many stored entries
    /// there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        let children = self.children()?;
        Ok(clear_prefix(&mut self.store, &children))
    }
}

impl<K: ToKey + FromKey, C: Nestable<S>, S: Store + Clone> Nested<K, C, S> {
    /// Every key that holds anything, in key order. It skips over each key's rows: two host
    /// calls per key (open a cursor past the last key, read one entry), however many rows
    /// the key's collection has.
    pub fn keys(&self) -> Iter<'_, K> {
        let children = match self.children() {
            Ok(c) => c,
            Err(e) => return Iter::failed(e),
        };
        let (mut start, end) = bounds(&children);
        let mut done = false;
        Iter::from_batches(move || {
            if done {
                return Ok(Vec::new());
            }
            let Some((stored, _)) = self
                .store
                .range(&start, &end, false)
                .next(1)
                .into_iter()
                .next()
            else {
                done = true;
                return Ok(Vec::new());
            };
            let (key_bytes, k) = decode_child_key::<K>(&stored, &children)?;
            // Past every row of this key: its prefix, then 0xFF, which no tag reaches.
            let own = encode(
                &[
                    self.path.parts(),
                    &[KeyPart::Str(NESTED.into()), KeyPart::Bytes(key_bytes)],
                ]
                .concat(),
            )?;
            start = [own.as_slice(), &[0xFF]].concat();
            Ok(vec![k])
        })
    }

    /// Every key that holds anything, with its collection, in key order (see
    /// [`Nested::keys`] for the cost).
    pub fn iter(&self) -> impl Iterator<Item = Result<(K, C), StorageError>> + '_ {
        self.keys().map(move |k| {
            let k = k?;
            let c = self.get(&k)?;
            Ok((k, c))
        })
    }
}

/// The key a stored child row belongs to: the bytes part right after `(path.., "c")`, and the
/// key tuple it encodes.
fn decode_child_key<K: FromKey>(
    stored: &[u8],
    children: &[u8],
) -> Result<(Vec<u8>, K), StorageError> {
    let parts = key_after(stored, children)?;
    let Some(KeyPart::Bytes(key_bytes)) = parts.first() else {
        return Err(StorageError::Decode("not a nested key".into()));
    };
    let k = key::decode(key_bytes)
        .and_then(|parts| K::from_key(&parts))
        .ok_or_else(|| StorageError::Decode("a nested key of another type".into()))?;
    Ok((key_bytes.clone(), k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::{Cell, Counter, Set, UnorderedMap, Vector};
    use crate::store::MemStore;

    #[test]
    fn a_state_fields_keys_do_not_move() {
        // A top-level collection's path is its name alone, so its keys are what they were.
        let store = MemStore::new();
        let mut v: Vector<u32, MemStore> = Vector::with_store("v", store.clone());
        let _ = v.push(&7).unwrap();
        let mut want = Vec::new();
        for parts in [
            vec![
                KeyPart::Str("v".into()),
                KeyPart::Str("e".into()),
                KeyPart::Int(0),
            ],
            vec![KeyPart::Str("v".into()), KeyPart::Str("n".into())],
        ] {
            want.push(key::encode(&parts));
        }
        for k in want {
            assert!(store.get(&k).is_some(), "{k:?}");
        }
    }

    #[test]
    fn each_key_has_its_own_collection() {
        let store = MemStore::new();
        let feeds: Nested<String, Vector<u32>, MemStore> = Nested::with_store("feeds", store);
        let mut ann = feeds.get(&"ann".into()).unwrap();
        let mut bo = feeds.get(&"bo".into()).unwrap();
        let _ = ann.extend(&[1, 2, 3]).unwrap();
        let _ = bo.push(&9).unwrap();
        assert_eq!(ann.len().unwrap(), 3);
        assert_eq!(bo.len().unwrap(), 1);
        let again = feeds.get(&"ann".into()).unwrap();
        let read: Vec<u32> = again.iter().collect::<Result<_, _>>().unwrap();
        assert_eq!(read, [1, 2, 3]);
        assert!(feeds.get(&"cy".into()).unwrap().is_empty().unwrap());
    }

    #[test]
    fn keys_skip_over_each_collections_rows() {
        let store = MemStore::new();
        let mut tags: Nested<String, Set<u32>, MemStore> =
            Nested::with_store("tags", store.clone());
        assert!(tags.is_empty().unwrap());
        for (k, n) in [("b", 1_500u32), ("a", 3), ("c", 1)] {
            let values: Vec<u32> = (0..n).collect();
            let _ = tags.get(&k.into()).unwrap().extend(&values).unwrap();
        }
        store.reset_calls();
        let keys: Vec<String> = tags.keys().collect::<Result<_, _>>().unwrap();
        assert_eq!(keys, ["a", "b", "c"]);
        // Two calls per key and two for the end, not a read of b's 1,500 rows.
        assert_eq!(store.calls(), 8);
        assert!(tags.contains(&"b".into()).unwrap());
        assert!(!tags.contains(&"z".into()).unwrap());
        assert_eq!(tags.remove(&"b".into()).unwrap(), 1_501);
        assert!(!tags.contains(&"b".into()).unwrap());
        let left: Vec<String> = tags
            .iter()
            .map(|e| e.map(|(k, _)| k))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(left, ["a", "c"]);
        assert_eq!(tags.clear().unwrap(), 4 + 2);
        assert!(tags.is_empty().unwrap());
    }

    #[test]
    fn nesting_goes_more_than_one_level_and_keeps_neighbours_apart() {
        let store = MemStore::new();
        let boards: Nested<String, Nested<u32, UnorderedMap<String, String>>, MemStore> =
            Nested::with_store("boards", store.clone());
        let mut cell: Cell<u32, MemStore> = Cell::with_store("boards2", store.clone());
        cell.set(&1).unwrap();
        let mut card = boards.get(&"team".into()).unwrap().get(&7).unwrap();
        let _ = card.insert(&"title".into(), &"ship it".into()).unwrap();
        let mut other = boards.get(&"team".into()).unwrap().get(&8).unwrap();
        let _ = other.insert(&"title".into(), &"later".into()).unwrap();
        let team = boards.get(&"team".into()).unwrap();
        let ids: Vec<u32> = team.keys().collect::<Result<_, _>>().unwrap();
        assert_eq!(ids, [7, 8]);
        assert_eq!(
            team.get(&7)
                .unwrap()
                .get(&"title".into())
                .unwrap()
                .as_deref(),
            Some("ship it")
        );
        assert_eq!(
            cell.get().unwrap(),
            Some(1),
            "the neighbouring field is untouched"
        );
    }

    #[test]
    fn a_counter_and_a_table_nest_too() {
        let store = MemStore::new();
        let mut visits: Nested<String, Counter<MemStore>, MemStore> =
            Nested::with_store("visits", store);
        let mut home = visits.get(&"home".into()).unwrap();
        home.add(2).unwrap();
        assert_eq!(visits.get(&"home".into()).unwrap().get().unwrap(), 2);
        assert_eq!(visits.get(&"about".into()).unwrap().get().unwrap(), 0);
        assert_eq!(visits.remove(&"home".into()).unwrap(), 1);
    }

    #[test]
    fn a_text_nests_with_its_ids() {
        use crate::collections::Text;
        let docs: Nested<u32, Text<MemStore>, MemStore> =
            Nested::with_store("docs", MemStore::new());
        let mut one = docs.get(&1).unwrap();
        let _ids = one.insert(0, "hello").unwrap();
        let mut two = docs.get(&2).unwrap();
        let _ids = two.insert(0, "bye").unwrap();
        assert_eq!(docs.get(&1).unwrap().read().unwrap(), "hello");
        assert_eq!(two.len().unwrap(), 3);
    }

    #[test]
    fn a_key_too_long_for_its_rows_is_refused() {
        let feeds: Nested<String, Vector<u32>, MemStore> =
            Nested::with_store("feeds", MemStore::new());
        let long = "x".repeat(1_100);
        assert!(matches!(
            feeds.get(&long),
            Err(StorageError::KeyTooLong { .. })
        ));
    }
}
