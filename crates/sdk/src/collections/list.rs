//! `List<T>`: an ordered list where inserting or moving an item anywhere is a few writes.
//!
//! Keys:
//!
//! - `(path.., "o", position)` holds `{id, value}`, in list order: the position is bytes that
//!   sort as the list does (8 bytes of a counter, then a fraction);
//! - `(path.., "i", id)` holds the item's position, so an item is found by its id;
//! - `(path.., "n")` holds the length.
//!
//! An item's id (`env::unique_id()`) never changes: a move writes a new position, not a new
//! item. Appending or prepending steps the counter, so keys stay short however long the list
//! grows; inserting between two items takes a fraction between theirs, a byte longer every
//! few inserts into the same gap.

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::app::StateField;
use crate::commons::{bounds, clear_prefix, collection_key, iter_entries, read_count, KeyPath};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::env;
use crate::error::StorageError;
use crate::json::Bytes;
use crate::store::{DefaultStore, RangeCursor, Store};
use crate::table::{FindOpts, Iter, Progress};

/// Where the first item of an empty list goes: the middle of the counter, so the list can grow
/// both ways.
const MIDDLE: u64 = 1 << 63;

/// An ordered list with stable item ids, where `insert_after`, `insert_before` and the moves
/// cost a few reads and writes wherever the item is, unlike [`Vector`](super::Vector), whose
/// inserts rewrite everything after the position. Reading in order is one cursor.
pub struct List<T, S: Store = DefaultStore> {
    name: KeyPath,
    store: S,
    _type: PhantomData<fn() -> T>,
}

impl<T, S: Store> StateField for List<T, S> {
    const KIND: &'static str = "list";
}

impl<T, S: Store> crate::collections::Nestable<S> for List<T, S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self {
            name: path,
            store,
            _type: PhantomData,
        }
    }
}

/// A stored item: its id and its value.
#[derive(Serialize, Deserialize)]
struct Item<V> {
    id: String,
    value: V,
}

/// A position: the counter's 8 big-endian bytes, then the fraction.
fn position(int: u64, frac: &[u8]) -> Vec<u8> {
    [&int.to_be_bytes()[..], frac].concat()
}

/// A position split back into counter and fraction.
fn split(pos: &[u8]) -> Result<(u64, &[u8]), StorageError> {
    let (int, frac) = pos
        .split_first_chunk::<8>()
        .ok_or_else(|| StorageError::Decode("not a list position".into()))?;
    Ok((u64::from_be_bytes(*int), frac))
}

/// A fraction strictly between `lo` and `hi` (`None`: 1.0), with no trailing zero byte, so a
/// fraction between it and anything above stays possible.
fn fraction_between(lo: &[u8], hi: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    // The upper bound per digit: `hi`'s digit while it holds, 256 once it no longer bounds.
    let mut bounded = hi.is_some();
    loop {
        let l = u16::from(lo.get(i).copied().unwrap_or(0));
        let h = if bounded {
            u16::from(hi.and_then(|h| h.get(i)).copied().unwrap_or(0))
        } else {
            256
        };
        if h > l + 1 {
            // Room for a digit strictly between: done.
            out.push(((l + h) / 2) as u8);
            return out;
        }
        // `h` is `l` or `l + 1`: keep `l` and look further, where `hi` no longer bounds once
        // it went past.
        out.push(l as u8);
        if h > l {
            bounded = false;
        }
        i += 1;
    }
}

/// A position strictly between `a` and `b` (either may be absent: the list's ends).
fn between(a: Option<&[u8]>, b: Option<&[u8]>) -> Result<Vec<u8>, StorageError> {
    Ok(match (a, b) {
        (None, None) => position(MIDDLE, &[]),
        (Some(a), None) => {
            let (int, frac) = split(a)?;
            match int.checked_add(1) {
                Some(next) => position(next, &[]),
                None => position(int, &fraction_between(frac, None)),
            }
        }
        (None, Some(b)) => {
            let (int, frac) = split(b)?;
            match int.checked_sub(1) {
                Some(prev) => position(prev, &[]),
                None => position(int, &fraction_between(&[], Some(frac))),
            }
        }
        (Some(a), Some(b)) => {
            let (ai, af) = split(a)?;
            let (bi, bf) = split(b)?;
            if bi > ai + 1 {
                position(ai + (bi - ai) / 2, &[])
            } else if bi == ai {
                position(ai, &fraction_between(af, Some(bf)))
            } else {
                position(ai, &fraction_between(af, None))
            }
        }
    })
}

impl<T, S: Store + Default> List<T, S> {
    /// A list on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T, S: Store> List<T, S> {
    /// A list on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: KeyPath::root(name),
            store,
            _type: PhantomData,
        }
    }

    fn order_key(&self, pos: &[u8]) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "o", &[KeyPart::Bytes(pos.to_vec())])
    }

    fn order_base(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "o", &[])
    }

    fn id_key(&self, id: &str) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "i", &[KeyPart::Str(id.into())])
    }

    fn len_key(&self) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "n", &[])
    }

    /// How many items there are: one read.
    pub fn len(&self) -> Result<u64, StorageError> {
        read_count(&self.store, &self.len_key()?, self.name.label())
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        Ok(self.len()? == 0)
    }

    /// The position of the item `id`; `UnknownItem` when there is none.
    fn position_of(&self, id: &str) -> Result<Vec<u8>, StorageError> {
        let stored = self
            .store
            .get(&self.id_key(id)?)
            .ok_or_else(|| StorageError::UnknownItem(id.into()))?;
        Ok(value::from_slice::<Bytes>(&stored)?.0)
    }

    /// The position of the item next to `pos`: after it, or before it with `before`.
    fn neighbour(&self, pos: &[u8], before: bool) -> Result<Option<Vec<u8>>, StorageError> {
        let base = self.order_base()?;
        let here = self.order_key(pos)?;
        let (start, end) = bounds(&base);
        let mut cursor = if before {
            self.store.range(&start, &here, true)
        } else {
            self.store
                .range(&[here.as_slice(), &[0x00]].concat(), &end, false)
        };
        cursor
            .next(1)
            .into_iter()
            .next()
            .map(|(k, _)| self.position_from_key(&base, &k))
            .transpose()
    }

    /// The position at one end: the first item's, or the last's.
    fn end(&self, last: bool) -> Result<Option<Vec<u8>>, StorageError> {
        let base = self.order_base()?;
        let (start, end) = bounds(&base);
        self.store
            .range(&start, &end, last)
            .next(1)
            .into_iter()
            .next()
            .map(|(k, _)| self.position_from_key(&base, &k))
            .transpose()
    }

    fn position_from_key(&self, base: &[u8], stored: &[u8]) -> Result<Vec<u8>, StorageError> {
        match crate::commons::key_after(stored, base)?.as_slice() {
            [KeyPart::Bytes(pos)] => Ok(pos.clone()),
            _ => Err(StorageError::Decode("not a list position".into())),
        }
    }
}

impl<T: Serialize + DeserializeOwned, S: Store> List<T, S> {
    /// Store `value` as a new item at `pos`; returns its id.
    fn put_new(&mut self, pos: &[u8], value: &T) -> Result<String, StorageError> {
        let id = env::unique_id();
        let order = self.order_key(pos)?;
        let at = self.id_key(&id)?;
        let item = value::to_vec(&Item {
            id: id.clone(),
            value,
        })?;
        self.store.put(&order, &item)?;
        self.store.put(&at, &value::to_vec(&Bytes(pos.to_vec()))?)?;
        self.store.add(&self.len_key()?, 1)?;
        Ok(id)
    }

    /// Append `value`; returns the new item's id. Finds the last position (a cursor read of
    /// one entry), then two writes and the length.
    pub fn push_back(&mut self, value: &T) -> Result<String, StorageError> {
        let last = self.end(true)?;
        let pos = between(last.as_deref(), None)?;
        self.put_new(&pos, value)
    }

    /// Prepend `value`; returns the new item's id.
    pub fn push_front(&mut self, value: &T) -> Result<String, StorageError> {
        let first = self.end(false)?;
        let pos = between(None, first.as_deref())?;
        self.put_new(&pos, value)
    }

    /// Insert `value` right after the item `after`; returns the new item's id. A read of
    /// `after`'s position and of its neighbour, then two writes and the length, wherever it is.
    pub fn insert_after(&mut self, after: &str, value: &T) -> Result<String, StorageError> {
        let at = self.position_of(after)?;
        let next = self.neighbour(&at, false)?;
        let pos = between(Some(&at), next.as_deref())?;
        self.put_new(&pos, value)
    }

    /// Insert `value` right before the item `before`; returns the new item's id.
    pub fn insert_before(&mut self, before: &str, value: &T) -> Result<String, StorageError> {
        let at = self.position_of(before)?;
        let prev = self.neighbour(&at, true)?;
        let pos = between(prev.as_deref(), Some(&at))?;
        self.put_new(&pos, value)
    }

    /// The value of the item `id`; `None` when there is none.
    pub fn get(&self, id: &str) -> Result<Option<T>, StorageError> {
        let pos = match self.position_of(id) {
            Ok(pos) => pos,
            Err(StorageError::UnknownItem(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let stored = self
            .store
            .get(&self.order_key(&pos)?)
            .ok_or(StorageError::Dangling)?;
        Ok(Some(value::from_slice::<Item<T>>(&stored)?.value))
    }

    /// Replace the value of the item `id`, keeping its place; returns the old value.
    pub fn set(&mut self, id: &str, value: &T) -> Result<T, StorageError> {
        let pos = self.position_of(id)?;
        let order = self.order_key(&pos)?;
        let old = self.store.get(&order).ok_or(StorageError::Dangling)?;
        let old = value::from_slice::<Item<T>>(&old)?.value;
        let item = value::to_vec(&Item {
            id: id.to_owned(),
            value,
        })?;
        self.store.put(&order, &item)?;
        Ok(old)
    }

    /// Remove the item `id` and return its value; `None` when there is none.
    pub fn remove(&mut self, id: &str) -> Result<Option<T>, StorageError> {
        let pos = match self.position_of(id) {
            Ok(pos) => pos,
            Err(StorageError::UnknownItem(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let order = self.order_key(&pos)?;
        let stored = self.store.get(&order).ok_or(StorageError::Dangling)?;
        let value = value::from_slice::<Item<T>>(&stored)?.value;
        self.store.delete(&order)?;
        self.store.delete(&self.id_key(id)?)?;
        self.store.add(&self.len_key()?, -1)?;
        Ok(Some(value))
    }

    /// Put the item `id` at `pos`: its row moves (read, delete, put) and its id row points
    /// there. The value is copied as stored, never decoded.
    fn move_to(&mut self, id: &str, from: &[u8], to: &[u8]) -> Result<(), StorageError> {
        let old = self.order_key(from)?;
        let stored = self.store.get(&old).ok_or(StorageError::Dangling)?;
        self.store.delete(&old)?;
        self.store.put(&self.order_key(to)?, &stored)?;
        self.store
            .put(&self.id_key(id)?, &value::to_vec(&Bytes(to.to_vec()))?)?;
        Ok(())
    }

    /// Move the item `id` right after the item `after`: a few reads and three writes,
    /// wherever both are. Moving an item after itself changes nothing.
    pub fn move_after(&mut self, id: &str, after: &str) -> Result<(), StorageError> {
        let from = self.position_of(id)?;
        if id == after {
            return Ok(());
        }
        let at = self.position_of(after)?;
        let next = self.neighbour(&at, false)?;
        if next.as_deref() == Some(from.as_slice()) {
            return Ok(());
        }
        let to = between(Some(&at), next.as_deref())?;
        self.move_to(id, &from, &to)
    }

    /// Move the item `id` right before the item `before`.
    pub fn move_before(&mut self, id: &str, before: &str) -> Result<(), StorageError> {
        let from = self.position_of(id)?;
        if id == before {
            return Ok(());
        }
        let at = self.position_of(before)?;
        let prev = self.neighbour(&at, true)?;
        if prev.as_deref() == Some(from.as_slice()) {
            return Ok(());
        }
        let to = between(prev.as_deref(), Some(&at))?;
        self.move_to(id, &from, &to)
    }

    /// Move the item `id` to the front.
    pub fn move_to_front(&mut self, id: &str) -> Result<(), StorageError> {
        let from = self.position_of(id)?;
        let first = self.end(false)?;
        if first.as_deref() == Some(from.as_slice()) {
            return Ok(());
        }
        let to = between(None, first.as_deref())?;
        self.move_to(id, &from, &to)
    }

    /// Move the item `id` to the back.
    pub fn move_to_back(&mut self, id: &str) -> Result<(), StorageError> {
        let from = self.position_of(id)?;
        let last = self.end(true)?;
        if last.as_deref() == Some(from.as_slice()) {
            return Ok(());
        }
        let to = between(last.as_deref(), None)?;
        self.move_to(id, &from, &to)
    }

    /// Every item as `(id, value)`, in list order, through one cursor.
    pub fn iter(&self) -> Iter<'_, (String, T)> {
        self.iter_with(FindOpts::default())
    }

    /// The items from `opts` on: `.reverse()` walks from the back, `.limit(n)` reads `n` per
    /// batch.
    pub fn iter_with(&self, opts: FindOpts) -> Iter<'_, (String, T)> {
        match self.order_base() {
            Ok(base) => iter_entries(&self.store, &base, &opts, |(_, v)| {
                let item: Item<T> = value::from_slice(&v)?;
                Ok((item.id, item.value))
            }),
            Err(e) => Iter::failed(e),
        }
    }

    /// The first item as `(id, value)`.
    pub fn first(&self) -> Result<Option<(String, T)>, StorageError> {
        self.iter_with(FindOpts::default().limit(1))
            .next()
            .transpose()
    }

    /// The last item as `(id, value)`.
    pub fn last(&self) -> Result<Option<(String, T)>, StorageError> {
        self.iter_with(FindOpts::default().limit(1).reverse())
            .next()
            .transpose()
    }

    /// Remove every item, a batch at a time; returns how many stored entries there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        let all = crate::commons::encode(self.name.parts())?;
        clear_prefix(&mut self.store, &all)
    }

    /// Remove at most `max` items from the front, with their id rows and the length;
    /// `done` once the list is empty (see [`Table::clear_some`](crate::Table::clear_some)).
    pub fn clear_some(&mut self, max: u64) -> Result<Progress, StorageError> {
        let base = self.order_base()?;
        let (start, end) = bounds(&base);
        let mut removed = 0u64;
        let mut done = false;
        while removed < max {
            // A fresh cursor per batch: the previous batch is gone, so this one starts after it.
            let want = (max - removed).min(u64::from(crate::store::MAX_RANGE_BATCH)) as u32;
            let batch = self.store.range(&start, &end, false).next(want);
            if batch.is_empty() {
                done = true;
                break;
            }
            for (k, v) in batch {
                let item: Item<serde::de::IgnoredAny> = value::from_slice(&v)?;
                self.store.delete(&k)?;
                self.store.delete(&self.id_key(&item.id)?)?;
                removed += 1;
            }
        }
        if removed > 0 {
            self.store.add(&self.len_key()?, -(removed as i64))?;
        }
        if !done {
            done = self.store.range(&start, &end, false).next(1).is_empty();
        }
        Ok(Progress { removed, done })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_fall_strictly_between_and_never_end_in_zero() {
        let cases: &[(&[u8], Option<&[u8]>)] = &[
            (&[], None),
            (&[], Some(&[1])),
            (&[5, 7], Some(&[5, 9])),
            (&[5, 7], Some(&[5, 8])),
            (&[255, 255], None),
            (&[0, 0, 1], Some(&[0, 1])),
        ];
        for &(lo, hi) in cases {
            let mid = fraction_between(lo, hi);
            assert!(mid.as_slice() > lo, "{lo:?} < {mid:?}");
            if let Some(hi) = hi {
                assert!(mid.as_slice() < hi, "{mid:?} < {hi:?}");
            }
            assert_ne!(mid.last(), Some(&0), "{mid:?}");
        }
    }

    #[test]
    fn many_inserts_into_one_gap_stay_ordered_and_short() {
        // Inserting again and again right after the same item.
        let a = position(MIDDLE, &[]);
        let b = position(MIDDLE + 1, &[]);
        let mut hi = b.clone();
        for _ in 0..1_000 {
            let mid = between(Some(&a), Some(&hi)).unwrap();
            assert!(mid > a && mid < hi);
            hi = mid;
        }
        assert!(
            hi.len() < 8 + 1_000 / 7 + 2,
            "about a byte per 8 halvings: {}",
            hi.len()
        );
        // Appending never grows the key.
        let mut last = a;
        for _ in 0..1_000 {
            last = between(Some(&last), None).unwrap();
        }
        assert_eq!(last.len(), 8);
    }
}
