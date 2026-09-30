//! A map with node-local secondary indexes over its values.
//!
//! [`IndexedMap<K, V>`] stores exactly what an [`UnorderedMap<K, V>`] stores —
//! the same entries, under the same ids, with the same merge — and additionally
//! keeps one lookup table per index the value type declares, so a read like
//! "the open issues", "the notes of deal 7" or "the newest twenty posts" is a
//! seek instead of a scan over every entry.
//!
//! # Why this exists
//!
//! Almost every list method an app writes has the same shape: read the whole
//! map, filter by a field, join by a foreign key, sort, count. On an
//! `UnorderedMap` each of those costs `O(n)` in the WHOLE collection even when
//! the answer is twenty rows, and it keeps growing until a read no longer fits
//! the execution budget. An index makes the cost `O(log n + k)` in what was
//! asked for.
//!
//! # Declaring indexes
//!
//! The value type lists its indexes by implementing [`Indexed`] — normally with
//! `#[derive(Indexed)]` from the SDK:
//!
//! ```ignore
//! #[derive(BorshSerialize, BorshDeserialize, Indexed)]
//! #[index(status_created(status, created_at))]
//! pub struct Issue {
//!     #[index] pub status: LwwRegister<String>,
//!     #[index] pub assignee: LwwRegister<Option<String>>,
//!     #[index] pub labels: LwwRegister<Vec<String>>,   // one entry per label
//!     pub created_at: LwwRegister<u64>,
//!     pub title: LwwRegister<String>,
//! }
//!
//! let open = issues.query("status").eq("open").entries()?;
//! let newest_open = issues
//!     .query("status_created").eq("open").desc().limit(20).entries()?;
//! let open_count = issues.query("status").eq("open").count()?;
//! ```
//!
//! An index value is anything implementing [`IndexValue`]: strings, integers,
//! `bool`, byte arrays, and — through `LwwRegister`, `Option`, `Vec` and tuples —
//! the usual wrappers. `Option::None` leaves the entry out of that index; a `Vec`
//! puts the entry in once per element; a tuple is a compound key whose leading
//! components can be matched on their own, which is how "status = open, newest
//! first" is one seek.
//!
//! # What is and is not synced
//!
//! Only the entries. The indexes are a **node-local, derived** view kept in the
//! same non-synced keyspace [`SortedMap`](super::SortedMap) uses, so there is
//! nothing extra on the wire and nothing extra in the root hash: the collection
//! reports [`CrdtType::UnorderedMap`], and two nodes holding the same entries —
//! one reading them as an `UnorderedMap`, the other as an `IndexedMap` — agree on
//! every hash. The borsh layout is the `UnorderedMap`'s own, so **switching a
//! field from `UnorderedMap` to `IndexedMap` needs no data migration**: the
//! indexes are built on the first query.
//!
//! # Keeping the indexes honest
//!
//! Correctness never depends on every write going through this type. A validity
//! marker records the collection's `full_hash` (plus a fingerprint of the index
//! declarations) at the moment the indexes were last consistent. A query first
//! compares it with the current `full_hash`; on a mismatch it rebuilds.
//!
//! * `insert` / `update` / `remove` / `clear` maintain the indexes
//!   incrementally and re-stamp the marker — but only if it was current before
//!   the write, so a write never certifies an index that was already stale.
//! * A remote sync, a write through any other path, or a changed index
//!   declaration moves the `full_hash` (or the fingerprint) without touching the
//!   indexes, so the next query sees the mismatch and rebuilds.
//!
//! # What it costs
//!
//! * each write pays one index write per changed index key, plus a marker
//!   read/write;
//! * the node stores one extra row per (entry, index value);
//! * **the first query after a remote change is `O(n)`**: the rebuild reads every
//!   entry to re-derive its keys, though it only writes the keys that changed.
//!
//! That last line is the price of maintaining the index from inside the guest,
//! where a sync applied host-side is invisible. It is the same trade
//! `SortedMap` makes. Where the adaptor backs no ordered keyspace (e.g.
//! `PrivateStorage`) every query falls back to an in-memory scan and sort —
//! correct, and no faster than doing it by hand.
//!
//! # Key encoding
//!
//! Every index key is a sequence of components, each the value's
//! order-preserving bytes (integers big-endian, signed ones offset-binary) with
//! `0x00` escaped to `0x00 0xFF`, closed by the terminator `0x00 0x01`. The
//! escape makes the encoding order-preserving and the terminator makes every
//! component self-delimiting, so:
//!
//! * byte order is value order, component by component — ranges are seeks;
//! * no encoded value is a prefix of another — `eq("ab")` never matches `"abc"`;
//! * a compound key's leading components are a prefix of the whole — `eq` on the
//!   first component of `(status, created_at)` selects exactly that status,
//!   already ordered by `created_at`.
//!
//! The entry's 32-byte id follows the components, which makes every row unique
//! and orders equal values by id.

use core::borrow::Borrow;
use core::fmt;
use core::ops::{Bound, RangeBounds};
use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, MergeStrategy, Mergeable, StorageStrategy};
use super::{LwwRegister, StorageKey, StoreError, UnorderedMap, ValueRef};
use crate::address::Id;
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::{MainStorage, StorageAdaptor};

/// Domain separator for an index's keyspace id, so it can never equal an entry
/// id or a collection id derived from the same parent.
const DOMAIN_INDEX_SPACE: &[u8] = b"__calimero_indexed_map_index__";

/// Domain separator for the id the validity marker is stored under.
const DOMAIN_MARKER: &[u8] = b"__calimero_indexed_map_marker__";

/// Version of the key encoding, folded into the marker fingerprint: changing
/// the encoding must invalidate every index built under the old one.
const ENCODING_VERSION: u8 = 1;

/// Escape byte: a literal `0x00` inside a component is written `0x00 0xFF`.
const ESCAPED_ZERO: [u8; 2] = [0x00, 0xFF];

/// Closes every component. Sorts below [`ESCAPED_ZERO`], which is what makes a
/// value sort before every longer value it is a prefix of.
const TERMINATOR: [u8; 2] = [0x00, 0x01];

/// Rows scanned per host call when an unbounded read has to walk an index. The
/// host caps a single scan; walking in pages keeps a large index readable.
const SCAN_PAGE: usize = 10_000;

/// A value that can be a key in an [`IndexedMap`] index.
///
/// Implementations push **complete encoded keys** — see the
/// [module docs](self) for the format — onto `out`: none to leave the entry out
/// of the index, one for an ordinary value, several for a multi-valued field.
/// Build a scalar with [`push_component`]; the provided impls cover the common
/// types, and a newtype can delegate to its inner value.
pub trait IndexValue {
    /// Push this value's encoded keys onto `out`.
    fn encode_index(&self, out: &mut Vec<Vec<u8>>);
}

/// The indexes a value type declares, and the keys one value contributes.
///
/// Normally derived: `#[derive(Indexed)]` from the SDK generates this from
/// `#[index]` field attributes and struct-level
/// `#[index(name(a, b))]` compound declarations.
pub trait Indexed {
    /// The index names, in declaration order. A query names an index by one of
    /// these; the position is what [`index_keys`](Self::index_keys) receives.
    const INDEXES: &'static [&'static str];

    /// Push the encoded keys this value contributes to the index at position
    /// `index` in [`INDEXES`](Self::INDEXES).
    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>);
}

/// Append one encoded component — `bytes` escaped and terminated — to `buf`.
///
/// The building block for [`IndexValue`] impls: pass a value's
/// order-preserving bytes (big-endian for integers).
pub fn push_component(buf: &mut Vec<u8>, bytes: &[u8]) {
    for &byte in bytes {
        if byte == 0 {
            buf.extend_from_slice(&ESCAPED_ZERO);
        } else {
            buf.push(byte);
        }
    }
    buf.extend_from_slice(&TERMINATOR);
}

/// One single-component key built from `bytes`.
fn component(bytes: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(bytes.len() + TERMINATOR.len());
    push_component(&mut buf, bytes);
    buf
}

impl IndexValue for str {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        out.push(component(self.as_bytes()));
    }
}

impl IndexValue for String {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        self.as_str().encode_index(out);
    }
}

impl IndexValue for bool {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        out.push(component(&[u8::from(*self)]));
    }
}

impl<const N: usize> IndexValue for [u8; N] {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        out.push(component(self));
    }
}

macro_rules! unsigned_index_value {
    ($($ty:ty),*) => {$(
        impl IndexValue for $ty {
            fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
                out.push(component(&self.to_be_bytes()));
            }
        }
    )*};
}

unsigned_index_value!(u8, u16, u32, u64, u128);

// Offset-binary: flipping the sign bit maps the signed range onto the unsigned
// one in order, so the big-endian bytes compare like the values.
macro_rules! signed_index_value {
    ($($ty:ty => $unsigned:ty),*) => {$(
        impl IndexValue for $ty {
            fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
                let flipped = (*self as $unsigned) ^ (1 << (<$unsigned>::BITS - 1));
                out.push(component(&flipped.to_be_bytes()));
            }
        }
    )*};
}

signed_index_value!(i8 => u8, i16 => u16, i32 => u32, i64 => u64, i128 => u128);

impl<T: IndexValue + ?Sized> IndexValue for &T {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        (**self).encode_index(out);
    }
}

/// `None` contributes no key: the entry is simply absent from the index.
impl<T: IndexValue> IndexValue for Option<T> {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        if let Some(value) = self {
            value.encode_index(out);
        }
    }
}

/// One key per element — a multi-valued field such as a tag list. Duplicate
/// elements index the entry once.
impl<T: IndexValue> IndexValue for Vec<T> {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        for element in self {
            element.encode_index(out);
        }
    }
}

impl<T: IndexValue> IndexValue for LwwRegister<T> {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        self.get().encode_index(out);
    }
}

/// Every key of `head` followed by every key of `tail`: the cartesian product,
/// so a compound index over a multi-valued field still indexes each element.
fn concat_keys(head: &[Vec<u8>], tail: &[Vec<u8>], out: &mut Vec<Vec<u8>>) {
    for h in head {
        for t in tail {
            let mut key = Vec::with_capacity(h.len() + t.len());
            key.extend_from_slice(h);
            key.extend_from_slice(t);
            out.push(key);
        }
    }
}

impl<A: IndexValue, B: IndexValue> IndexValue for (A, B) {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        self.0.encode_index(&mut a);
        self.1.encode_index(&mut b);
        concat_keys(&a, &b, out);
    }
}

impl<A: IndexValue, B: IndexValue, C: IndexValue> IndexValue for (A, B, C) {
    fn encode_index(&self, out: &mut Vec<Vec<u8>>) {
        let mut ab = Vec::new();
        (&self.0, &self.1).encode_index(&mut ab);
        let mut c = Vec::new();
        self.2.encode_index(&mut c);
        concat_keys(&ab, &c, out);
    }
}

/// The single key `value` encodes to — a query operand must name exactly one.
fn single_key<Q: IndexValue + ?Sized>(value: &Q) -> Result<Vec<u8>, String> {
    let mut keys = Vec::new();
    value.encode_index(&mut keys);
    match <[Vec<u8>; 1]>::try_from(keys) {
        Ok([key]) => Ok(key),
        Err(keys) => Err(format!(
            "an index query value must encode to exactly one key, got {}",
            keys.len()
        )),
    }
}

fn invalid(message: String) -> StoreError {
    StoreError::StorageError(StorageError::InvalidData(message))
}

/// `key` without its final terminator — the form a range bound compares on:
/// every row whose value equals `key` sorts strictly after it.
fn open_form(key: &[u8]) -> &[u8] {
    &key[..key.len() - TERMINATOR.len()]
}

/// The smallest byte string sorting after every row that starts with `key`
/// (a complete encoded key, so it ends in the terminator `0x00 0x01`): the same
/// bytes with the final `0x01` raised to `0x02`, which no escape can produce.
fn past(key: &[u8]) -> Vec<u8> {
    let mut bound = key.to_vec();
    if let Some(last) = bound.last_mut() {
        *last = TERMINATOR[1] + 1;
    }
    bound
}

/// A row's full key: the encoded index key followed by the entry id.
fn row_key(key: &[u8], entry: Id) -> Vec<u8> {
    let mut row = Vec::with_capacity(key.len() + 32);
    row.extend_from_slice(key);
    row.extend_from_slice(entry.as_bytes());
    row
}

/// The entry id a row ends with.
fn row_entry(row: &[u8]) -> Option<Id> {
    let tail = row.len().checked_sub(32)?;
    let id: [u8; 32] = row[tail..].try_into().ok()?;
    Some(Id::new(id))
}

/// Whether `row` falls within `[lo, hi)` as a [`Query`] computes them.
fn within(row: &[u8], lo: &Bound<Vec<u8>>, hi: &Bound<Vec<u8>>) -> bool {
    let above = match lo {
        Bound::Included(lo) => row >= lo.as_slice(),
        Bound::Excluded(lo) => row > lo.as_slice(),
        Bound::Unbounded => true,
    };
    let below = match hi {
        Bound::Included(hi) => row <= hi.as_slice(),
        Bound::Excluded(hi) => row < hi.as_slice(),
        Bound::Unbounded => true,
    };
    above && below
}

/// A map keyed by `K` whose values declare secondary indexes.
///
/// See the [module documentation](self).
#[derive(BorshSerialize, BorshDeserialize)]
pub struct IndexedMap<K, V, S: StorageAdaptor = MainStorage> {
    /// The entries. The indexes are node-local and are not part of the stored
    /// value, so an `IndexedMap` serializes byte for byte as this map does.
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: UnorderedMap<K, V, S>,
}

impl<K, V, S> IndexedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new map with a random ID.
    ///
    /// Right for top-level `#[app::state]` fields (the macro reassigns a
    /// deterministic id after `init()`) and for maps nested in other values.
    pub fn new() -> Self {
        Self {
            inner: UnorderedMap::new(),
        }
    }

    /// Create a new map with a deterministic ID derived from `field_name` —
    /// the same id an `UnorderedMap` of that name gets.
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self {
            inner: UnorderedMap::new_with_field_name(field_name),
        }
    }

    /// Reassign the map's ID deterministically from `field_name`, migrating its
    /// entries. Called by the `#[app::state]` macro after `init()`.
    pub fn reassign_deterministic_id(&mut self, field_name: &str)
    where
        K: StorageKey,
        V: 'static,
    {
        self.inner.reassign_deterministic_id(field_name);
    }
}

impl<K, V, S> IndexedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + Indexed + 'static,
    S: StorageAdaptor,
{
    /// Insert `value` at `key`, returning the previous value if there was one.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn insert(&mut self, key: K, value: V) -> Result<Option<V>, StoreError> {
        // Entries inherit the collection's storage domain, as `UnorderedMap::insert`.
        let inherited = self.inner.element().metadata.storage_type.clone();
        self.insert_with_storage_type(key, value, inherited)
    }

    /// [`insert`](Self::insert) with the entry stamped `storage_type`: how an
    /// `Authored` or `Frozen` policy writes through this map.
    pub(crate) fn insert_with_storage_type(
        &mut self,
        key: K,
        value: V,
        storage_type: StorageType,
    ) -> Result<Option<V>, StoreError> {
        super::rekey::register_rekey::<Self>();

        let maintain = self.index_current();
        let entry = self.inner.entry_id(&key);
        let new_keys = maintain.then(|| keys_of(&value));
        let previous = self
            .inner
            .insert_with_storage_type(key, value, storage_type, None)?;
        if let Some(new_keys) = new_keys {
            let old_keys = previous.as_ref().map(keys_of);
            self.write_diff(entry, old_keys.as_deref(), Some(&new_keys));
        }
        Ok(previous)
    }

    /// Mutate the value at `key` in place with `f`, keeping the indexes in step
    /// with whatever `f` changed. Returns `None` if `key` is absent.
    ///
    /// This is the way to change an indexed field of a stored value. There is
    /// deliberately no `get_mut`: a mutable guard would let an indexed field
    /// change behind the indexes' back — still correct, because the marker
    /// catches it, but at the price of a full rebuild on the next query.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn update<Q, R>(
        &mut self,
        key: &Q,
        f: impl FnOnce(&mut V) -> R,
    ) -> Result<Option<R>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let maintain = self.index_current();
        let entry = self.inner.entry_id(key);
        let Some(mut guard) = self.inner.get_mut(key)? else {
            return Ok(None);
        };
        let old_keys = maintain.then(|| keys_of(&*guard));
        let result = f(&mut guard);
        let new_keys = maintain.then(|| keys_of(&*guard));
        // The guard writes the value back when it drops; the marker must be
        // stamped against the hash that write produces, so drop it first.
        drop(guard);
        if let (Some(old_keys), Some(new_keys)) = (old_keys, new_keys) {
            self.write_diff(entry, Some(&old_keys), Some(&new_keys));
        }
        Ok(Some(result))
    }

    /// Remove `key`, returning its value if it was present.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn remove<Q>(&mut self, key: &Q) -> Result<Option<V>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let maintain = self.index_current();
        let entry = self.inner.entry_id(key);
        let removed = self.inner.remove(key)?;
        if maintain {
            if let Some(value) = &removed {
                self.write_diff(entry, Some(&keys_of(value)), None);
            }
        }
        Ok(removed)
    }

    /// Remove `owner`'s entry at `key`, in a map where every owner keeps its
    /// own entry per key, returning its value.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub(crate) fn remove_by_owner(
        &mut self,
        owner: &AccountId,
        key: &K,
    ) -> Result<Option<V>, StoreError> {
        let maintain = self.index_current();
        let entry = super::owned_keyed_entry_id(self.inner.slot_id(key), owner);
        let removed = self.inner.remove_by_owner(owner, key)?;
        if maintain {
            if let Some(value) = &removed {
                self.write_diff(entry, Some(&keys_of(value)), None);
            }
        }
        Ok(removed)
    }

    /// `owner`'s value at `key`. See [`UnorderedMap::get_by_owner`].
    pub(crate) fn get_by_owner(&self, owner: &AccountId, key: &K) -> Result<Option<V>, StoreError> {
        self.inner.get_by_owner(owner, key)
    }

    /// See [`UnorderedMap::bind_slot_keys`].
    pub(crate) fn bind_slot_keys(&mut self) {
        self.inner.bind_slot_keys();
    }

    /// Every owned entry with its owner. See [`UnorderedMap::owned_entries`].
    pub(crate) fn owned_entries(
        &self,
        owner: Option<&AccountId>,
    ) -> Result<Vec<(AccountId, K, V)>, StoreError> {
        self.inner.owned_entries(owner)
    }

    /// Every owner's value at `key`. See [`UnorderedMap::entries_at`].
    pub(crate) fn entries_at(&self, key: &K) -> Result<Vec<(AccountId, V)>, StoreError> {
        self.inner.entries_at(key)
    }

    /// Remove every entry.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.inner.clear()?;
        if S::index_supported() {
            let mut persisted = true;
            for index in 0..V::INDEXES.len() {
                persisted &= S::index_clear(self.space(index));
            }
            if persisted {
                self.stamp();
            }
        }
        Ok(())
    }

    /// Start a query against the index named `index` (one of
    /// [`Indexed::INDEXES`]). With no further conditions it selects every
    /// entry the index holds, in index order.
    ///
    /// An unknown name is reported when the query runs.
    pub fn query(&self, index: &str) -> Query<'_, K, V, S> {
        let position = V::INDEXES.iter().position(|name| *name == index);
        Query {
            map: self,
            index: position,
            prefix: Vec::new(),
            range: None,
            desc: false,
            skip: 0,
            limit: None,
            error: position
                .is_none()
                .then(|| format!("no index named `{index}` on this map")),
        }
    }

    /// Whether the indexes can be trusted as they stand: the adaptor backs an
    /// index and the marker matches the current hash and declarations. Read
    /// BEFORE a write, it decides whether that write may maintain the indexes
    /// and re-stamp — a write must never certify an index that was stale.
    fn index_current(&self) -> bool {
        S::index_supported()
            && S::index_meta_get(self.marker_id()).as_deref() == Some(&self.marker()[..])
    }

    /// Make the indexes usable for a read, rebuilding them if the marker is
    /// stale. `false` means the caller must answer by scanning instead: the
    /// adaptor backs no index, or the rebuild's writes did not all land.
    ///
    /// The second case is not hypothetical. An execution whose node-local
    /// writes are suppressed (the node's migration check runs that way) drops
    /// every row a rebuild writes; reading the index anyway would answer from
    /// an index that was never built, and report nothing.
    fn ensure_index(&self) -> Result<bool, StoreError> {
        if !S::index_supported() {
            return Ok(false);
        }
        if self.index_current() {
            return Ok(true);
        }
        self.rebuild()
    }

    /// Reconcile every index with the stored entries, writing only the rows
    /// that differ, then stamp the marker. Returns whether every write landed;
    /// if one did not, the marker stays stale and the next query retries.
    fn rebuild(&self) -> Result<bool, StoreError> {
        let mut desired: Vec<BTreeSet<Vec<u8>>> = vec![BTreeSet::new(); V::INDEXES.len()];
        for (entry, _key, value) in self.inner.entries_with_ids()? {
            for (index, keys) in keys_of(&value).into_iter().enumerate() {
                desired[index].extend(keys.iter().map(|k| row_key(k, entry)));
            }
        }

        let mut persisted = true;
        for (index, desired) in desired.iter().enumerate() {
            let space = self.space(index);
            let existing: BTreeSet<Vec<u8>> =
                scan::<S>(space, &Bound::Unbounded, &Bound::Unbounded)
                    .into_iter()
                    .map(|(row, _)| row)
                    .collect();
            for row in existing.difference(desired) {
                persisted &= S::index_remove(space, row);
            }
            for row in desired.difference(&existing) {
                persisted &= row_entry(row).is_some_and(|entry| S::index_put(space, row, entry));
            }
        }

        if persisted {
            self.stamp();
        }
        Ok(persisted)
    }

    /// Apply one entry's change to every index and re-stamp the marker. The
    /// caller has established the marker was current before the write.
    fn write_diff(
        &self,
        entry: Id,
        old: Option<&[BTreeSet<Vec<u8>>]>,
        new: Option<&[BTreeSet<Vec<u8>>]>,
    ) {
        let empty = BTreeSet::new();
        let mut persisted = true;
        for index in 0..V::INDEXES.len() {
            let space = self.space(index);
            let old = old.map_or(&empty, |keys| &keys[index]);
            let new = new.map_or(&empty, |keys| &keys[index]);
            for key in old.difference(new) {
                persisted &= S::index_remove(space, &row_key(key, entry));
            }
            for key in new.difference(old) {
                persisted &= S::index_put(space, &row_key(key, entry), entry);
            }
        }
        if persisted {
            self.stamp();
        }
    }

    /// Record that the indexes are consistent with the current entries.
    fn stamp(&self) {
        let _ = S::index_meta_put(self.marker_id(), &self.marker());
    }

    /// What the marker must hold for the indexes to be trusted: the
    /// collection's current `full_hash`, followed by a fingerprint of the index
    /// declarations and the key encoding.
    fn marker(&self) -> Vec<u8> {
        let full_hash = <Index<S>>::get_hashes_for(self.id())
            .ok()
            .flatten()
            .map_or([0; 32], |(full, _own)| full);

        let mut fingerprint = Sha256::new();
        fingerprint.update([ENCODING_VERSION]);
        for name in V::INDEXES {
            fingerprint.update((name.len() as u64).to_le_bytes());
            fingerprint.update(name.as_bytes());
        }

        let mut marker = full_hash.to_vec();
        marker.extend_from_slice(&fingerprint.finalize());
        marker
    }

    /// The id the validity marker is stored under.
    fn marker_id(&self) -> Id {
        let mut hasher = Sha256::new();
        hasher.update(self.id().as_bytes());
        hasher.update(DOMAIN_MARKER);
        Id::new(hasher.finalize().into())
    }

    /// The keyspace of the index at `index`, derived from its NAME so that
    /// reordering the declarations does not reshuffle the rows.
    fn space(&self, index: usize) -> Id {
        let mut hasher = Sha256::new();
        hasher.update(self.id().as_bytes());
        hasher.update(DOMAIN_INDEX_SPACE);
        hasher.update(V::INDEXES[index].as_bytes());
        Id::new(hasher.finalize().into())
    }
}

impl<K, V, S> IndexedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Every `(key, value)` entry, in storage order — `O(n)`. The point of an
    /// `IndexedMap` is to not do this: prefer a [`query`](Self::query).
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)> + '_, StoreError> {
        self.inner.entries()
    }

    /// The value at `key`, if any.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn get<Q>(&self, key: &Q) -> Result<Option<ValueRef<V>>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        self.inner.get(key)
    }

    /// Whether `key` is present.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn contains<Q>(&self, key: &Q) -> Result<bool, StoreError>
    where
        K: Borrow<Q> + PartialEq,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        self.inner.contains(key)
    }

    /// The number of entries.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.inner.len()
    }

    /// Whether the map is empty.
    ///
    /// # Errors
    ///
    /// Returns any underlying storage error.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.inner.is_empty()
    }
}

/// Each index's keys for `value`, deduplicated.
fn keys_of<V: Indexed>(value: &V) -> Vec<BTreeSet<Vec<u8>>> {
    (0..V::INDEXES.len())
        .map(|index| {
            let mut keys = Vec::new();
            value.index_keys(index, &mut keys);
            keys.into_iter().collect()
        })
        .collect()
}

/// Every `(row, entry)` of `space` within `[lo, hi)`, ascending, walked in
/// pages so no single host scan has to return an unbounded range.
fn scan<S: StorageAdaptor>(
    space: Id,
    lo: &Bound<Vec<u8>>,
    hi: &Bound<Vec<u8>>,
) -> Vec<(Vec<u8>, Id)> {
    let mut rows = Vec::new();
    let mut start = lo.clone();
    loop {
        let page = S::index_range(space, start, hi.clone(), 0, Some(SCAN_PAGE));
        let full = page.len() == SCAN_PAGE;
        let Some((last, _)) = page.last() else {
            break;
        };
        start = Bound::Excluded(last.clone());
        rows.extend(page);
        if !full {
            break;
        }
    }
    rows
}

/// A lower and an upper bound on row bytes.
type RowBounds = (Bound<Vec<u8>>, Bound<Vec<u8>>);

/// A read against one index of an [`IndexedMap`], built with chained
/// conditions and run by [`entries`](Self::entries), [`keys`](Self::keys),
/// [`first`](Self::first) or [`count`](Self::count).
///
/// Conditions apply to the index key's components in order: each
/// [`eq`](Self::eq) pins the next component, and one optional
/// [`range`](Self::range) bounds the component after those. Rows come back in
/// index order — ascending, or descending after [`desc`](Self::desc) — with
/// [`skip`](Self::skip) and [`limit`](Self::limit) applied to that order.
///
/// A malformed condition (an unknown index, a value that encodes to more than
/// one key, `eq` after `range`) does not panic mid-chain: it is recorded and
/// reported when the query runs.
pub struct Query<'a, K, V, S: StorageAdaptor = MainStorage> {
    map: &'a IndexedMap<K, V, S>,
    /// The index position.
    index: Option<usize>,
    /// The encoded components pinned by `eq`.
    prefix: Vec<u8>,
    /// Bounds on the component after the prefix, each a single encoded key.
    range: Option<RowBounds>,
    desc: bool,
    skip: usize,
    limit: Option<usize>,
    /// The first malformed condition, reported when the query runs.
    error: Option<String>,
}

impl<K, V, S: StorageAdaptor> fmt::Debug for Query<'_, K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("index", &self.index)
            .field("desc", &self.desc)
            .field("skip", &self.skip)
            .field("limit", &self.limit)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<K, V, S> Query<'_, K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + Indexed + 'static,
    S: StorageAdaptor,
{
    /// Pin the next component of the index key to `value`. On a compound index
    /// the components left free order the result.
    #[must_use]
    pub fn eq<Q: IndexValue + ?Sized>(mut self, value: &Q) -> Self {
        if self.range.is_some() {
            self.fail("`eq` cannot follow `range` in one query".to_owned());
            return self;
        }
        match single_key(value) {
            Ok(key) => self.prefix.extend_from_slice(&key),
            Err(error) => self.fail(error),
        }
        self
    }

    /// Bound the component after the pinned ones to `range`.
    #[must_use]
    pub fn range<Q: IndexValue>(mut self, range: impl RangeBounds<Q>) -> Self {
        if self.range.is_some() {
            self.fail("a query takes at most one `range`".to_owned());
            return self;
        }
        let encode = |bound: Bound<&Q>| -> Result<Bound<Vec<u8>>, String> {
            Ok(match bound {
                Bound::Included(value) => Bound::Included(single_key(value)?),
                Bound::Excluded(value) => Bound::Excluded(single_key(value)?),
                Bound::Unbounded => Bound::Unbounded,
            })
        };
        match (encode(range.start_bound()), encode(range.end_bound())) {
            (Ok(lower), Ok(upper)) => self.range = Some((lower, upper)),
            (Err(error), _) | (_, Err(error)) => self.fail(error),
        }
        self
    }

    /// Return rows in descending index order.
    #[must_use]
    pub fn desc(mut self) -> Self {
        self.desc = true;
        self
    }

    /// Skip the first `n` rows of the result order.
    #[must_use]
    pub fn skip(mut self, n: usize) -> Self {
        self.skip = n;
        self
    }

    /// Return at most `n` rows.
    #[must_use]
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    /// Run the query and load the matching entries — `O(log n + skip + limit)`
    /// on an index-backed adaptor. An entry with several keys in range of a
    /// multi-valued index appears once per key, as it has one row per key.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed query, or any underlying storage error.
    pub fn entries(self) -> Result<Vec<(K, V)>, StoreError> {
        let hits = self.hits()?;
        let mut out = Vec::with_capacity(hits.len());
        for entry in hits {
            if let Some(pair) = self.map.inner.get_by_id(entry)? {
                out.push(pair);
            }
        }
        Ok(out)
    }

    /// Run the query and return only the keys of the matching entries.
    ///
    /// # Errors
    ///
    /// As [`entries`](Self::entries).
    pub fn keys(self) -> Result<Vec<K>, StoreError> {
        Ok(self.entries()?.into_iter().map(|(key, _)| key).collect())
    }

    /// The first matching entry in result order, if any.
    ///
    /// # Errors
    ///
    /// As [`entries`](Self::entries).
    pub fn first(self) -> Result<Option<(K, V)>, StoreError> {
        Ok(self.limit(1).entries()?.into_iter().next())
    }

    /// How many rows match, ignoring `skip` and `limit` — without loading a
    /// single entry on an index-backed adaptor.
    ///
    /// # Errors
    ///
    /// As [`entries`](Self::entries).
    pub fn count(self) -> Result<usize, StoreError> {
        let (index, (lo, hi)) = self.plan()?;
        if self.map.ensure_index()? {
            Ok(scan::<S>(self.map.space(index), &lo, &hi).len())
        } else {
            Ok(self.rows_in_memory(index, &lo, &hi)?.len())
        }
    }

    fn fail(&mut self, error: String) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    /// The matching entry ids, in result order, after `skip` and `limit`.
    fn hits(&self) -> Result<Vec<Id>, StoreError> {
        let (index, (lo, hi)) = self.plan()?;
        let rows = if self.map.ensure_index()? {
            let space = self.map.space(index);
            match (self.desc, self.limit) {
                (true, limit) => descending::<S>(space, &lo, &hi, self.skip, limit),
                // One host scan when the page fits in one; the host caps a scan,
                // so anything larger is walked in pages rather than cut short.
                (false, Some(limit)) if limit <= SCAN_PAGE => {
                    S::index_range(space, lo, hi, self.skip, Some(limit))
                }
                (false, limit) => {
                    let rows = scan::<S>(space, &lo, &hi).into_iter().skip(self.skip);
                    match limit {
                        Some(limit) => rows.take(limit).collect(),
                        None => rows.collect(),
                    }
                }
            }
        } else {
            let mut rows = self.rows_in_memory(index, &lo, &hi)?;
            if self.desc {
                rows.reverse();
            }
            let rows = rows.into_iter().skip(self.skip);
            match self.limit {
                Some(limit) => rows.take(limit).collect(),
                None => rows.collect(),
            }
        };
        Ok(rows.into_iter().map(|(_, entry)| entry).collect())
    }

    /// Resolve the index and turn the conditions into row bounds `[lo, hi)`
    /// within the index's keyspace.
    fn plan(&self) -> Result<(usize, RowBounds), StoreError> {
        if let Some(error) = &self.error {
            return Err(invalid(error.clone()));
        }
        let index = self
            .index
            .ok_or_else(|| invalid("no index by that name on this map".to_owned()))?;
        let prefix = &self.prefix;
        let with_prefix = |tail: &[u8]| {
            let mut bytes = prefix.clone();
            bytes.extend_from_slice(tail);
            bytes
        };

        // A prefix is complete components, so every row under it lies in
        // `[prefix, past(prefix))`. Within that, the rows whose next component
        // equals `key` lie in `[open_form(key), past(key))` — the module docs
        // show why — which is all a range bound needs.
        let lo = match self.range.as_ref().map(|(lower, _)| lower) {
            Some(Bound::Included(key)) => Bound::Included(with_prefix(open_form(key))),
            Some(Bound::Excluded(key)) => Bound::Included(with_prefix(&past(key))),
            Some(Bound::Unbounded) | None if prefix.is_empty() => Bound::Unbounded,
            Some(Bound::Unbounded) | None => Bound::Included(prefix.clone()),
        };
        let hi = match self.range.as_ref().map(|(_, upper)| upper) {
            Some(Bound::Included(key)) => Bound::Excluded(with_prefix(&past(key))),
            Some(Bound::Excluded(key)) => Bound::Excluded(with_prefix(open_form(key))),
            Some(Bound::Unbounded) | None if prefix.is_empty() => Bound::Unbounded,
            Some(Bound::Unbounded) | None => Bound::Excluded(past(prefix)),
        };
        Ok((index, (lo, hi)))
    }

    /// The rows the index would hold within `[lo, hi)`, ascending, derived by
    /// scanning every entry — the answer for an adaptor that backs no index.
    fn rows_in_memory(
        &self,
        index: usize,
        lo: &Bound<Vec<u8>>,
        hi: &Bound<Vec<u8>>,
    ) -> Result<Vec<(Vec<u8>, Id)>, StoreError> {
        let mut rows = BTreeMap::new();
        for (entry, _key, value) in self.map.inner.entries_with_ids()? {
            let mut keys = Vec::new();
            value.index_keys(index, &mut keys);
            for row in keys.iter().map(|k| row_key(k, entry)) {
                if within(&row, lo, hi) {
                    let _ = rows.insert(row, entry);
                }
            }
        }
        Ok(rows.into_iter().collect())
    }
}

/// Rows of `space` within `[lo, hi)` in DESCENDING order, after skipping
/// `skip`, capped at `limit` — one reverse seek per row, so a page costs
/// `O((skip + limit) log n)` however large the index.
fn descending<S: StorageAdaptor>(
    space: Id,
    lo: &Bound<Vec<u8>>,
    hi: &Bound<Vec<u8>>,
    skip: usize,
    limit: Option<usize>,
) -> Vec<(Vec<u8>, Id)> {
    let mut rows = Vec::new();
    let mut upper = hi.clone();
    let mut skipped = 0;
    while limit.is_none_or(|limit| rows.len() < limit) {
        let Some((row, entry)) = S::index_last_in(space, lo.clone(), upper) else {
            break;
        };
        upper = Bound::Excluded(row.clone());
        if skipped < skip {
            skipped += 1;
        } else {
            rows.push((row, entry));
        }
    }
    rows
}

impl<K, V, S> fmt::Debug for IndexedMap<K, V, S>
where
    K: fmt::Debug + BorshSerialize + BorshDeserialize,
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IndexedMap")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<K, V, S> Default for IndexedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S> Serialize for IndexedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + Serialize,
    V: BorshSerialize + BorshDeserialize + Serialize,
    S: StorageAdaptor,
{
    fn serialize<Ser: serde::Serializer>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        Serialize::serialize(&self.inner, serializer)
    }
}

/// An indexed map is a search index's collection through the map it wraps:
/// its entries are that map's.
impl<K, V, S> calimero_sdk::search::SearchCollection for IndexedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize + calimero_sdk::search::Searchable,
    S: StorageAdaptor,
{
    type Key = K;
    type Value = V;

    fn search_entry(
        &self,
        id: [u8; 32],
    ) -> Result<Option<calimero_sdk::search::Entry<Self>>, calimero_sdk::search::SearchError> {
        self.inner.search_entry(id)
    }

    fn search_page(
        &self,
        from: [u8; 32],
        at_least: usize,
    ) -> Result<calimero_sdk::search::Page, calimero_sdk::search::SearchError> {
        self.inner.search_page(from, at_least)
    }
}

impl<K, V, S> Data for IndexedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        self.inner.collections()
    }

    fn element(&self) -> &Element {
        self.inner.element()
    }

    fn element_mut(&mut self) -> &mut Element {
        self.inner.element_mut()
    }
}

/// Re-key a nested `IndexedMap` exactly as a nested `UnorderedMap` is re-keyed:
/// the entries are the map's, and the indexes follow the new id on their own
/// (the marker no longer matches, so the next query rebuilds them).
impl<K, V, S> super::rekey::RekeyTarget for IndexedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn rekey_relative_to(&mut self, parent_id: Id) {
        self.inner.rekey_relative_to(parent_id);
    }
}

/// The entries' merge. A merge writes through the inner map without touching
/// the indexes, which the marker catches: the next query rebuilds.
#[diagnostic::do_not_recommend]
impl<K, V, S> Mergeable for IndexedMap<K, V, S>
where
    K: StorageKey + Clone,
    V: BorshSerialize + BorshDeserialize + Mergeable + 'static,
    S: StorageAdaptor,
{
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.inner.merge(&other.inner)
    }
}

/// `UnorderedMap`, deliberately: the indexes are node-local and derived, so
/// the sync layer has nothing to treat differently.
impl<K: 'static, V: 'static, S: StorageAdaptor> CrdtMeta for IndexedMap<K, V, S> {
    fn crdt_type() -> CrdtType {
        CrdtType::UnorderedMap
    }

    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured
    }

    fn can_contain_crdts() -> bool {
        true
    }
}

/// Structural: merged by its `crdt_type` like an `UnorderedMap`.
#[diagnostic::do_not_recommend]
impl<K, V, S: StorageAdaptor> MergeStrategy for IndexedMap<K, V, S> {
    const DISPATCHED: bool = false;
}

#[cfg(test)]
mod tests;
