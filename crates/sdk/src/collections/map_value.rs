//! What an [`UnorderedMap`](super::UnorderedMap) holds under a key: a plain value, stored as
//! one CBOR entry, or a whole collection with its own keys.
//!
//! - A plain value (`V: Serialize + DeserializeOwned`) is stored at `(path.., "m", key..)`,
//!   the map's count at `(path.., "n")`, as maps always were. `get` answers `Option<V>`.
//! - A collection (`Vector`, `UnorderedMap`, `Set`, `Cell`, `Counter`, `Frozen`, `PerUser`,
//!   `Text`, `RichText`, `Table`) lives at the map's path, then `"c"` and the key's encoded
//!   tuple as one bytes part: `(path.., "c", encode(key), <its own keys>)`. `get` answers the
//!   collection itself, a handle that reads nothing until used, so a `Vector` per key costs
//!   what a state field's `Vector` costs: nothing is serialized whole.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::collections::{
    Cell, Counter, Frozen, Nestable, PerUser, RichText, Set, Text, UnorderedMap, Vector,
};
use crate::commons::{
    bounds, clear_prefix, collection_key, encode, iter_entries, key_after, read_count, Entry,
    KeyPath, NESTED,
};
use crate::encoding::key::{self, FromKey, KeyPart, ToKey};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{RangeCursor, Store};
use crate::table::{FindOpts, Iter, Record, Table};

/// A type an [`UnorderedMap`] can hold as its value: any serde value, or any collection.
///
/// The map's `get`, `contains`, `remove`, `is_empty`, `clear`, `iter` and `keys` work for
/// both and answer per kind ([`MapValue::Get`], [`MapValue::Removed`], [`MapValue::Item`]);
/// `insert`, `len`, `page`, `range` and the rest are for plain values.
pub trait MapValue<S: Store>: Sized {
    /// What `get` answers: `Option<V>` for a value, the collection itself for a collection.
    type Get;
    /// What `remove` answers: the value it held, or how many stored entries the collection had.
    type Removed;
    /// What `iter` yields beside each key: the value, or the collection.
    type Item;

    #[doc(hidden)]
    fn get(store: &S, path: &KeyPath, key: &[KeyPart]) -> Result<Self::Get, StorageError>;
    #[doc(hidden)]
    fn contains(store: &S, path: &KeyPath, key: &[KeyPart]) -> Result<bool, StorageError>;
    #[doc(hidden)]
    fn remove(
        store: &mut S,
        path: &KeyPath,
        key: &[KeyPart],
    ) -> Result<Self::Removed, StorageError>;
    #[doc(hidden)]
    fn is_empty(store: &S, path: &KeyPath) -> Result<bool, StorageError>;
    #[doc(hidden)]
    fn clear(store: &mut S, path: &KeyPath) -> Result<u64, StorageError>;
    #[doc(hidden)]
    fn entries<'a, K: FromKey + 'a>(store: &'a S, path: &KeyPath) -> Iter<'a, (K, Self::Item)>
    where
        Self: 'a;
}

// --- plain values ---------------------------------------------------------------------

/// A plain value's stored entry decoded, its key read after `map_base`.
pub(crate) fn decode_value<K: FromKey, V: DeserializeOwned>(
    map_base: &[u8],
    (k, v): Entry,
) -> Result<(K, V), StorageError> {
    let parts = key_after(&k, map_base)?;
    let key =
        K::from_key(&parts).ok_or_else(|| StorageError::Decode("a key of another type".into()))?;
    Ok((key, value::from_slice(&v)?))
}

impl<V: Serialize + DeserializeOwned, S: Store> MapValue<S> for V {
    type Get = Option<V>;
    type Removed = Option<V>;
    type Item = V;

    fn get(store: &S, path: &KeyPath, key: &[KeyPart]) -> Result<Option<V>, StorageError> {
        store
            .get(&collection_key(path, "m", key)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    fn contains(store: &S, path: &KeyPath, key: &[KeyPart]) -> Result<bool, StorageError> {
        Ok(store.get(&collection_key(path, "m", key)?).is_some())
    }

    fn remove(store: &mut S, path: &KeyPath, key: &[KeyPart]) -> Result<Option<V>, StorageError> {
        let stored = collection_key(path, "m", key)?;
        let Some(bytes) = store.get(&stored) else {
            return Ok(None);
        };
        let old = value::from_slice(&bytes)?;
        store.delete(&stored);
        store.add(&collection_key(path, "n", &[])?, -1);
        Ok(Some(old))
    }

    fn is_empty(store: &S, path: &KeyPath) -> Result<bool, StorageError> {
        Ok(read_count(store, &collection_key(path, "n", &[])?, path.label())? == 0)
    }

    fn clear(store: &mut S, path: &KeyPath) -> Result<u64, StorageError> {
        let base = collection_key(path, "m", &[])?;
        let count = collection_key(path, "n", &[])?;
        let removed = clear_prefix(store, &base);
        store.delete(&count);
        Ok(removed)
    }

    fn entries<'a, K: FromKey + 'a>(store: &'a S, path: &KeyPath) -> Iter<'a, (K, V)>
    where
        V: 'a,
    {
        let base = match collection_key(path, "m", &[]) {
            Ok(b) => b,
            Err(e) => return Iter::failed(e),
        };
        let map_base = base.clone();
        iter_entries(store, &base, &FindOpts::default(), move |e| {
            decode_value(&map_base, e)
        })
    }
}

// --- collections ----------------------------------------------------------------------

/// The encoded prefix every collection value's keys start with: `(path.., "c")`.
fn children(path: &KeyPath) -> Result<Vec<u8>, StorageError> {
    let mut parts = path.parts().to_vec();
    parts.push(KeyPart::Str(NESTED.into()));
    encode(&parts)
}

/// The path of the collection under `key`, refused (`KeyTooLong`) when its keys cannot fit.
fn child(path: &KeyPath, key: &[KeyPart]) -> Result<KeyPath, StorageError> {
    let child = path.child(key);
    let _fits = encode(child.parts())?;
    Ok(child)
}

fn collection_get<C: Nestable<S>, S: Store + Clone>(
    store: &S,
    path: &KeyPath,
    key: &[KeyPart],
) -> Result<C, StorageError> {
    Ok(C::at(child(path, key)?, store.clone()))
}

fn collection_contains<S: Store>(
    store: &S,
    path: &KeyPath,
    key: &[KeyPart],
) -> Result<bool, StorageError> {
    let prefix = encode(child(path, key)?.parts())?;
    let (start, end) = bounds(&prefix);
    Ok(!store.range(&start, &end, false).next(1).is_empty())
}

fn collection_remove<S: Store>(
    store: &mut S,
    path: &KeyPath,
    key: &[KeyPart],
) -> Result<u64, StorageError> {
    let prefix = encode(child(path, key)?.parts())?;
    Ok(clear_prefix(store, &prefix))
}

fn collection_is_empty<S: Store>(store: &S, path: &KeyPath) -> Result<bool, StorageError> {
    let (start, end) = bounds(&children(path)?);
    Ok(store.range(&start, &end, false).next(1).is_empty())
}

fn collection_clear<S: Store>(store: &mut S, path: &KeyPath) -> Result<u64, StorageError> {
    let prefix = children(path)?;
    Ok(clear_prefix(store, &prefix))
}

/// Every key whose collection holds anything, with that collection, in key order. It skips
/// over each key's rows: two host calls per key (a cursor opened past the last key, one entry
/// read), however many rows the key's collection has.
fn collection_entries<'a, K: FromKey + 'a, C: Nestable<S> + 'a, S: Store + Clone>(
    store: &'a S,
    path: &KeyPath,
) -> Iter<'a, (K, C)> {
    let all = match children(path) {
        Ok(c) => c,
        Err(e) => return Iter::failed(e),
    };
    let path = path.clone();
    let (mut start, end) = bounds(&all);
    let mut done = false;
    Iter::from_batches(move || {
        if done {
            return Ok(Vec::new());
        }
        let Some((stored, _)) = store.range(&start, &end, false).next(1).into_iter().next() else {
            done = true;
            return Ok(Vec::new());
        };
        let parts = key_after(&stored, &all)?;
        let Some(KeyPart::Bytes(key_bytes)) = parts.first() else {
            return Err(StorageError::Decode("not a collection value's key".into()));
        };
        let key_parts = key::decode(key_bytes)
            .ok_or_else(|| StorageError::Decode("not a collection value's key".into()))?;
        let k = K::from_key(&key_parts)
            .ok_or_else(|| StorageError::Decode("a key of another type".into()))?;
        let own = child(&path, &key_parts)?;
        // Past every row of this key: its prefix, then 0xFF, which no tag reaches.
        start = [encode(own.parts())?.as_slice(), &[0xFF]].concat();
        Ok(vec![(k, C::at(own, store.clone()))])
    })
}

/// A collection as a map value: `get` answers the collection under the key.
macro_rules! collection_value {
    ($([$($gen:tt)*] $ty:ty),* $(,)?) => {$(
        impl<$($gen)*> MapValue<S> for $ty {
            type Get = Self;
            type Removed = u64;
            type Item = Self;

            fn get(store: &S, path: &KeyPath, key: &[KeyPart]) -> Result<Self, StorageError> {
                collection_get(store, path, key)
            }

            fn contains(store: &S, path: &KeyPath, key: &[KeyPart]) -> Result<bool, StorageError> {
                collection_contains(store, path, key)
            }

            fn remove(
                store: &mut S,
                path: &KeyPath,
                key: &[KeyPart],
            ) -> Result<u64, StorageError> {
                collection_remove(store, path, key)
            }

            fn is_empty(store: &S, path: &KeyPath) -> Result<bool, StorageError> {
                collection_is_empty(store, path)
            }

            fn clear(store: &mut S, path: &KeyPath) -> Result<u64, StorageError> {
                collection_clear(store, path)
            }

            fn entries<'a, K: FromKey + 'a>(store: &'a S, path: &KeyPath) -> Iter<'a, (K, Self)>
            where
                Self: 'a,
            {
                collection_entries(store, path)
            }
        }
    )*};
}

collection_value! {
    [T, S: Store + Clone] Vector<T, S>,
    [K2: ToKey, V2, S: Store + Clone] UnorderedMap<K2, V2, S>,
    [T: ToKey, S: Store + Clone] Set<T, S>,
    [T, S: Store + Clone] Cell<T, S>,
    [S: Store + Clone] Counter<S>,
    [T, H, S: Store + Clone] Frozen<T, H, S>,
    [T, S: Store + Clone] PerUser<T, S>,
    [S: Store + Clone] Text<S>,
    [S: Store + Clone] RichText<S>,
    [T: Record, S: Store + Clone] Table<T, S>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    #[test]
    fn a_state_fields_keys_do_not_move() {
        // A top-level collection's path is its name alone, so its keys are what they were.
        let store = MemStore::new();
        let mut v: Vector<u32, MemStore> = Vector::with_store("v", store.clone());
        let _ = v.push(&7).unwrap();
        for parts in [
            vec![
                KeyPart::Str("v".into()),
                KeyPart::Str("e".into()),
                KeyPart::Int(0),
            ],
            vec![KeyPart::Str("v".into()), KeyPart::Str("n".into())],
        ] {
            assert!(store.get(&key::encode(&parts)).is_some(), "{parts:?}");
        }
        let mut m: UnorderedMap<String, u32, MemStore> =
            UnorderedMap::with_store("m", store.clone());
        let _ = m.insert(&"k".into(), &1).unwrap();
        let at = key::encode(&[
            KeyPart::Str("m".into()),
            KeyPart::Str("m".into()),
            KeyPart::Str("k".into()),
        ]);
        assert!(
            store.get(&at).is_some(),
            "a plain value stays at (name, \"m\", key)"
        );
    }

    #[test]
    fn a_vector_per_key_is_its_own_vector() {
        let store = MemStore::new();
        let feeds: UnorderedMap<String, Vector<u32, MemStore>, MemStore> =
            UnorderedMap::with_store("feeds", store.clone());
        let mut ann = feeds.get(&"ann".into()).unwrap();
        let mut bo = feeds.get(&"bo".into()).unwrap();
        let _ = ann.extend(&[1, 2, 3]).unwrap();
        let _ = bo.push(&9).unwrap();
        assert_eq!(ann.len().unwrap(), 3);
        assert_eq!(bo.len().unwrap(), 1);
        let read: Vec<u32> = feeds
            .get(&"ann".into())
            .unwrap()
            .iter()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(read, [1, 2, 3]);
        assert!(feeds.get(&"cy".into()).unwrap().is_empty().unwrap());
        // Pushing one element writes its row and its length, nothing else.
        let writes = store.writes();
        let _ = ann.push(&4).unwrap();
        assert_eq!(store.writes() - writes, 2);
    }

    #[test]
    fn keys_skip_over_each_collections_rows() {
        let store = MemStore::new();
        let mut tags: UnorderedMap<String, Set<u32, MemStore>, MemStore> =
            UnorderedMap::with_store("tags", store.clone());
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
        let sizes: Vec<(String, u64)> = tags
            .iter()
            .map(|e| e.and_then(|(k, set)| Ok((k, set.len()?))))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(sizes, [("a".to_string(), 3), ("c".to_string(), 1)]);
        assert_eq!(tags.clear().unwrap(), 4 + 2);
        assert!(tags.is_empty().unwrap());
    }

    #[test]
    fn maps_of_maps_go_more_than_one_level_and_keep_neighbours_apart() {
        let store = MemStore::new();
        type Card = UnorderedMap<String, String, MemStore>;
        type Board = UnorderedMap<u32, Card, MemStore>;
        let boards: UnorderedMap<String, Board, MemStore> =
            UnorderedMap::with_store("boards", store.clone());
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
            card.len().unwrap(),
            1,
            "the innermost map keeps its own count"
        );
        assert_eq!(
            cell.get().unwrap(),
            Some(1),
            "the neighbouring field is untouched"
        );
    }

    #[test]
    fn counters_and_texts_are_values_too() {
        let store = MemStore::new();
        let mut visits: UnorderedMap<String, Counter<MemStore>, MemStore> =
            UnorderedMap::with_store("visits", store.clone());
        let mut home = visits.get(&"home".into()).unwrap();
        home.add(2).unwrap();
        assert_eq!(visits.get(&"home".into()).unwrap().get().unwrap(), 2);
        assert_eq!(visits.get(&"about".into()).unwrap().get().unwrap(), 0);
        assert_eq!(visits.remove(&"home".into()).unwrap(), 1);
        let docs: UnorderedMap<u32, Text<MemStore>, MemStore> =
            UnorderedMap::with_store("docs", store);
        let mut one = docs.get(&1).unwrap();
        let _ids = one.insert(0, "hello").unwrap();
        assert_eq!(docs.get(&1).unwrap().read().unwrap(), "hello");
    }

    #[test]
    fn a_key_too_long_for_its_rows_is_refused() {
        let feeds: UnorderedMap<String, Vector<u32, MemStore>, MemStore> =
            UnorderedMap::with_store("feeds", MemStore::new());
        let long = "x".repeat(1_100);
        assert!(matches!(
            feeds.get(&long),
            Err(StorageError::KeyTooLong { .. })
        ));
    }
}
