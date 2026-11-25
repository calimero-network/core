//! Tables with declared indexes, unique constraints and counts (D9).
//!
//! Key layout, every key starting with the table name as a string part:
//!
//! - `(table, "r", key..)` is the record, a deterministic CBOR value;
//! - `(table, "i", index, fields.., key..)` is an index entry, holding the record's storage key;
//! - `(table, "u", name, fields..)` is a unique entry, holding the record's storage key;
//! - `(table, "c", name, fields..)` is a count, a CBOR integer changed only with `Store::add` (D47);
//! - `(table, "n")` is how many records the table holds, a CBOR integer: written once by the
//!   first write that finds it missing (by counting the records), then changed only with
//!   `Store::add`;
//! - `(table, "l")` holds CBOR `null` once `(table, "n")` is kept: a write reads it, never
//!   `(table, "n")`, so inserts into one table do not conflict over the length (D47).
//!
//! A table written before the length existed has neither row until its first insert or
//! removal; until then [`Table::len`] counts its records.
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (D decisions in section 2): D9
//! (tables, indexes and counts live in the SDK), D47 (`storage.add` and `unique-id` avoid
//! serializing calls).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::{bounds, iter_entries, read_page, KeyPath};
use crate::encoding::key::{self, KeyPart, ToKey};
use crate::encoding::value;
use crate::error::{StorageError, TableError};
use crate::store::{check_key, DefaultStore, RangeCursor, Store, MAX_RANGE_BATCH};

/// The kind part of the row holding how many records the table holds.
const LEN: &str = "n";
/// The kind part of the row saying [`LEN`] is kept; it is written once and only ever read.
const LEN_KEPT: &str = "l";
/// What the [`LEN_KEPT`] row holds: CBOR `null`.
const PRESENT: &[u8] = &[0xF6];

/// A named set of fields: an index, a unique constraint or a count.
pub type FieldSet = (&'static str, &'static [&'static str]);

/// A table record and its declared keys, indexes, unique constraints and counts.
pub trait Record: Serialize + DeserializeOwned {
    /// The fields forming the record key.
    const KEY: &'static [&'static str];
    /// Unique constraints: name and fields.
    const UNIQUE: &'static [FieldSet];
    /// Indexes: name and fields.
    const INDEXES: &'static [FieldSet];
    /// Maintained counts: name and fields.
    const COUNTS: &'static [FieldSet];
    /// Text fields the host indexes for full-text search (spec section 7); none by default.
    const SEARCH: &'static [&'static str] = &[];
    /// [`Self::SEARCH`] as a JSON list, for the state ABI.
    const SEARCH_JSON: &'static str = "[]";

    /// The key parts of the named fields, in order.
    ///
    /// Implementations may panic on a field the record does not have: the field lists are
    /// constants checked by the app's own tests.
    fn key_parts(&self, fields: &[&str]) -> Vec<KeyPart>;
}

/// The default page size, and the cap on `limit`, of a query.
pub const DEFAULT_LIMIT: u32 = 100;
/// The largest page a query returns.
pub const MAX_LIMIT: u32 = 1000;

/// One page of a query. `next` is an opaque cursor, present when more follow.
///
/// A cursor is the base64url of the last returned storage key. It is bound to the index and
/// the prefix it came from (another index or prefix gives `BadCursor`), not to the world
/// version or to `from`/`to`, and it is opaque by convention only: it is not signed, so a
/// client can forge one, which can only move its own read within the same index and prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// The records of this page.
    pub items: Vec<T>,
    /// The cursor of the next page.
    pub next: Option<String>,
}

impl<T> IntoIterator for Page<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

/// Reads the next batch for [`Iter`]; an empty batch is the end.
type NextBatch<'a, T> = Box<dyn FnMut() -> Result<Vec<T>, StorageError> + 'a>;

/// Every item of a multi-row read, in the read's order, fetched a batch at a time as the
/// loop reaches it: nothing is read before the first `next`, and stopping early (`take`,
/// `find`, `break`) reads no further batch.
///
/// The collections' and tables' iterators read through one host cursor: one range open, then
/// one host call per batch of `limit` entries (1,000 when unset, the host's largest batch),
/// with no entry read twice. `from`, `to`, `reverse` and a starting `cursor` apply as they do
/// to one page. A failed read is yielded once as `Err`, and the iterator ends after it.
///
/// On a node a cursor reads the state as of the first batch (the committed state and the
/// run's writes before it), so writes the loop makes are not met again.
pub struct Iter<'a, T> {
    next_batch: NextBatch<'a, T>,
    items: std::vec::IntoIter<T>,
    done: bool,
}

impl<'a, T: 'a> Iter<'a, T> {
    /// Iterate the pages `fetch` reads, starting at `opts`: each page is one [`Page`] read
    /// of `opts.limit` (1,000 when unset), continued from its `next`.
    pub fn new(
        mut opts: FindOpts,
        mut fetch: impl FnMut(&FindOpts) -> Result<Page<T>, StorageError> + 'a,
    ) -> Self {
        opts.limit = Some(opts.limit.unwrap_or(MAX_LIMIT).clamp(1, MAX_LIMIT));
        let mut last = false;
        Self::from_batches(move || {
            if last {
                return Ok(Vec::new());
            }
            let page = fetch(&opts)?;
            match page.next {
                Some(cursor) => opts.cursor = Some(cursor),
                None => last = true,
            }
            Ok(page.items)
        })
    }

    /// Iterate the batches `next_batch` reads until it answers an empty one.
    pub(crate) fn from_batches(
        next_batch: impl FnMut() -> Result<Vec<T>, StorageError> + 'a,
    ) -> Self {
        Self {
            next_batch: Box::new(next_batch),
            items: Vec::new().into_iter(),
            done: false,
        }
    }

    /// An iterator that yields `e` and ends.
    pub(crate) fn failed(e: StorageError) -> Self {
        let mut e = Some(e);
        Self::from_batches(move || match e.take() {
            Some(e) => Err(e),
            None => Ok(Vec::new()),
        })
    }
}

impl<T> Iterator for Iter<'_, T> {
    type Item = Result<T, StorageError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.items.next() {
                return Some(Ok(item));
            }
            if self.done {
                return None;
            }
            match (self.next_batch)() {
                Ok(batch) if batch.is_empty() => self.done = true,
                Ok(batch) => self.items = batch.into_iter(),
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

impl<T> core::iter::FusedIterator for Iter<'_, T> {}

/// How far a bounded removal got (`clear_some`, [`Table::remove_by`]): how many it removed,
/// and whether nothing is left. A run makes at most 10,000 writes, so a large clear is done a
/// slice per run, from a timer, until `done`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// How many were removed this time.
    pub removed: u64,
    /// Whether nothing is left to remove.
    pub done: bool,
}

/// Options of [`Table::find`] and [`Table::scan`].
#[derive(Clone, Debug, Default)]
pub struct FindOpts {
    /// Page size; capped at 1,000; 100 when unset; 0 is an empty page.
    pub limit: Option<u32>,
    /// The `next` of the previous page.
    pub cursor: Option<String>,
    /// Newest key first.
    pub reverse: bool,
    /// Start at these further index parts (inclusive), after the prefix.
    pub from: Option<Vec<KeyPart>>,
    /// End before these further index parts (exclusive), after the prefix.
    pub to: Option<Vec<KeyPart>>,
}

impl FindOpts {
    /// Set the page size.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Continue from a cursor.
    pub fn cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    /// Read newest key first.
    pub fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    /// Start at these index parts after the prefix (inclusive).
    pub fn from(mut self, parts: Vec<KeyPart>) -> Self {
        self.from = Some(parts);
        self
    }

    /// End before these index parts after the prefix (exclusive).
    pub fn to(mut self, parts: Vec<KeyPart>) -> Self {
        self.to = Some(parts);
        self
    }
}

/// Every storage key one record owns, computed from its fields.
struct Entries {
    record: Vec<u8>,
    indexes: Vec<Vec<u8>>,
    uniques: Vec<(&'static str, Vec<u8>)>,
    counts: Vec<Vec<u8>>,
}

/// A table of `T` records over a store.
pub struct Table<T: Record, S: Store = DefaultStore> {
    name: KeyPath,
    store: S,
    _record: PhantomData<fn() -> T>,
}

impl<T: Record, S: Store> StateField for Table<T, S> {
    const KIND: &'static str = "table";
    const SEARCH: &'static str = T::SEARCH_JSON;
}

impl<T: Record, S: Store + Default> Table<T, S> {
    /// A table on the default store; the name is part of every key, so it never moves.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T: Record, S: Store> Table<T, S> {
    /// A table on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: KeyPath::root(name),
            store,
            _record: PhantomData,
        }
    }

    fn prefix(&self, kind: &str, rest: &[&str], parts: &[KeyPart]) -> Vec<KeyPart> {
        let mut all = self.name.parts().to_vec();
        all.push(KeyPart::Str(kind.into()));
        all.extend(rest.iter().map(|r| KeyPart::Str((*r).into())));
        all.extend_from_slice(parts);
        all
    }

    fn encode(&self, parts: &[KeyPart]) -> Result<Vec<u8>, StorageError> {
        let encoded = key::encode(parts);
        check_key(&encoded)?;
        Ok(encoded)
    }

    fn record_key(&self, key: &[KeyPart]) -> Result<Vec<u8>, StorageError> {
        self.encode(&self.prefix("r", &[], key))
    }

    fn entries(&self, record: &T) -> Result<Entries, StorageError> {
        let key = record.key_parts(T::KEY);
        let mut indexes = Vec::with_capacity(T::INDEXES.len());
        for (name, fields) in T::INDEXES {
            let mut parts = record.key_parts(fields);
            parts.extend(key.iter().cloned());
            indexes.push(self.encode(&self.prefix("i", &[name], &parts))?);
        }
        let mut uniques = Vec::with_capacity(T::UNIQUE.len());
        for (name, fields) in T::UNIQUE {
            let parts = record.key_parts(fields);
            // SQL NULL semantics: an entry with a None part is neither written nor checked.
            if parts.contains(&KeyPart::None) {
                continue;
            }
            uniques.push((*name, self.encode(&self.prefix("u", &[name], &parts))?));
        }
        let mut counts = Vec::with_capacity(T::COUNTS.len());
        for (name, fields) in T::COUNTS {
            let parts = record.key_parts(fields);
            counts.push(self.encode(&self.prefix("c", &[name], &parts))?);
        }
        Ok(Entries {
            record: self.record_key(&key)?,
            indexes,
            uniques,
            counts,
        })
    }

    /// Refuse a unique clash with a record other than the one at `record_key`.
    /// Refuse a unique entry another record holds. An entry the record already had (`old`)
    /// is its own, so it is neither read nor written again.
    fn check_unique(&self, new: &Entries, old: Option<&Entries>) -> Result<(), TableError> {
        for (name, key) in &new.uniques {
            if old.is_some_and(|o| o.uniques.iter().any(|(_, ok)| ok == key)) {
                continue;
            }
            if let Some(owner) = self.store.get(key) {
                if owner != new.record {
                    return Err(TableError::Duplicate {
                        index: (*name).into(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Move the index, unique and count entries from `old` to `new`; either may be absent.
    fn reindex(
        &mut self,
        old: Option<&Entries>,
        new: Option<&Entries>,
    ) -> Result<(), StorageError> {
        if let Some(old) = old {
            let keep =
                |k: &Vec<u8>, list: Option<&Vec<Vec<u8>>>| list.is_some_and(|l| l.contains(k));
            for k in &old.indexes {
                if !keep(k, new.map(|n| &n.indexes)) {
                    self.store.delete(k)?;
                }
            }
            for (_, k) in &old.uniques {
                if !new.is_some_and(|n| n.uniques.iter().any(|(_, nk)| nk == k)) {
                    self.store.delete(k)?;
                }
            }
            for k in &old.counts {
                if !keep(k, new.map(|n| &n.counts)) {
                    self.store.add(k, -1)?;
                }
            }
        }
        if let Some(new) = new {
            for k in &new.indexes {
                if !old.is_some_and(|o| o.indexes.contains(k)) {
                    self.store.put(k, &new.record)?;
                }
            }
            for (_, k) in &new.uniques {
                if !old.is_some_and(|o| o.uniques.iter().any(|(_, ok)| ok == k)) {
                    self.store.put(k, &new.record)?;
                }
            }
            for k in &new.counts {
                if !old.is_some_and(|o| o.counts.contains(k)) {
                    self.store.add(k, 1)?;
                }
            }
        }
        Ok(())
    }

    /// The encoded key of `(table, kind)`. Never over the key limit once a record key of this
    /// table passed it: `(table, "r", key..)` is longer.
    fn table_row(&self, kind: &str) -> Vec<u8> {
        key::encode(&self.prefix(kind, &[], &[]))
    }

    /// How many records are stored, counted through one cursor without decoding any.
    fn count_records(&self) -> Result<u64, StorageError> {
        let base = self.encode(&self.prefix("r", &[], &[]))?;
        let (start, end) = bounds(&base);
        let mut cursor = self.store.range(&start, &end, false);
        let mut n = 0;
        loop {
            let batch = cursor.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                return Ok(n);
            }
            n += batch.len() as u64;
        }
    }

    /// Move the stored length by `delta` after the records changed by it. Nothing when it is
    /// 0. When the length is kept, one read of [`LEN_KEPT`] and one `add`; otherwise (a table
    /// stored before the length existed, or emptied by `clear`) the records are counted, as
    /// they are now, and the length and [`LEN_KEPT`] written: the length is never started
    /// from 0 under records it did not count.
    fn adjust_len(&mut self, delta: i64) -> Result<(), StorageError> {
        if delta == 0 {
            return Ok(());
        }
        let len = self.table_row(LEN);
        let kept = self.table_row(LEN_KEPT);
        if self.store.get(&kept).is_some() {
            self.store.add(&len, delta)?;
            return Ok(());
        }
        let n = self.count_records()?;
        self.store.put(&len, &value::to_vec(&n)?)?;
        self.store.put(&kept, PRESENT)?;
        Ok(())
    }

    fn read(&self, record_key: &[u8]) -> Result<Option<T>, StorageError> {
        self.store
            .get(record_key)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Insert a record, refusing an existing key (`Duplicate { index: "key" }`) or a unique clash.
    pub fn insert(&mut self, record: T) -> Result<(), TableError> {
        let new = self.entries(&record)?;
        let bytes = value::to_vec(&record)?;
        if self.store.get(&new.record).is_some() {
            return Err(TableError::Duplicate {
                index: "key".into(),
            });
        }
        self.check_unique(&new, None)?;
        self.store.put(&new.record, &bytes)?;
        self.reindex(None, Some(&new))?;
        Ok(self.adjust_len(1)?)
    }

    /// Insert or replace a record; unique constraints still apply.
    pub fn upsert(&mut self, record: T) -> Result<(), TableError> {
        let new = self.entries(&record)?;
        let bytes = value::to_vec(&record)?;
        let old = self
            .read(&new.record)?
            .map(|o| self.entries(&o))
            .transpose()?;
        self.check_unique(&new, old.as_ref())?;
        self.store.put(&new.record, &bytes)?;
        self.reindex(old.as_ref(), Some(&new))?;
        if old.is_none() {
            self.adjust_len(1)?;
        }
        Ok(())
    }

    /// The record with this key.
    pub fn get<K: ToKey + ?Sized>(&self, key: &K) -> Result<Option<T>, StorageError> {
        self.read(&self.record_key(&key.to_key())?)
    }

    /// Full-text search over the fields the record declares with
    /// `#[table(search = [..])]` (spec section 7), best match first. Views only: the host
    /// keeps the index from the state the view reads. A hit whose record is gone is
    /// skipped.
    ///
    /// # Errors
    ///
    /// `Search` when the table declares no search fields or the query is refused (over 256
    /// bytes, a limit over 100); decoding errors as for `get`.
    pub fn search(
        &self,
        text: &str,
        opts: crate::SearchOpts,
    ) -> Result<crate::SearchResults<T>, StorageError> {
        if self.name.is_nested() {
            // The host indexes the tables the state declares, by name; a nested one is not.
            return Err(StorageError::Search(
                "search covers the state's own tables, not a nested one".into(),
            ));
        }
        let limit = opts.limit.unwrap_or(crate::search::DEFAULT_SEARCH_LIMIT);
        let page = crate::env::search_query(
            self.name.label(),
            text,
            opts.mode,
            opts.cursor.unwrap_or(0),
            limit,
        )
        .map_err(StorageError::Search)?;
        let mut items = Vec::with_capacity(page.hits.len());
        for hit in page.hits {
            if let Some(record) = self.read(&hit.key)? {
                items.push(crate::Found {
                    record,
                    score: hit.score,
                    snippet: hit.snippet,
                });
            }
        }
        Ok(crate::SearchResults {
            items,
            next: page.next,
            total: page.total,
        })
    }

    /// Whether a record with this key exists.
    pub fn contains<K: ToKey + ?Sized>(&self, key: &K) -> Result<bool, StorageError> {
        Ok(self.store.get(&self.record_key(&key.to_key())?).is_some())
    }

    /// How many records the table holds: one read of the length every insert and removal
    /// keeps.
    ///
    /// A table with no length row (stored before tables kept one and not inserted into or
    /// removed from since, or emptied by [`Table::clear`]) is counted instead: one cursor
    /// over every record, a host call per 1,000. Its next insert or removal writes the row.
    pub fn len(&self) -> Result<u64, StorageError> {
        let Some(bytes) = self.store.get(&self.encode(&self.prefix(LEN, &[], &[]))?) else {
            return self.count_records();
        };
        u64::try_from(value::int_from_slice(&bytes)?)
            .map_err(|_| StorageError::BadCounter(format!("{}.len", self.name.label())))
    }

    /// Whether the table holds no record: one read of the length, or, with no length row,
    /// one cursor read of one record.
    pub fn is_empty(&self) -> Result<bool, StorageError> {
        let Some(bytes) = self.store.get(&self.encode(&self.prefix(LEN, &[], &[]))?) else {
            return Ok(!self.any_under(&self.encode(&self.prefix("r", &[], &[]))?));
        };
        Ok(value::int_from_slice(&bytes)? == 0)
    }

    /// Change the record with this key and re-index what changed; false when it is absent.
    pub fn update<K: ToKey + ?Sized>(
        &mut self,
        key: &K,
        change: impl FnOnce(&mut T),
    ) -> Result<bool, TableError> {
        let record_key = self.record_key(&key.to_key())?;
        let Some(mut record) = self.read(&record_key)? else {
            return Ok(false);
        };
        let old = self.entries(&record)?;
        change(&mut record);
        let new = self.entries(&record)?;
        if new.record != old.record {
            return Err(StorageError::KeyChanged.into());
        }
        let bytes = value::to_vec(&record)?;
        self.check_unique(&new, Some(&old))?;
        self.store.put(&new.record, &bytes)?;
        self.reindex(Some(&old), Some(&new))?;
        Ok(true)
    }

    /// Remove the record with this key and its index, unique and count entries.
    pub fn remove<K: ToKey + ?Sized>(&mut self, key: &K) -> Result<bool, StorageError> {
        let record_key = self.record_key(&key.to_key())?;
        let Some(record) = self.read(&record_key)? else {
            return Ok(false);
        };
        self.remove_record(&record)?;
        self.adjust_len(-1)?;
        Ok(true)
    }

    /// Remove a record already read, with its index, unique and count entries: no read. The
    /// caller moves the length ([`Table::adjust_len`]), once for every record it removed.
    fn remove_record(&mut self, record: &T) -> Result<(), StorageError> {
        let old = self.entries(record)?;
        self.store.delete(&old.record)?;
        self.reindex(Some(&old), None)?;
        Ok(())
    }

    /// Whether any key is left under the encoded `base`: one cursor read of one entry.
    fn any_under(&self, base: &[u8]) -> bool {
        let (start, end) = bounds(base);
        !self.store.range(&start, &end, false).next(1).is_empty()
    }

    /// Keep only the records `keep` accepts; returns how many were removed. Every record is
    /// read once, through one cursor, and a rejected one is removed from what was read (a
    /// delete for it and each of its index and unique entries, an `add` per count), with no
    /// second read; the length moves once for all of them. Each removal costs
    /// `1 + indexes + uniques + counts` writes against the run's 10,000: past that, use
    /// [`Table::remove_by`] or [`Table::clear_some`] a slice per run.
    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) -> Result<u64, StorageError> {
        let mut removed = 0;
        let outcome = self.retain_counting(keep, &mut removed);
        // The length follows what was removed, even when a later record failed.
        self.adjust_len(-len_delta(removed))?;
        outcome.map(|()| removed)
    }

    fn retain_counting(
        &mut self,
        mut keep: impl FnMut(&T) -> bool,
        removed: &mut u64,
    ) -> Result<(), StorageError> {
        let base = self.encode(&self.prefix("r", &[], &[]))?;
        let (start, end) = bounds(&base);
        let mut all = self.store.range(&start, &end, false);
        loop {
            let batch = all.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                return Ok(());
            }
            for (_, bytes) in batch {
                let record: T = value::from_slice(&bytes)?;
                if !keep(&record) {
                    self.remove_record(&record)?;
                    *removed += 1;
                }
            }
        }
    }

    /// Remove the records whose index fields start with `prefix` (as [`Table::find`] finds
    /// them), at most `max` of them: a read of each record and its writes, nothing else.
    /// `done` says whether none is left under the prefix; pass `u64::MAX` for all.
    pub fn remove_by(
        &mut self,
        index: &str,
        prefix: &[KeyPart],
        max: u64,
    ) -> Result<Progress, StorageError> {
        let Some((_, fields)) = T::INDEXES.iter().find(|(name, _)| *name == index) else {
            return Err(StorageError::UnknownName(index.into()));
        };
        if prefix.len() > fields.len() {
            return Err(StorageError::PrefixTooLong {
                given: prefix.len(),
                max: fields.len(),
            });
        }
        let base = self.encode(&self.prefix("i", &[index], prefix))?;
        let mut removed = 0;
        let outcome = self.remove_under(&base, max, &mut removed, |t, record_key| {
            t.read(&record_key)?.ok_or(StorageError::Dangling)
        });
        self.adjust_len(-len_delta(removed))?;
        let exhausted = outcome?;
        Ok(Progress {
            removed,
            done: exhausted || !self.any_under(&base),
        })
    }

    /// Remove at most `max` of the records the entries under `base` lead to (`record` reads
    /// one from its entry's value), counting them in `removed`; whether the entries ran out.
    fn remove_under(
        &mut self,
        base: &[u8],
        max: u64,
        removed: &mut u64,
        record: impl Fn(&Self, Vec<u8>) -> Result<T, StorageError>,
    ) -> Result<bool, StorageError> {
        let (start, end) = bounds(base);
        let mut found = self.store.range(&start, &end, false);
        while *removed < max {
            let want = (max - *removed).min(u64::from(MAX_RANGE_BATCH)) as u32;
            let batch = found.next(want);
            if batch.is_empty() {
                return Ok(true);
            }
            for (_, entry) in batch {
                let record = record(self, entry)?;
                self.remove_record(&record)?;
                *removed += 1;
            }
        }
        Ok(false)
    }

    /// Remove at most `max` records, in key order, with their index, unique and count
    /// entries; `done` says whether the table is empty. Call it from a timer until `done` to
    /// clear a table too large for one run's writes.
    pub fn clear_some(&mut self, max: u64) -> Result<Progress, StorageError> {
        let base = self.encode(&self.prefix("r", &[], &[]))?;
        let mut removed = 0;
        let outcome = self.remove_under(&base, max, &mut removed, |_, bytes| {
            value::from_slice(&bytes)
        });
        self.adjust_len(-len_delta(removed))?;
        let exhausted = outcome?;
        Ok(Progress {
            removed,
            done: exhausted || !self.any_under(&base),
        })
    }

    /// Records whose index fields start with `prefix`, in index order, one page at a time.
    pub fn find(
        &self,
        index: &str,
        prefix: &[KeyPart],
        opts: FindOpts,
    ) -> Result<Page<T>, StorageError> {
        let Some((_, fields)) = T::INDEXES.iter().find(|(name, _)| *name == index) else {
            return Err(StorageError::UnknownName(index.into()));
        };
        if prefix.len() > fields.len() {
            return Err(StorageError::PrefixTooLong {
                given: prefix.len(),
                max: fields.len(),
            });
        }
        let base = self.prefix("i", &[index], prefix);
        let (entries, next) = read_page(&self.store, &self.encode(&base)?, &opts)?;
        let mut items = Vec::with_capacity(entries.len());
        for (_, record_key) in entries {
            let record = self.read(&record_key)?.ok_or(StorageError::Dangling)?;
            items.push(record);
        }
        Ok(Page { items, next })
    }

    /// All records in key order, one page at a time.
    pub fn scan(&self, opts: FindOpts) -> Result<Page<T>, StorageError> {
        let base = self.prefix("r", &[], &[]);
        let (entries, next) = read_page(&self.store, &self.encode(&base)?, &opts)?;
        let items = entries
            .iter()
            .map(|(_, bytes)| value::from_slice(bytes))
            .collect::<Result<_, _>>()?;
        Ok(Page { items, next })
    }

    /// Remove every record with its index, unique and count entries, a batch at a time;
    /// returns how many records there were.
    pub fn clear(&mut self) -> Result<u64, StorageError> {
        let records = self.encode(&self.prefix("r", &[], &[]))?;
        let removed = crate::commons::clear_prefix(&mut self.store, &records)?;
        let all = self.encode(self.name.parts())?;
        let _ = crate::commons::clear_prefix(&mut self.store, &all)?;
        Ok(removed)
    }

    /// Every record in key order, a page at a time as the loop reaches it.
    pub fn iter(&self) -> Iter<'_, T> {
        self.scan_iter(FindOpts::default())
    }

    /// [`Table::scan`] over every page from `opts` on.
    pub fn scan_iter(&self, opts: FindOpts) -> Iter<'_, T> {
        match self.encode(&self.prefix("r", &[], &[])) {
            Ok(base) => iter_entries(&self.store, &base, &opts, |(_, v)| value::from_slice(&v)),
            Err(e) => Iter::failed(e),
        }
    }

    /// [`Table::find`] over every page from `opts` on.
    pub fn find_iter(&self, index: &str, prefix: &[KeyPart], opts: FindOpts) -> Iter<'_, T> {
        let Some((_, fields)) = T::INDEXES.iter().find(|(name, _)| *name == index) else {
            return Iter::failed(StorageError::UnknownName(index.into()));
        };
        if prefix.len() > fields.len() {
            return Iter::failed(StorageError::PrefixTooLong {
                given: prefix.len(),
                max: fields.len(),
            });
        }
        match self.encode(&self.prefix("i", &[index], prefix)) {
            // An index entry holds the record's storage key: one read per record.
            Ok(base) => iter_entries(&self.store, &base, &opts, move |(_, record_key)| {
                self.read(&record_key)?.ok_or(StorageError::Dangling)
            }),
            Err(e) => Iter::failed(e),
        }
    }

    /// The maintained count `name` for the records matching `prefix`.
    ///
    /// A prefix naming every field of the count is one read; a shorter one sums the counts under it.
    pub fn count(&self, name: &str, prefix: &[KeyPart]) -> Result<u64, StorageError> {
        let Some((_, fields)) = T::COUNTS.iter().find(|(n, _)| *n == name) else {
            return Err(StorageError::UnknownName(name.into()));
        };
        if prefix.len() > fields.len() {
            return Err(StorageError::PrefixTooLong {
                given: prefix.len(),
                max: fields.len(),
            });
        }
        let base = self.prefix("c", &[name], prefix);
        let sum: i128 = if prefix.len() == fields.len() {
            match self.store.get(&self.encode(&base)?) {
                Some(bytes) => value::int_from_slice(&bytes)?,
                None => 0,
            }
        } else {
            let (start, end) = bounds(&self.encode(&base)?);
            let mut cursor = self.store.range(&start, &end, false);
            let mut sum = 0;
            loop {
                let batch = cursor.next(MAX_RANGE_BATCH);
                if batch.is_empty() {
                    break;
                }
                for (_, bytes) in batch {
                    sum += value::int_from_slice(&bytes)?;
                }
            }
            sum
        };
        u64::try_from(sum).map_err(|_| StorageError::BadCounter(name.into()))
    }
}

/// A number of records as a length change: a run removes far fewer than `i64::MAX`.
fn len_delta(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl<T: Record, S: Store> crate::collections::Nestable<S> for Table<T, S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self {
            name: path,
            store,
            _record: PhantomData,
        }
    }
}
