//! The `collections` host calls of [`HostCore`] (PoC): tables, maps and tallies by name,
//! written through `storage_put`/`storage_delete`/`storage_add` so the diff, the caps, view
//! denial and generations apply unchanged. Each call is one-to-one with a WIT function of
//! `calimero:app/collections`: the inner `Result` is the WIT `coll-error`, the outer a trap.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ciborium::Value;

use super::{HostCore, MAX_RANGE_BATCH};
use crate::collections::{
    camel, canonical, canonical_value, check_wit_key, collection_fuel, decode_key, encode_key,
    field_part, read_value, write_value, CollEntry, CollError, CollPage, CollectionSchema, KeyPart,
    TableSchema, MAX_COLLECTION_KEY_BYTES, MAX_COLLECTION_LIMIT,
};
use crate::state::{stored_key, KeyValue, GENERATION};
use crate::types::{HostDenied, InvocationKind};

/// A collection call's answer: a trap, a soft error or the value.
type Answer<T> = Result<Result<T, CollError>, HostDenied>;

/// Unwraps a soft result, answering its error to the guest.
macro_rules! soft {
    ($e:expr) => {
        match $e {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        }
    };
}

/// Every storage key one record owns, computed from its fields (as `calimero_sdk::table`).
struct Entries {
    record: Vec<u8>,
    indexes: Vec<Vec<u8>>,
    /// The unique entries with no `None` part, by their camelCase name.
    uniques: Vec<(String, Vec<u8>)>,
    counts: Vec<Vec<u8>>,
}

fn str_part(s: &str) -> KeyPart {
    KeyPart::Str(s.to_owned())
}

/// `parts` encoded, refused over [`MAX_COLLECTION_KEY_BYTES`].
fn checked(parts: &[KeyPart]) -> Result<Vec<u8>, CollError> {
    within(encode_key(parts))
}

/// Refuses a key over [`MAX_COLLECTION_KEY_BYTES`]. The SDK reads this exact message back
/// as its `KeyTooLong { len, max }` (`calimero_sdk` `host_collections::key_too_long`).
fn within(key: Vec<u8>) -> Result<Vec<u8>, CollError> {
    if key.len() > MAX_COLLECTION_KEY_BYTES {
        return Err(CollError::BadRecord(format!(
            "a key of {} bytes, over {MAX_COLLECTION_KEY_BYTES}",
            key.len()
        )));
    }
    Ok(key)
}

/// `(name, kind, rest..)`, checked.
fn collection_key(name: &str, kind: &str, rest: &[KeyPart]) -> Result<Vec<u8>, CollError> {
    let mut parts = vec![str_part(name), str_part(kind)];
    parts.extend_from_slice(rest);
    checked(&parts)
}

/// The parts of a stored key after `base`.
fn key_after(stored: &[u8], base: &[u8]) -> Result<Vec<KeyPart>, CollError> {
    stored
        .strip_prefix(base)
        .and_then(decode_key)
        .ok_or_else(|| CollError::BadRecord("not a key of this collection".into()))
}

/// A stored count or tally: a CBOR integer.
fn integer(bytes: &[u8]) -> Result<i128, CollError> {
    match read_value(bytes) {
        Ok(Value::Integer(i)) => Ok(i128::from(i)),
        _ => Err(CollError::BadRecord(
            "a stored count is not an integer".into(),
        )),
    }
}

/// The set named `name`, as written in `#[table]` or camelCased (the name in its keys),
/// with that key name.
fn named_set<'s>(
    sets: &'s std::collections::BTreeMap<String, Vec<String>>,
    name: &str,
) -> Result<(String, &'s [String]), CollError> {
    sets.iter()
        .find(|(set, _)| *set == name || camel(set) == name)
        .map(|(set, fields)| (camel(set), fields.as_slice()))
        .ok_or_else(|| CollError::UnknownCollection(name.to_owned()))
}

fn check_prefix(prefix: &[KeyPart], fields: usize) -> Result<(), CollError> {
    if prefix.len() > fields {
        return Err(CollError::BadRecord(format!(
            "a prefix of {} parts, at most {fields}",
            prefix.len()
        )));
    }
    Ok(())
}

/// A record's fields: its CBOR map entries.
type Fields = Vec<(Value, Value)>;

/// A record's canonical bytes and its fields; it must be a CBOR map.
fn parse_record(bytes: &[u8]) -> Result<(Vec<u8>, Fields), CollError> {
    let value = read_value(bytes)
        .and_then(canonical)
        .map_err(CollError::BadRecord)?;
    let bytes = write_value(&value).map_err(CollError::BadRecord)?;
    match value {
        Value::Map(fields) => Ok((bytes, fields)),
        _ => Err(CollError::BadRecord("a record is a CBOR map".into())),
    }
}

/// Every key a record owns under `table` (as `Table::entries`).
fn entries(
    table: &str,
    schema: &TableSchema,
    record: &[(Value, Value)],
) -> Result<Entries, CollError> {
    let parts = |fields: &[String]| -> Result<Vec<KeyPart>, CollError> {
        fields
            .iter()
            .map(|f| field_part(record, f).map_err(CollError::BadRecord))
            .collect()
    };
    let key = parts(&schema.key)?;
    let mut indexes = Vec::with_capacity(schema.indexes.len());
    for (name, fields) in &schema.indexes {
        let mut rest = vec![str_part(&camel(name))];
        rest.extend(parts(fields)?);
        rest.extend(key.iter().cloned());
        indexes.push(collection_key(table, "i", &rest)?);
    }
    let mut uniques = Vec::with_capacity(schema.unique.len());
    for (name, fields) in &schema.unique {
        let fields = parts(fields)?;
        // SQL NULL semantics: an entry with a None part is neither written nor checked.
        if fields.contains(&KeyPart::None) {
            continue;
        }
        let name = camel(name);
        let mut rest = vec![str_part(&name)];
        rest.extend(fields);
        uniques.push((name, collection_key(table, "u", &rest)?));
    }
    let mut counts = Vec::with_capacity(schema.counts.len());
    for (name, fields) in &schema.counts {
        let mut rest = vec![str_part(&camel(name))];
        rest.extend(parts(fields)?);
        counts.push(collection_key(table, "c", &rest)?);
    }
    Ok(Entries {
        record: collection_key(table, "r", &key)?,
        indexes,
        uniques,
        counts,
    })
}

impl HostCore {
    /// Fuel the run's collection calls cost so far; the engine adds it to the guest's.
    #[must_use]
    pub fn host_fuel(&self) -> u64 {
        self.host_fuel
    }

    #[cfg(test)]
    pub(crate) fn write_count(&self) -> u32 {
        self.writes
    }

    /// Insert a record (deterministic CBOR map), refusing a taken key (`Duplicate("key")`)
    /// or unique entry (`Duplicate(<camelCase name>)`).
    pub fn table_insert(&mut self, table: &str, record: &[u8]) -> Answer<()> {
        self.metered(true, |host| host.put_record(table, record, false))
    }

    /// Insert or replace a record; unique entries still apply.
    pub fn table_upsert(&mut self, table: &str, record: &[u8]) -> Answer<()> {
        self.metered(true, |host| host.put_record(table, record, true))
    }

    /// The record with this key.
    pub fn table_get(&mut self, table: &str, key: &[KeyPart]) -> Answer<Option<Vec<u8>>> {
        self.metered(false, |host| {
            let _schema = soft!(host.table_schema(table));
            let record_key = soft!(collection_key(table, "r", key));
            Ok(Ok(host.coll_get(&record_key)?))
        })
    }

    /// Remove the record with this key and its index, unique and count entries.
    pub fn table_remove(&mut self, table: &str, key: &[KeyPart]) -> Answer<bool> {
        self.metered(true, |host| {
            let schema = soft!(host.table_schema(table));
            let record_key = soft!(collection_key(table, "r", key));
            let Some(stored) = host.coll_get(&record_key)? else {
                return Ok(Ok(false));
            };
            let (_, fields) = soft!(parse_record(&stored));
            let old = soft!(entries(table, &schema, &fields));
            host.coll_delete(&old.record)?;
            host.reindex(Some(&old), None)?;
            Ok(Ok(true))
        })
    }

    /// Records whose index fields (or, with no index, key fields) start with `prefix`, in
    /// that order, one page of at most [`MAX_COLLECTION_LIMIT`] at a time. Each entry is
    /// the record's key and its bytes.
    pub fn table_find(
        &mut self,
        table: &str,
        index: Option<&str>,
        prefix: &[KeyPart],
        cursor: Option<&str>,
        limit: u32,
        reverse: bool,
    ) -> Answer<CollPage> {
        self.metered(false, |host| {
            let schema = soft!(host.table_schema(table));
            let record_base = soft!(collection_key(table, "r", &[]));
            let Some(index) = index else {
                soft!(check_prefix(prefix, schema.key.len()));
                let base = soft!(collection_key(table, "r", prefix));
                let (found, next) = soft!(host.read_page(&base, cursor, limit, reverse)?);
                let mut items = Vec::with_capacity(found.len());
                for (k, value) in found {
                    let key = soft!(key_after(&k, &record_base));
                    items.push(CollEntry { key, value });
                }
                return Ok(Ok(CollPage { items, next }));
            };
            let (name, fields) = soft!(named_set(&schema.indexes, index));
            soft!(check_prefix(prefix, fields.len()));
            let mut rest = vec![str_part(&name)];
            rest.extend_from_slice(prefix);
            let base = soft!(collection_key(table, "i", &rest));
            let (found, next) = soft!(host.read_page(&base, cursor, limit, reverse)?);
            let mut items = Vec::with_capacity(found.len());
            for (_, record_key) in found {
                let Some(value) = host.coll_get(&record_key)? else {
                    return Ok(Err(CollError::BadRecord("a dangling index entry".into())));
                };
                let key = soft!(key_after(&record_key, &record_base));
                items.push(CollEntry { key, value });
            }
            Ok(Ok(CollPage { items, next }))
        })
    }

    /// The maintained count `count` for the records matching `prefix`: one read when the
    /// prefix names every field, else the sum of the counts under it.
    pub fn table_count(&mut self, table: &str, count: &str, prefix: &[KeyPart]) -> Answer<u64> {
        self.metered(false, |host| {
            let schema = soft!(host.table_schema(table));
            let (name, fields) = soft!(named_set(&schema.counts, count));
            soft!(check_prefix(prefix, fields.len()));
            let mut rest = vec![str_part(&name)];
            rest.extend_from_slice(prefix);
            let base = soft!(collection_key(table, "c", &rest));
            let sum = if prefix.len() == fields.len() {
                match host.coll_get(&base)? {
                    Some(bytes) => soft!(integer(&bytes)),
                    None => 0,
                }
            } else {
                let end = [base.as_slice(), &[0xFF]].concat();
                soft!(host.coll_sum(&base, &end)?)
            };
            Ok(u64::try_from(sum)
                .map_err(|_| CollError::BadRecord(format!("count {name} is negative"))))
        })
    }

    /// The value at `key`.
    pub fn map_get(&mut self, map: &str, key: &[KeyPart]) -> Answer<Option<Vec<u8>>> {
        self.metered(false, |host| {
            soft!(host.expect_kind(map, &CollectionSchema::Map));
            let k = soft!(collection_key(map, "m", key));
            Ok(Ok(host.coll_get(&k)?))
        })
    }

    /// Set the value at `key` (canonicalized), returning the value it replaces; a new key
    /// adds one to the map's count.
    pub fn map_insert(
        &mut self,
        map: &str,
        key: &[KeyPart],
        value: &[u8],
    ) -> Answer<Option<Vec<u8>>> {
        self.metered(true, |host| {
            soft!(host.expect_kind(map, &CollectionSchema::Map));
            soft!(check_wit_key(key).map_err(CollError::BadRecord));
            let k = soft!(collection_key(map, "m", key));
            let count = soft!(collection_key(map, "n", &[]));
            let value = soft!(canonical_value(value).map_err(CollError::BadRecord));
            let old = host.coll_get(&k)?;
            host.coll_put(&k, &value)?;
            if old.is_none() {
                host.coll_add(&count, 1)?;
            }
            Ok(Ok(old))
        })
    }

    /// Remove `key`, returning the value it held; one less in the map's count.
    pub fn map_remove(&mut self, map: &str, key: &[KeyPart]) -> Answer<Option<Vec<u8>>> {
        self.metered(true, |host| {
            soft!(host.expect_kind(map, &CollectionSchema::Map));
            let k = soft!(collection_key(map, "m", key));
            let count = soft!(collection_key(map, "n", &[]));
            let Some(old) = host.coll_get(&k)? else {
                return Ok(Ok(None));
            };
            host.coll_delete(&k)?;
            host.coll_add(&count, -1)?;
            Ok(Ok(Some(old)))
        })
    }

    /// Entries whose key starts with the whole parts `prefix`, in key order, one page of
    /// at most [`MAX_COLLECTION_LIMIT`] at a time (the SDK's `UnorderedMap::prefix`).
    pub fn map_page(
        &mut self,
        map: &str,
        prefix: &[KeyPart],
        cursor: Option<&str>,
        limit: u32,
        reverse: bool,
    ) -> Answer<CollPage> {
        self.metered(false, |host| {
            soft!(host.expect_kind(map, &CollectionSchema::Map));
            let map_base = soft!(collection_key(map, "m", &[]));
            let base = soft!(within([map_base.as_slice(), &encode_key(prefix)].concat()));
            let (found, next) = soft!(host.read_page(&base, cursor, limit, reverse)?);
            let mut items = Vec::with_capacity(found.len());
            for (k, value) in found {
                let key = soft!(key_after(&k, &map_base));
                items.push(CollEntry { key, value });
            }
            Ok(Ok(CollPage { items, next }))
        })
    }

    /// Add `delta` to the counter at `key` with `storage.add` (D47).
    pub fn tally_add(&mut self, tally: &str, key: &[KeyPart], delta: i64) -> Answer<()> {
        self.metered(true, |host| {
            soft!(host.expect_kind(tally, &CollectionSchema::Tally));
            soft!(check_wit_key(key).map_err(CollError::BadRecord));
            let k = soft!(collection_key(tally, "t", key));
            host.coll_add(&k, delta)?;
            Ok(Ok(()))
        })
    }

    /// The counter at `key`; 0 when nothing was added.
    pub fn tally_get(&mut self, tally: &str, key: &[KeyPart]) -> Answer<i64> {
        self.metered(false, |host| {
            soft!(host.expect_kind(tally, &CollectionSchema::Tally));
            let k = soft!(collection_key(tally, "t", key));
            let n = match host.coll_get(&k)? {
                Some(bytes) => soft!(integer(&bytes)),
                None => 0,
            };
            Ok(i64::try_from(n)
                .map_err(|_| CollError::BadRecord(format!("tally {tally} out of range"))))
        })
    }

    // --- internals -------------------------------------------------------------------

    /// Runs one collection call: the interface's policy, then `call`, then its fuel
    /// (the one place a collection call is charged, whatever it answered).
    fn metered<T>(&mut self, write: bool, call: impl FnOnce(&mut Self) -> Answer<T>) -> Answer<T> {
        self.meter = (0, 0);
        let answer = match self.may_use_collections(write) {
            Ok(()) => call(self),
            Err(denied) => Err(denied),
        };
        let (bytes, entries) = std::mem::take(&mut self.meter);
        self.host_fuel = self
            .host_fuel
            .saturating_add(collection_fuel(bytes, entries));
        answer
    }

    /// Collections are closed in migrations and actions; writes are for logged runs only,
    /// refused before any soft error so a view can never learn from a write.
    fn may_use_collections(&self, write: bool) -> Result<(), HostDenied> {
        if matches!(
            self.inv.kind,
            InvocationKind::Migrate | InvocationKind::Action
        ) {
            return Err(self.denied("collections"));
        }
        if write && !self.is_logged_write() {
            return Err(self.denied("collection writes"));
        }
        Ok(())
    }

    fn lookup(&self, name: &str) -> Result<&CollectionSchema, CollError> {
        self.collections
            .get(name)
            .ok_or_else(|| CollError::UnknownCollection(name.to_owned()))
    }

    fn table_schema(&self, name: &str) -> Result<TableSchema, CollError> {
        match self.lookup(name)? {
            CollectionSchema::Table(schema) => Ok(schema.clone()),
            other => Err(CollError::WrongKind(other.kind().to_owned())),
        }
    }

    fn expect_kind(&self, name: &str, kind: &CollectionSchema) -> Result<(), CollError> {
        let found = self.lookup(name)?;
        if found != kind {
            return Err(CollError::WrongKind(found.kind().to_owned()));
        }
        Ok(())
    }

    fn put_record(&mut self, table: &str, record: &[u8], upsert: bool) -> Answer<()> {
        let schema = soft!(self.table_schema(table));
        let (bytes, fields) = soft!(parse_record(record));
        let new = soft!(entries(table, &schema, &fields));
        let old = match self.coll_get(&new.record)? {
            Some(_) if !upsert => return Ok(Err(CollError::Duplicate("key".into()))),
            Some(stored) => {
                let (_, old_fields) = soft!(parse_record(&stored));
                Some(soft!(entries(table, &schema, &old_fields)))
            }
            None => None,
        };
        for (name, key) in &new.uniques {
            if let Some(owner) = self.coll_get(key)? {
                if owner != new.record {
                    return Ok(Err(CollError::Duplicate(name.clone())));
                }
            }
        }
        self.coll_put(&new.record, &bytes)?;
        self.reindex(old.as_ref(), Some(&new))?;
        Ok(Ok(()))
    }

    /// Move the index, unique and count entries from `old` to `new` (as `Table::reindex`).
    fn reindex(&mut self, old: Option<&Entries>, new: Option<&Entries>) -> Result<(), HostDenied> {
        if let Some(old) = old {
            for k in &old.indexes {
                if !new.is_some_and(|n| n.indexes.contains(k)) {
                    self.coll_delete(k)?;
                }
            }
            for (_, k) in &old.uniques {
                if !new.is_some_and(|n| n.uniques.iter().any(|(_, nk)| nk == k)) {
                    self.coll_delete(k)?;
                }
            }
            for k in &old.counts {
                if !new.is_some_and(|n| n.counts.contains(k)) {
                    self.coll_add(k, -1)?;
                }
            }
        }
        if let Some(new) = new {
            for k in &new.indexes {
                if !old.is_some_and(|o| o.indexes.contains(k)) {
                    self.coll_put(k, &new.record)?;
                }
            }
            for (_, k) in &new.uniques {
                self.coll_put(k, &new.record)?;
            }
            for k in &new.counts {
                if !old.is_some_and(|o| o.counts.contains(k)) {
                    self.coll_add(k, 1)?;
                }
            }
        }
        Ok(())
    }

    /// Up to `limit + 1` entries under `base`, the page cut at `limit` (capped at
    /// [`MAX_COLLECTION_LIMIT`]); the cursor is the base64url of the previous page's last
    /// key and must lie under `base` (as the SDK's `read_page`).
    fn read_page(
        &mut self,
        base: &[u8],
        cursor: Option<&str>,
        limit: u32,
        reverse: bool,
    ) -> Answer<(Vec<KeyValue>, Option<String>)> {
        let limit = limit.min(MAX_COLLECTION_LIMIT) as usize;
        if limit == 0 {
            return Ok(Ok((Vec::new(), None)));
        }
        let mut start = base.to_vec();
        let mut end = [base, &[0xFF]].concat();
        if let Some(cursor) = cursor {
            let Ok(last) = URL_SAFE_NO_PAD.decode(cursor) else {
                return Ok(Err(CollError::BadCursor));
            };
            if last.len() > MAX_COLLECTION_KEY_BYTES || !last.starts_with(base) {
                return Ok(Err(CollError::BadCursor));
            }
            if reverse {
                end = last;
            } else {
                // The smallest key after `last`.
                start = [last.as_slice(), &[0x00]].concat();
            }
        }
        let mut found = self.coll_range(&start, &end, reverse, limit + 1)?;
        let next = if found.len() > limit {
            found.truncate(limit);
            found.last().map(|(k, _)| URL_SAFE_NO_PAD.encode(k))
        } else {
            None
        };
        Ok(Ok((found, next)))
    }

    // Storage through the guest's own primitives, metered.

    fn measure(&mut self, bytes: usize, entries: usize) {
        self.meter.0 = self.meter.0.saturating_add(bytes);
        self.meter.1 = self.meter.1.saturating_add(entries);
    }

    fn coll_get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, HostDenied> {
        let value = self.storage_get(key)?;
        self.measure(key.len() + value.as_ref().map_or(0, Vec::len), 1);
        Ok(value)
    }

    fn coll_put(&mut self, key: &[u8], value: &[u8]) -> Result<(), HostDenied> {
        self.storage_put(key, value)?;
        self.measure(key.len() + value.len(), 1);
        Ok(())
    }

    fn coll_delete(&mut self, key: &[u8]) -> Result<(), HostDenied> {
        self.storage_delete(key)?;
        self.measure(key.len(), 1);
        Ok(())
    }

    fn coll_add(&mut self, key: &[u8], delta: i64) -> Result<(), HostDenied> {
        self.storage_add(key, delta)?;
        let written = self
            .current(&stored_key(GENERATION, key))
            .map_or(0, |v| v.len());
        self.measure(key.len() + written, 1);
        Ok(())
    }

    /// The sum of the stored integers in `[start, end)`, read through one cursor a batch of
    /// [`MAX_RANGE_BATCH`] at a time (never more held), each entry metered as it is read.
    fn coll_sum(&mut self, start: &[u8], end: &[u8]) -> Answer<i128> {
        let cursor = self.range_open(start, end, false)?;
        let mut sum = 0i128;
        let read = loop {
            let batch = match self.range_next(cursor, MAX_RANGE_BATCH) {
                Ok(batch) => batch,
                Err(denied) => break Err(denied),
            };
            if batch.is_empty() {
                break Ok(Ok(sum));
            }
            let bytes = batch.iter().map(|(k, v)| k.len() + v.len()).sum();
            self.measure(bytes, batch.len());
            let mut bad = None;
            for (_, value) in &batch {
                match integer(value) {
                    Ok(n) => sum = sum.saturating_add(n),
                    Err(error) => {
                        bad = Some(error);
                        break;
                    }
                }
            }
            if let Some(error) = bad {
                break Ok(Err(error));
            }
        };
        self.range_close(cursor);
        read
    }

    /// Up to `want` entries of `[start, end)` through one cursor, closed before returning.
    fn coll_range(
        &mut self,
        start: &[u8],
        end: &[u8],
        reverse: bool,
        want: usize,
    ) -> Result<Vec<KeyValue>, HostDenied> {
        let cursor = self.range_open(start, end, reverse)?;
        let mut found: Vec<KeyValue> = Vec::new();
        let read = loop {
            let left = want - found.len();
            if left == 0 {
                break Ok(());
            }
            let max = u32::try_from(left).unwrap_or(u32::MAX).min(MAX_RANGE_BATCH);
            match self.range_next(cursor, max) {
                Ok(batch) if batch.is_empty() => break Ok(()),
                Ok(batch) => found.extend(batch),
                Err(denied) => break Err(denied),
            }
        };
        self.range_close(cursor);
        read?;
        let bytes = found.iter().map(|(k, v)| k.len() + v.len()).sum();
        self.measure(bytes, found.len());
        Ok(found)
    }
}
