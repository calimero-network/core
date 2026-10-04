//! What generic code must bound a map's key by to use it.
//!
//! Iteration checks each entry's key against the id it is stored at, from the
//! same `AsRef<[u8]>` bytes `insert` derives the id from. So a helper that
//! iterates, or a type deriving `Debug`, `PartialEq`, `Ord` or `Serialize` over
//! a map, bounds `K` by those bytes; `len` asks only borsh, and `get` only that
//! the borrowed key be bytes. Nothing here runs: every item only has to
//! compile, at each key type an app commonly uses.

use core::borrow::Borrow;
use core::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::AccountId;
use calimero_storage::address::Id;
use calimero_storage::collections::{Indexed, IndexedMap, SortedMap, StorageKey, UnorderedMap};
use serde::Serialize;

/// A newtype key with no byte view: it can be read, never inserted.
#[derive(BorshSerialize, BorshDeserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct PostId(String);

/// The same newtype with the byte view `insert` needs.
#[derive(BorshSerialize, BorshDeserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct PostKey(String);

impl AsRef<[u8]> for PostKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Borrow<str> for PostKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A value an `IndexedMap` can hold.
#[derive(BorshSerialize, BorshDeserialize, Debug, PartialEq, Serialize)]
struct Post(String);

impl Indexed for Post {
    const INDEXES: &'static [&'static str] = &[];

    fn index_keys(&self, _index: usize, _out: &mut Vec<Vec<u8>>) {}
}

fn unordered_len<K, V>(map: &UnorderedMap<K, V>) -> usize
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    map.len().unwrap_or_default()
}

fn unordered_entries<K, V>(map: &UnorderedMap<K, V>) -> Vec<(K, V)>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize,
{
    map.entries().map(Iterator::collect).unwrap_or_default()
}

fn unordered_get_str<K, V>(map: &UnorderedMap<K, V>, key: &str) -> bool
where
    K: BorshSerialize + BorshDeserialize + Borrow<str>,
    V: BorshSerialize + BorshDeserialize,
{
    map.get(key).is_ok_and(|value| value.is_some())
}

fn unordered_get<K, V>(map: &UnorderedMap<K, V>, key: &K) -> bool
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq,
    V: BorshSerialize + BorshDeserialize,
{
    map.get(key).is_ok_and(|value| value.is_some())
}

fn unordered_insert<K, V>(map: &mut UnorderedMap<K, V>, key: K, value: V) -> bool
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
{
    map.insert(key, value).is_ok()
}

fn sorted_len<K, V>(map: &SortedMap<K, V>) -> usize
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    map.len().unwrap_or_default()
}

fn sorted_reads<K, V>(map: &SortedMap<K, V>) -> (Vec<(K, V)>, Vec<K>, Vec<V>)
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Ord,
    V: BorshSerialize + BorshDeserialize,
{
    (
        map.entries().map(Iterator::collect).unwrap_or_default(),
        map.keys().map(Iterator::collect).unwrap_or_default(),
        map.values().map(Iterator::collect).unwrap_or_default(),
    )
}

fn sorted_get_str<K, V>(map: &SortedMap<K, V>, key: &str) -> bool
where
    K: BorshSerialize + BorshDeserialize + Borrow<str>,
    V: BorshSerialize + BorshDeserialize,
{
    map.get(key).is_ok_and(|value| value.is_some())
}

fn sorted_insert<K, V>(map: &mut SortedMap<K, V>, key: K, value: V) -> bool
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
{
    map.insert(key, value).is_ok()
}

fn indexed_reads<K, V>(map: &IndexedMap<K, V>) -> (usize, Vec<(K, V)>)
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize,
{
    (
        map.len().unwrap_or_default(),
        map.entries().map(Iterator::collect).unwrap_or_default(),
    )
}

fn indexed_get_str<K, V>(map: &IndexedMap<K, V>, key: &str) -> bool
where
    K: BorshSerialize + BorshDeserialize + Borrow<str>,
    V: BorshSerialize + BorshDeserialize,
{
    map.get(key).is_ok_and(|value| value.is_some())
}

fn indexed_insert<K, V>(map: &mut IndexedMap<K, V>, key: K, value: V) -> bool
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + Indexed + 'static,
{
    map.insert(key, value).is_ok()
}

/// App state generic over its key, deriving what a state struct commonly does.
#[derive(Debug, PartialEq, Serialize)]
struct Board<K, V>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize,
{
    posts: UnorderedMap<K, V>,
}

/// The same over an indexed map, which has no `PartialEq`.
#[derive(Debug, Serialize)]
struct Catalog<K, V>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize,
{
    posts: IndexedMap<K, V>,
}

/// The same over a sorted map, which orders by `K`.
#[derive(Debug, PartialEq, Serialize)]
struct Ranked<K, V>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Ord,
    V: BorshSerialize + BorshDeserialize,
{
    posts: SortedMap<K, V>,
}

/// A generic type deriving `Eq` and `Ord` over a map: the whole map compares.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Ordered<K, V>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize,
{
    posts: UnorderedMap<K, V>,
}

fn is_debug_eq<T: Debug + PartialEq>() {}

fn is_debug_serialize<T: Debug + Serialize>() {}

fn is_ord<T: Ord>() {}

/// Every count at key type `K`, whether or not `K` can be inserted.
macro_rules! counts_at {
    ($key:ty) => {{
        let _: fn(&UnorderedMap<$key, u8>) -> usize = unordered_len::<$key, u8>;
        let _: fn(&SortedMap<$key, u8>) -> usize = sorted_len::<$key, u8>;
    }};
}

/// Every read at key type `K`, which needs the key bytes to check each entry.
macro_rules! reads_at {
    ($key:ty) => {{
        counts_at!($key);
        let _: fn(&UnorderedMap<$key, u8>) -> Vec<($key, u8)> = unordered_entries::<$key, u8>;
        let _ = sorted_reads::<$key, u8>;
        let _ = indexed_reads::<$key, Post>;
        is_debug_eq::<UnorderedMap<$key, u8>>();
        is_debug_eq::<SortedMap<$key, u8>>();
        is_debug_eq::<Board<$key, u8>>();
        is_debug_eq::<Ranked<$key, u8>>();
        is_ord::<UnorderedMap<$key, u8>>();
        is_ord::<SortedMap<$key, u8>>();
        is_ord::<Ordered<$key, u8>>();
    }};
}

/// Every `Serialize` at key type `K`, which a key without `Serialize` lacks.
macro_rules! serializes_at {
    ($key:ty) => {{
        is_debug_serialize::<UnorderedMap<$key, u8>>();
        is_debug_serialize::<SortedMap<$key, u8>>();
        is_debug_serialize::<IndexedMap<$key, Post>>();
        is_debug_serialize::<Board<$key, u8>>();
        is_debug_serialize::<Catalog<$key, Post>>();
        is_debug_serialize::<Ranked<$key, u8>>();
    }};
}

/// Every keyed call at key type `K`, which needs the key bytes.
macro_rules! keyed_at {
    ($key:ty) => {{
        reads_at!($key);
        serializes_at!($key);
        let _ = unordered_get::<$key, u8>;
        let _ = unordered_insert::<$key, u8>;
        let _ = sorted_insert::<$key, u8>;
        let _ = indexed_insert::<$key, Post>;
    }};
}

#[test]
fn keys_with_bytes_read_and_write() {
    keyed_at!(String);
    keyed_at!(Vec<u8>);
    keyed_at!([u8; 32]);
    keyed_at!(AccountId);
    keyed_at!(PostKey);
}

#[test]
fn keys_borrowed_as_str_read() {
    let _ = unordered_get_str::<String, u8>;
    let _ = unordered_get_str::<PostKey, u8>;
    let _ = sorted_get_str::<String, u8>;
    let _ = indexed_get_str::<String, Post>;
}

/// `Id`, `u64` and a newtype without a byte view were never insertable (the
/// slot id derives from `AsRef<[u8]>`), so a map of them holds only entries a
/// peer filed, which iteration cannot check. It still counts them.
#[test]
fn keys_without_bytes_still_count() {
    counts_at!(Id);
    counts_at!(u64);
    counts_at!(PostId);
}
