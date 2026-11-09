//! Host collections (PoC, feature `host-collections`, app components only): the calls of
//! `calimero:app/collections` behind `Table`, `UnorderedMap` and `Tally` on the default
//! store. The host writes the keys and bytes the guest path writes, so the two read each
//! other's state.
//!
//! Every function answers `None` when the host does not serve the name (a collection the
//! state does not declare, say one built by hand with `new`): the caller then takes the
//! guest path, which stores at the same keys.

use crate::bindings::calimero::app::collections as wit;
use crate::encoding::key::KeyPart;
use crate::error::{StorageError, TableError};
use crate::table::{FindOpts, DEFAULT_LIMIT};

/// A host answer, or `None` for the guest path.
pub(crate) type Host<T, E = StorageError> = Option<Result<T, E>>;

/// One entry of a page: the key after the collection's base and the stored bytes.
pub(crate) type HostEntry = (Vec<KeyPart>, Vec<u8>);

/// A page of entries and the cursor of the next.
pub(crate) type HostPage = (Vec<HostEntry>, Option<String>);

/// Runs a host call for the collection `name`; `None` when the host has no such collection.
fn served<T>(name: &str, answer: Result<T, wit::CollError>) -> Host<T, TableError> {
    match answer {
        Ok(value) => Some(Ok(value)),
        Err(wit::CollError::UnknownCollection(n)) if n == name => None,
        Err(error) => Some(Err(table_error(name, error))),
    }
}

/// [`served`] for a call that cannot clash (`Duplicate` is not expected outside inserts).
fn storage<T>(name: &str, answer: Result<T, wit::CollError>) -> Host<T> {
    served(name, answer).map(|r| r.map_err(into_storage))
}

fn into_storage(error: TableError) -> StorageError {
    match error {
        TableError::Storage(s) => s,
        TableError::Duplicate { index } => StorageError::Encode(format!("duplicate on `{index}`")),
    }
}

fn table_error(name: &str, error: wit::CollError) -> TableError {
    match error {
        wit::CollError::Duplicate(index) => TableError::Duplicate { index },
        wit::CollError::BadRecord(message) => StorageError::Encode(message).into(),
        wit::CollError::BadCursor => StorageError::BadCursor.into(),
        wit::CollError::UnknownCollection(n) => StorageError::UnknownName(n).into(),
        wit::CollError::WrongKind(kind) => {
            StorageError::Decode(format!("`{name}` is a {kind} in the state")).into()
        }
    }
}

/// A key as the WIT carries it: an integer as `int` when it fits an `s64`, else `uint`; one
/// outside both has no WIT form and is refused (the guest path would store it).
fn wit_key(parts: &[KeyPart]) -> Result<Vec<wit::KeyPart>, StorageError> {
    parts
        .iter()
        .map(|part| {
            Ok(match part {
                KeyPart::None => wit::KeyPart::None,
                KeyPart::Bool(b) => wit::KeyPart::Boolean(*b),
                KeyPart::Int(i) => match (i64::try_from(*i), u64::try_from(*i)) {
                    (Ok(i), _) => wit::KeyPart::Int(i),
                    (_, Ok(u)) => wit::KeyPart::Uint(u),
                    _ => {
                        return Err(StorageError::Encode(format!(
                            "key integer {i} is outside s64 and u64"
                        )))
                    }
                },
                KeyPart::Str(s) => wit::KeyPart::Text(s.clone()),
                KeyPart::Bytes(b) => wit::KeyPart::Bytes(b.clone()),
            })
        })
        .collect()
}

fn sdk_key(parts: Vec<wit::KeyPart>) -> Vec<KeyPart> {
    parts
        .into_iter()
        .map(|part| match part {
            wit::KeyPart::None => KeyPart::None,
            wit::KeyPart::Boolean(b) => KeyPart::Bool(b),
            wit::KeyPart::Int(i) => KeyPart::Int(i.into()),
            wit::KeyPart::Uint(u) => KeyPart::Int(u.into()),
            wit::KeyPart::Text(s) => KeyPart::Str(s),
            wit::KeyPart::Bytes(b) => KeyPart::Bytes(b),
        })
        .collect()
}

/// `?` for a key conversion inside a function answering [`Host`].
macro_rules! key {
    ($parts:expr) => {
        match wit_key($parts) {
            Ok(key) => key,
            Err(error) => return Some(Err(error.into())),
        }
    };
}

/// The page options the host takes; `None` (guest path) for `from`/`to`, which it lacks.
fn page_opts(opts: &FindOpts) -> Option<(Option<&str>, u32, bool)> {
    if opts.from.is_some() || opts.to.is_some() {
        return None;
    }
    // The host reads `limit` 0 as an empty page, so the SDK's default is given here.
    let limit = opts.limit.unwrap_or(DEFAULT_LIMIT);
    Some((opts.cursor.as_deref(), limit, opts.reverse))
}

fn page(page: wit::Page) -> HostPage {
    let items = page
        .items
        .into_iter()
        .map(|e| (sdk_key(e.key), e.value))
        .collect();
    (items, page.next)
}

// --- tables --------------------------------------------------------------------------

/// Insert (or with `upsert`, insert or replace) a record given as its CBOR bytes.
pub(crate) fn table_put(table: &str, record: &[u8], upsert: bool) -> Host<(), TableError> {
    let answer = if upsert {
        wit::table_upsert(table, record)
    } else {
        wit::table_insert(table, record)
    };
    served(table, answer)
}

/// The record bytes at `key`.
pub(crate) fn table_get(table: &str, key: &[KeyPart]) -> Host<Option<Vec<u8>>> {
    let key = key!(key);
    storage(table, wit::table_get(table, &key))
}

/// Remove the record at `key`; whether it was there.
pub(crate) fn table_remove(table: &str, key: &[KeyPart]) -> Host<bool> {
    let key = key!(key);
    storage(table, wit::table_remove(table, &key))
}

/// A page of records by index (or by key with `index` none); `None` for `from`/`to`.
pub(crate) fn table_find(
    table: &str,
    index: Option<&str>,
    prefix: &[KeyPart],
    opts: &FindOpts,
) -> Host<HostPage> {
    let (cursor, limit, reverse) = page_opts(opts)?;
    let prefix = key!(prefix);
    storage(
        table,
        wit::table_find(table, index, &prefix, cursor, limit, reverse).map(page),
    )
}

/// The count `count` under `prefix`.
pub(crate) fn table_count(table: &str, count: &str, prefix: &[KeyPart]) -> Host<u64> {
    let prefix = key!(prefix);
    storage(table, wit::table_count(table, count, &prefix))
}

// --- maps ----------------------------------------------------------------------------

pub(crate) fn map_get(map: &str, key: &[KeyPart]) -> Host<Option<Vec<u8>>> {
    let key = key!(key);
    storage(map, wit::map_get(map, &key))
}

/// Set `key` to the CBOR `value`; the bytes it replaces.
pub(crate) fn map_insert(map: &str, key: &[KeyPart], value: &[u8]) -> Host<Option<Vec<u8>>> {
    let key = key!(key);
    storage(map, wit::map_insert(map, &key, value))
}

pub(crate) fn map_remove(map: &str, key: &[KeyPart]) -> Host<Option<Vec<u8>>> {
    let key = key!(key);
    storage(map, wit::map_remove(map, &key))
}

/// A page of entries whose key starts with the whole parts `prefix`; `None` for `from`/`to`.
pub(crate) fn map_page(map: &str, prefix: &[KeyPart], opts: &FindOpts) -> Host<HostPage> {
    let (cursor, limit, reverse) = page_opts(opts)?;
    let prefix = key!(prefix);
    storage(
        map,
        wit::map_page(map, &prefix, cursor, limit, reverse).map(page),
    )
}

// --- tallies -------------------------------------------------------------------------

pub(crate) fn tally_add(tally: &str, key: &[KeyPart], delta: i64) -> Host<()> {
    let key = key!(key);
    storage(tally, wit::tally_add(tally, &key, delta))
}

pub(crate) fn tally_get(tally: &str, key: &[KeyPart]) -> Host<i64> {
    let key = key!(key);
    storage(tally, wit::tally_get(tally, &key))
}
