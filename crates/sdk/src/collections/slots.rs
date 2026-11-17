//! What a container holds in each slot (an [`UnorderedMap`](super::UnorderedMap) under a key,
//! a [`Vector`](super::Vector) at a position, a [`PerUser`](super::PerUser) per account): a
//! plain value, stored as one CBOR entry, or a whole collection with its own keys.
//!
//! A collection in a slot lives at the container's path, then `"c"` and the slot's key as
//! one bytes part (`[key..]`, `[Int(position)]`, `[Str(account)]`), whatever the container.
//! A `Set`, a `Frozen` and a `Cell` have no slot to hold one: a set's elements are its keys,
//! a frozen value is its own content, and a cell is a single slot (`Cell<Vector<T>>` would be
//! the `Vector`).
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

/// A type a [`Vector`] can hold at each position: any serde value, or any collection.
///
/// `get`, `pop`, `iter` and `clear` work for both and answer per kind; `push`, `set`,
/// `insert`, `remove` and the rest are for plain values, and `push_new` appends a collection.
pub trait VecElement<S: Store>: Sized {
    /// What `get` answers: `Option<T>`, or `Option<C>` (the collection at that position).
    type Get;
    /// What `pop` answers: the value, or how many stored entries the collection had.
    type Popped;
    /// What `iter` yields: the value, or the collection.
    type Item;

    #[doc(hidden)]
    fn get(store: &S, path: &KeyPath, index: u64) -> Result<Self::Get, StorageError>;
    #[doc(hidden)]
    fn pop(store: &mut S, path: &KeyPath) -> Result<Option<Self::Popped>, StorageError>;
    #[doc(hidden)]
    fn clear(store: &mut S, path: &KeyPath) -> Result<u64, StorageError>;
    #[doc(hidden)]
    fn entries<'a>(store: &'a S, path: &KeyPath) -> Iter<'a, Self::Item>
    where
        Self: 'a;
}

/// A type a [`PerUser`] can hold per account: any serde value, or any collection.
///
/// `get`, `get_for`, `contains_for`, `remove` and `iter` work for both and answer per kind;
/// `set`, `page` and `iter_with` are for plain values.
pub trait UserValue<S: Store>: Sized {
    /// What `get` answers: `Option<T>`, or the account's collection.
    type Get;
    /// What `remove` answers: the value, or how many stored entries the collection had.
    type Removed;
    /// What `iter` yields beside each account.
    type Item;

    #[doc(hidden)]
    fn get(store: &S, path: &KeyPath, account: &str) -> Result<Self::Get, StorageError>;
    #[doc(hidden)]
    fn contains(store: &S, path: &KeyPath, account: &str) -> Result<bool, StorageError>;
    #[doc(hidden)]
    fn remove(store: &mut S, path: &KeyPath, account: &str) -> Result<Self::Removed, StorageError>;
    #[doc(hidden)]
    fn entries<'a>(store: &'a S, path: &KeyPath) -> Iter<'a, (String, Self::Item)>
    where
        Self: 'a;
}

/// The key part of a vector position.
fn position(index: u64) -> KeyPart {
    KeyPart::Int(index.into())
}

/// The key part of an account.
fn account_part(account: &str) -> KeyPart {
    KeyPart::Str(account.into())
}

/// A stored `(account, value)` under `base`.
pub(crate) fn decode_account<T: DeserializeOwned>(
    base: &[u8],
    (k, v): Entry,
) -> Result<(String, T), StorageError> {
    match key_after(&k, base)?.as_slice() {
        [KeyPart::Str(account)] => Ok((account.clone(), value::from_slice(&v)?)),
        _ => Err(StorageError::Decode("not an account key".into())),
    }
}

impl<V: Serialize + DeserializeOwned, S: Store> VecElement<S> for V {
    type Get = Option<V>;
    type Popped = V;
    type Item = V;

    fn get(store: &S, path: &KeyPath, index: u64) -> Result<Option<V>, StorageError> {
        store
            .get(&collection_key(path, "e", &[position(index)])?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    fn pop(store: &mut S, path: &KeyPath) -> Result<Option<V>, StorageError> {
        let len_key = collection_key(path, "n", &[])?;
        let Some(last) = read_count(store, &len_key, path.label())?.checked_sub(1) else {
            return Ok(None);
        };
        let key = collection_key(path, "e", &[position(last)])?;
        let old = store
            .get(&key)
            .ok_or(StorageError::Dangling)
            .and_then(|b| value::from_slice(&b))?;
        store.delete(&key);
        store.add(&len_key, -1);
        Ok(Some(old))
    }

    fn clear(store: &mut S, path: &KeyPath) -> Result<u64, StorageError> {
        let base = collection_key(path, "e", &[])?;
        let len_key = collection_key(path, "n", &[])?;
        let removed = clear_prefix(store, &base);
        store.delete(&len_key);
        Ok(removed)
    }

    fn entries<'a>(store: &'a S, path: &KeyPath) -> Iter<'a, V>
    where
        V: 'a,
    {
        match collection_key(path, "e", &[]) {
            Ok(base) => iter_entries(store, &base, &FindOpts::default(), |(_, v)| {
                value::from_slice(&v)
            }),
            Err(e) => Iter::failed(e),
        }
    }
}

impl<V: Serialize + DeserializeOwned, S: Store> UserValue<S> for V {
    type Get = Option<V>;
    type Removed = Option<V>;
    type Item = V;

    fn get(store: &S, path: &KeyPath, account: &str) -> Result<Option<V>, StorageError> {
        store
            .get(&collection_key(path, "u", &[account_part(account)])?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    fn contains(store: &S, path: &KeyPath, account: &str) -> Result<bool, StorageError> {
        Ok(store
            .get(&collection_key(path, "u", &[account_part(account)])?)
            .is_some())
    }

    fn remove(store: &mut S, path: &KeyPath, account: &str) -> Result<Option<V>, StorageError> {
        let key = collection_key(path, "u", &[account_part(account)])?;
        let Some(bytes) = store.get(&key) else {
            return Ok(None);
        };
        let old = value::from_slice(&bytes)?;
        store.delete(&key);
        Ok(Some(old))
    }

    fn entries<'a>(store: &'a S, path: &KeyPath) -> Iter<'a, (String, V)>
    where
        V: 'a,
    {
        match collection_key(path, "u", &[]) {
            Ok(base) => {
                let under = base.clone();
                iter_entries(store, &base, &FindOpts::default(), move |e| {
                    decode_account(&under, e)
                })
            }
            Err(e) => Iter::failed(e),
        }
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

/// Whether a stored row under a collection's own prefix is bookkeeping rather than content: a
/// length or count at 0 (`(.., "n")`, a table's `(.., "c", name, ..)`), or `Text`'s next id
/// (`(.., "i")`), at the collection's level or any collection's under it. The host keeps a row
/// `storage.add` brought to 0, so an emptied collection still has these.
fn is_bookkeeping(mut parts: &[KeyPart], stored: &[u8]) -> bool {
    let zero = || value::int_from_slice(stored).is_ok_and(|n| n == 0);
    // Down through the collections nested in this one: (.., "c", key-bytes, ..).
    while let [KeyPart::Str(kind), KeyPart::Bytes(_), rest @ ..] = parts {
        if kind != NESTED {
            break;
        }
        parts = rest;
    }
    match parts {
        [KeyPart::Str(kind)] if kind == "n" => zero(),
        [KeyPart::Str(kind)] if kind == "i" => true,
        [KeyPart::Str(kind), KeyPart::Str(_), ..] if kind == "c" => zero(),
        _ => false,
    }
}

/// Whether the collection at the encoded `prefix` holds anything: its rows read in small
/// batches until one is content. Rows sort by kind, so a non-empty collection's first row is
/// content (or a count above 0) and one batch decides.
fn holds_anything<S: Store>(store: &S, prefix: &[u8]) -> Result<bool, StorageError> {
    let (start, end) = bounds(prefix);
    let mut cursor = store.range(&start, &end, false);
    loop {
        let batch = cursor.next(16);
        if batch.is_empty() {
            return Ok(false);
        }
        for (k, v) in batch {
            if !is_bookkeeping(&key_after(&k, prefix)?, &v) {
                return Ok(true);
            }
        }
    }
}

fn collection_contains<S: Store>(
    store: &S,
    path: &KeyPath,
    key: &[KeyPart],
) -> Result<bool, StorageError> {
    holds_anything(store, &encode(child(path, key)?.parts())?)
}

fn collection_remove<S: Store>(
    store: &mut S,
    path: &KeyPath,
    key: &[KeyPart],
) -> Result<u64, StorageError> {
    let prefix = encode(child(path, key)?.parts())?;
    Ok(clear_prefix(store, &prefix))
}

fn collection_is_empty<S: Store + Clone>(store: &S, path: &KeyPath) -> Result<bool, StorageError> {
    // The first key whose collection holds anything, if any.
    match collection_entries::<Vec<KeyPart>, Cell<(), S>, S>(store, path).next() {
        None => Ok(true),
        Some(entry) => entry.map(|_| false),
    }
}

fn collection_clear<S: Store>(store: &mut S, path: &KeyPath) -> Result<u64, StorageError> {
    let prefix = children(path)?;
    Ok(clear_prefix(store, &prefix))
}

/// Every key whose collection holds anything, with that collection, in key order. It skips
/// over each key's rows: two host calls per key (a cursor opened past the last key, one entry
/// read), however many rows the key's collection has; a key whose collection was emptied
/// (only zero counts left) is skipped.
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
        loop {
            let Some((stored, first)) = store.range(&start, &end, false).next(1).into_iter().next()
            else {
                done = true;
                return Ok(Vec::new());
            };
            let parts = key_after(&stored, &all)?;
            let Some(KeyPart::Bytes(key_bytes)) = parts.first() else {
                return Err(StorageError::Decode("not a collection value's key".into()));
            };
            let key_parts = key::decode(key_bytes)
                .ok_or_else(|| StorageError::Decode("not a collection value's key".into()))?;
            let own = child(&path, &key_parts)?;
            let own_prefix = encode(own.parts())?;
            // Past every row of this key: its prefix, then 0xFF, which no tag reaches.
            start = [own_prefix.as_slice(), &[0xFF]].concat();
            let content = !is_bookkeeping(&key_after(&stored, &own_prefix)?, &first)
                || holds_anything(store, &own_prefix)?;
            if !content {
                continue;
            }
            let k = K::from_key(&key_parts)
                .ok_or_else(|| StorageError::Decode("a key of another type".into()))?;
            return Ok(vec![(k, C::at(own, store.clone()))]);
        }
    })
}

/// The collections of a vector of collections, positions `0..len`, in order. Reading them
/// reads only the length; each handle reads nothing until used.
fn collection_positions<'a, C: Nestable<S> + 'a, S: Store + Clone>(
    store: &'a S,
    path: &KeyPath,
) -> Iter<'a, C> {
    let path = path.clone();
    let mut next: Option<(u64, u64)> = None;
    Iter::from_batches(move || {
        let (from, len) = match next {
            Some(at) => at,
            None => {
                let len_key = collection_key(&path, "n", &[])?;
                (0, read_count(store, &len_key, path.label())?)
            }
        };
        let to = len.min(from + u64::from(crate::store::MAX_RANGE_BATCH));
        next = Some((to, len));
        (from..to)
            .map(|i| Ok(C::at(child(&path, &[position(i)])?, store.clone())))
            .collect()
    })
}

/// A collection as a map value, a vector element and a per-account value: `get` answers the
/// collection in that slot.
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

        impl<$($gen)*> VecElement<S> for $ty {
            type Get = Option<Self>;
            type Popped = u64;
            type Item = Self;

            fn get(store: &S, path: &KeyPath, index: u64) -> Result<Option<Self>, StorageError> {
                let len = read_count(store, &collection_key(path, "n", &[])?, path.label())?;
                if index >= len {
                    return Ok(None);
                }
                collection_get(store, path, &[position(index)]).map(Some)
            }

            fn pop(store: &mut S, path: &KeyPath) -> Result<Option<u64>, StorageError> {
                let len_key = collection_key(path, "n", &[])?;
                let Some(last) = read_count(store, &len_key, path.label())?.checked_sub(1) else {
                    return Ok(None);
                };
                let removed = collection_remove(store, path, &[position(last)])?;
                store.add(&len_key, -1);
                Ok(Some(removed))
            }

            fn clear(store: &mut S, path: &KeyPath) -> Result<u64, StorageError> {
                let removed = collection_clear(store, path)?;
                store.delete(&collection_key(path, "n", &[])?);
                Ok(removed)
            }

            fn entries<'a>(store: &'a S, path: &KeyPath) -> Iter<'a, Self>
            where
                Self: 'a,
            {
                collection_positions(store, path)
            }
        }

        impl<$($gen)*> UserValue<S> for $ty {
            type Get = Self;
            type Removed = u64;
            type Item = Self;

            fn get(store: &S, path: &KeyPath, account: &str) -> Result<Self, StorageError> {
                collection_get(store, path, &[account_part(account)])
            }

            fn contains(store: &S, path: &KeyPath, account: &str) -> Result<bool, StorageError> {
                collection_contains(store, path, &[account_part(account)])
            }

            fn remove(store: &mut S, path: &KeyPath, account: &str) -> Result<u64, StorageError> {
                collection_remove(store, path, &[account_part(account)])
            }

            fn entries<'a>(store: &'a S, path: &KeyPath) -> Iter<'a, (String, Self)>
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
    fn a_vector_of_collections_pushes_pops_and_iterates() {
        let store = MemStore::new();
        let mut rounds: Vector<Set<String, MemStore>, MemStore> =
            Vector::with_store("rounds", store.clone());
        let (first, mut one) = rounds.push_new().unwrap();
        assert_eq!(first, 0);
        let _ = one.extend(&["ann".to_string(), "bo".to_string()]).unwrap();
        let (_, mut two) = rounds.push_new().unwrap();
        let _ = two.insert(&"cy".into()).unwrap();
        assert_eq!(rounds.len().unwrap(), 2);
        assert!(rounds
            .get(0)
            .unwrap()
            .unwrap()
            .contains(&"bo".into())
            .unwrap());
        assert!(rounds.get(2).unwrap().is_none(), "past the end");
        let sizes: Vec<u64> = rounds
            .iter()
            .map(|r| r.and_then(|set| set.len()))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(sizes, [2, 1]);
        // Popping the last set removes its element and its count.
        assert_eq!(rounds.pop().unwrap(), Some(2));
        assert_eq!(rounds.len().unwrap(), 1);
        let (again, fresh) = rounds.push_new().unwrap();
        assert_eq!(again, 1);
        assert!(
            fresh.is_empty().unwrap(),
            "a position is empty again after a pop"
        );
        assert!(rounds.clear().unwrap() > 0);
        assert!(rounds.is_empty().unwrap());
        assert!(store.is_empty());
    }

    #[test]
    fn a_vector_of_vectors() {
        let grid: Vector<Vector<u8, MemStore>, MemStore> =
            Vector::with_store("grid", MemStore::new());
        let mut grid = grid;
        for row in 0..3u8 {
            let (_, mut cells) = grid.push_new().unwrap();
            let _ = cells.extend(&[row, row + 1]).unwrap();
        }
        let rows: Vec<Vec<u8>> = grid
            .iter()
            .map(|r| r.and_then(|row| row.iter().collect()))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, [vec![0, 1], vec![1, 2], vec![2, 3]]);
    }

    #[test]
    fn a_collection_per_account() {
        use crate::env::{mock, Caller};
        mock::reset();
        let store = MemStore::new();
        let mut inbox: PerUser<Vector<String, MemStore>, MemStore> =
            PerUser::with_store("inbox", store);
        for account in ["bo", "ann"] {
            mock::with_mut(|s| s.caller = Caller::Account(account.into()));
            let mut mine = inbox.get().unwrap();
            let _ = mine.push(&format!("hi {account}")).unwrap();
        }
        assert_eq!(inbox.get_for("ann").unwrap().len().unwrap(), 1);
        assert!(inbox.contains_for("bo").unwrap());
        assert!(!inbox.contains_for("cy").unwrap());
        let accounts: Vec<String> = inbox
            .iter()
            .map(|e| e.map(|(a, _)| a))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(accounts, ["ann", "bo"]);
        // The caller (ann) removes her own inbox only.
        assert_eq!(inbox.remove().unwrap(), 2);
        assert!(!inbox.contains_for("ann").unwrap());
        assert!(inbox.contains_for("bo").unwrap());
        mock::reset();
    }

    #[test]
    fn a_map_of_per_user_and_a_per_user_of_maps() {
        use crate::env::{mock, Caller};
        mock::reset();
        mock::with_mut(|s| s.caller = Caller::Account("ann".into()));
        let store = MemStore::new();
        let rooms: UnorderedMap<String, PerUser<u32, MemStore>, MemStore> =
            UnorderedMap::with_store("rooms", store.clone());
        let mut lobby = rooms.get(&"lobby".into()).unwrap();
        let _ = lobby.set(&3).unwrap();
        assert_eq!(
            rooms.get(&"lobby".into()).unwrap().get_for("ann").unwrap(),
            Some(3)
        );
        let prefs: PerUser<UnorderedMap<String, String, MemStore>, MemStore> =
            PerUser::with_store("prefs", store);
        let mut mine = prefs.get().unwrap();
        let _ = mine.insert(&"theme".into(), &"dark".into()).unwrap();
        assert_eq!(
            prefs
                .get_for("ann")
                .unwrap()
                .get(&"theme".into())
                .unwrap()
                .as_deref(),
            Some("dark")
        );
        mock::reset();
    }

    #[test]
    fn an_emptied_collection_no_longer_counts_as_a_key() {
        let store = MemStore::new();
        let mut lists: UnorderedMap<String, Vector<u32, MemStore>, MemStore> =
            UnorderedMap::with_store("lists", store.clone());
        let tags: UnorderedMap<String, UnorderedMap<String, u32, MemStore>, MemStore> =
            UnorderedMap::with_store("tags", store);
        let mut ann = lists.get(&"ann".into()).unwrap();
        let _ = ann.push(&1).unwrap();
        let _ = ann.pop().unwrap();
        let mut bo = tags.get(&"bo".into()).unwrap();
        let _ = bo.insert(&"n".into(), &0).unwrap();
        let _ = bo.remove(&"n".into()).unwrap();
        // Their length and count rows are 0 now, which is not content.
        assert!(!lists.contains(&"ann".into()).unwrap());
        assert!(lists.is_empty().unwrap());
        assert_eq!(lists.keys().count(), 0);
        assert!(!tags.contains(&"bo".into()).unwrap());
        // A user key named "n" holding 0 is content, not a count.
        let _ = bo.insert(&"n".into(), &0).unwrap();
        assert!(tags.contains(&"bo".into()).unwrap());
        assert_eq!(tags.keys().count(), 1);
        // Removing the emptied key still clears its leftover rows.
        let _ = lists.get(&"cy".into()).unwrap().push(&2).unwrap();
        let keys: Vec<String> = lists.keys().collect::<Result<_, _>>().unwrap();
        assert_eq!(keys, ["cy"]);
        assert_eq!(lists.remove(&"ann".into()).unwrap(), 1);
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
