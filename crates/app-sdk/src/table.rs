//! Tables with declared indexes, unique constraints and counts (D9).
//!
//! Key layout, every key starting with the table name as a string part:
//!
//! - `(table, "r", key..)` is the record, a deterministic CBOR value;
//! - `(table, "i", index, fields.., key..)` is an index entry, holding the record's storage key;
//! - `(table, "u", name, fields..)` is a unique entry, holding the record's storage key;
//! - `(table, "c", name, fields..)` is a count, a CBOR integer changed only with `Store::add` (D47).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::{bounds, read_page};
use crate::encoding::key::{self, KeyPart, ToKey};
use crate::encoding::value;
use crate::error::{StorageError, TableError};
use crate::store::{check_key, DefaultStore, RangeCursor, Store, MAX_RANGE_BATCH};

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
    name: String,
    store: S,
    _record: PhantomData<fn() -> T>,
}

impl<T: Record, S: Store> StateField for Table<T, S> {
    const KIND: &'static str = "table";
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
            name: name.into(),
            store,
            _record: PhantomData,
        }
    }

    fn prefix(&self, kind: &str, rest: &[&str], parts: &[KeyPart]) -> Vec<KeyPart> {
        let mut all = vec![KeyPart::Str(self.name.clone()), KeyPart::Str(kind.into())];
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
    fn check_unique(&self, new: &Entries) -> Result<(), TableError> {
        for (name, key) in &new.uniques {
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
    fn reindex(&mut self, old: Option<&Entries>, new: Option<&Entries>) {
        if let Some(old) = old {
            let keep =
                |k: &Vec<u8>, list: Option<&Vec<Vec<u8>>>| list.is_some_and(|l| l.contains(k));
            for k in &old.indexes {
                if !keep(k, new.map(|n| &n.indexes)) {
                    self.store.delete(k);
                }
            }
            for (_, k) in &old.uniques {
                if !new.is_some_and(|n| n.uniques.iter().any(|(_, nk)| nk == k)) {
                    self.store.delete(k);
                }
            }
            for k in &old.counts {
                if !keep(k, new.map(|n| &n.counts)) {
                    self.store.add(k, -1);
                }
            }
        }
        if let Some(new) = new {
            for k in &new.indexes {
                if !old.is_some_and(|o| o.indexes.contains(k)) {
                    self.store.put(k, &new.record);
                }
            }
            for (_, k) in &new.uniques {
                self.store.put(k, &new.record);
            }
            for k in &new.counts {
                if !old.is_some_and(|o| o.counts.contains(k)) {
                    self.store.add(k, 1);
                }
            }
        }
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
        self.check_unique(&new)?;
        self.store.put(&new.record, &bytes);
        self.reindex(None, Some(&new));
        Ok(())
    }

    /// Insert or replace a record; unique constraints still apply.
    pub fn upsert(&mut self, record: T) -> Result<(), TableError> {
        let new = self.entries(&record)?;
        let bytes = value::to_vec(&record)?;
        self.check_unique(&new)?;
        let old = self
            .read(&new.record)?
            .map(|o| self.entries(&o))
            .transpose()?;
        self.store.put(&new.record, &bytes);
        self.reindex(old.as_ref(), Some(&new));
        Ok(())
    }

    /// The record with this key.
    pub fn get<K: ToKey + ?Sized>(&self, key: &K) -> Result<Option<T>, StorageError> {
        self.read(&self.record_key(&key.to_key())?)
    }

    /// Whether a record with this key exists.
    pub fn contains<K: ToKey + ?Sized>(&self, key: &K) -> Result<bool, StorageError> {
        Ok(self.store.get(&self.record_key(&key.to_key())?).is_some())
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
        self.check_unique(&new)?;
        self.store.put(&new.record, &bytes);
        self.reindex(Some(&old), Some(&new));
        Ok(true)
    }

    /// Remove the record with this key and its index, unique and count entries.
    pub fn remove<K: ToKey + ?Sized>(&mut self, key: &K) -> Result<bool, StorageError> {
        let record_key = self.record_key(&key.to_key())?;
        let Some(record) = self.read(&record_key)? else {
            return Ok(false);
        };
        let old = self.entries(&record)?;
        self.store.delete(&old.record);
        self.reindex(Some(&old), None);
        Ok(true)
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
