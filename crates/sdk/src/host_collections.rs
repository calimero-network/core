//! Host collections (PoC, feature `host-collections`, app components only): the calls of
//! `calimero:app/collections` behind `Table`, `UnorderedMap` and `Tally` on the default
//! store. The host writes the keys and bytes the guest path writes, so the two read each
//! other's state.
//!
//! Records and map values cross the interface as JSON text ([`to_json`]); the host stores
//! them as the guest path's CBOR by the types the ABI declares. A record reads its own JSON
//! back as is (its derive writes and reads 64-bit fields as decimal strings); a map value
//! of a plain serde type gets its 64-bit integers back as decimal strings, which
//! [`map_value`] turns into numbers by the map's value type from the state ABI
//! ([`map_value_types`], registered by `AppState::load`).
//!
//! Every function answers `None` when the host does not serve the name (a collection the
//! state does not declare, say one built by hand with `new`): the caller then takes the
//! guest path, which stores at the same keys. The first such answer for a name is logged
//! ([`GUEST_PATH_LOG`]) and remembered, so the rest of the invocation goes straight to the
//! guest path without asking the host again: an invocation runs in a fresh instance, so
//! the memory lasts exactly one invocation.
//!
//! An operation the host does not serve on a collection it does serve (a page bounded by
//! `from`/`to`, a map's `len`, `clear` or `starts_with`, a table's `search`) also takes the
//! guest path, logged once per collection and reason ([`guest_only`]:
//! `` host-collections: guest path for `<name>` (<reason>) ``) without moving the
//! collection's other calls off the host. Every guest-path access of a host build is
//! therefore in the invocation's log.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::bindings::calimero::app::collections as wit;
use crate::encoding::key::KeyPart;
use crate::error::{StorageError, TableError};
use crate::table::{FindOpts, DEFAULT_LIMIT};

/// The start of the log line written the first time an invocation finds that the host
/// does not serve a collection (followed by its name in backticks).
pub(crate) const GUEST_PATH_LOG: &str = "host-collections: guest path for ";

thread_local! {
    /// The collections the host answered `unknown-collection` for in this invocation.
    static GUEST_PATH: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    /// `(collection, reason)` already logged by [`guest_only`] in this invocation.
    static GUEST_ONLY: RefCell<BTreeSet<(String, String)>> = const { RefCell::new(BTreeSet::new()) };
    /// The state's maps and their value types (ABI names), from [`map_value_types`].
    static MAP_VALUES: Cell<&'static [(&'static str, &'static str)]> = const { Cell::new(&[]) };
}

/// Remember the state's maps and their value types (`#[app::state]` passes them in `load`).
pub(crate) fn map_value_types(maps: &'static [(&'static str, &'static str)]) {
    MAP_VALUES.with(|m| m.set(maps));
}

/// A record or map value as the JSON text the host takes.
pub(crate) fn to_json<T: Serialize + ?Sized>(value: &T) -> Result<String, StorageError> {
    serde_json::to_string(value).map_err(|e| StorageError::Encode(e.to_string()))
}

/// A record from the host's JSON text: its derive reads 64-bit fields as decimal strings.
pub(crate) fn from_json<T: DeserializeOwned>(text: &str) -> Result<T, StorageError> {
    serde_json::from_str(text).map_err(|e| StorageError::Decode(e.to_string()))
}

/// A value of the map `map` from the host's JSON text: the 64-bit integers the host wrote as
/// decimal strings become numbers again, by the map's value type, before `V` reads them.
pub(crate) fn map_value<V: DeserializeOwned>(map: &str, text: &str) -> Result<V, StorageError> {
    let mut json: Value =
        serde_json::from_str(text).map_err(|e| StorageError::Decode(e.to_string()))?;
    let ty = MAP_VALUES.with(|m| {
        m.get()
            .iter()
            .find(|(name, _)| *name == map)
            .map(|(_, ty)| *ty)
    });
    if let Some(ty) = ty {
        numbers(ty, &mut json)?;
    }
    serde_json::from_value(json).map_err(|e| StorageError::Decode(e.to_string()))
}

/// Turn the decimal strings at the `u64`/`i64` positions of `ty` (an ABI type name:
/// `option<T>`, `list<T>`, `tuple<A, ..>`) into JSON numbers; other positions are left.
fn numbers(ty: &str, json: &mut Value) -> Result<(), StorageError> {
    let ty = ty.trim();
    let generic = |head: &str| ty.strip_prefix(head)?.strip_suffix('>');
    match json {
        Value::String(s) if ty == "u64" || ty == "i64" => {
            let n = if ty == "u64" {
                s.parse::<u64>().map(Value::from)
            } else {
                s.parse::<i64>().map(Value::from)
            };
            *json = n.map_err(|e| StorageError::Decode(format!("{ty} {s:?}: {e}")))?;
        }
        Value::Null => {}
        Value::Array(items) => {
            if let Some(inner) = generic("option<") {
                return numbers(inner, json);
            }
            if let Some(inner) = generic("list<") {
                for item in items {
                    numbers(inner, item)?;
                }
            } else if let Some(inner) = generic("tuple<") {
                for (ty, item) in split_top(inner).into_iter().zip(items) {
                    numbers(ty, item)?;
                }
            }
        }
        _ => {
            if let Some(inner) = generic("option<") {
                return numbers(inner, json);
            }
        }
    }
    Ok(())
}

/// `a, option<b>, tuple<c, d>` split at its top-level commas.
fn split_top(list: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in list.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&list[start..]);
    parts
}

/// Whether `name` is already known to be served by the guest path.
fn on_guest_path(name: &str) -> bool {
    GUEST_PATH.with(|names| names.borrow().contains(name))
}

/// Remember (and log once) that the host does not serve `name`.
fn guest_path(name: &str) {
    let first = GUEST_PATH.with(|names| names.borrow_mut().insert(name.to_owned()));
    if first {
        crate::env::log(&format!("{GUEST_PATH_LOG}`{name}`"));
    }
}

/// Log (once per collection and reason in this invocation) that `name`, a collection the
/// host may serve, is accessed by the guest path for `why`: an operation the host lacks.
/// Unlike [`guest_path`] it does not route the collection's later calls away from the host.
/// A collection already on the guest path was logged when it went there.
pub(crate) fn guest_only(name: &str, why: &str) {
    if on_guest_path(name) {
        return;
    }
    let first = GUEST_ONLY.with(|seen| seen.borrow_mut().insert((name.to_owned(), why.to_owned())));
    if first {
        crate::env::log(&format!("{GUEST_PATH_LOG}`{name}` ({why})"));
    }
}

/// `?` for [`on_guest_path`] at the top of a function answering [`Host`].
macro_rules! served_by_host {
    ($name:expr) => {
        if on_guest_path($name) {
            return None;
        }
    };
}

/// A host answer, or `None` for the guest path.
pub(crate) type Host<T, E = StorageError> = Option<Result<T, E>>;

/// One entry of a page: the key after the collection's base and the value as JSON.
pub(crate) type HostEntry = (Vec<KeyPart>, String);

/// A page of entries and the cursor of the next.
pub(crate) type HostPage = (Vec<HostEntry>, Option<String>);

/// Runs a host call for the collection `name`; `None` when the host has no such collection.
fn served<T>(name: &str, answer: Result<T, wit::CollError>) -> Host<T, TableError> {
    match answer {
        Ok(value) => Some(Ok(value)),
        Err(wit::CollError::UnknownCollection(n)) if n == name => {
            guest_path(name);
            None
        }
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
        wit::CollError::BadRecord(message) => key_too_long(&message)
            .unwrap_or(StorageError::Encode(message))
            .into(),
        wit::CollError::BadCursor => StorageError::BadCursor.into(),
        // An unknown index, unique or count of a served table: an error, never a fallback.
        wit::CollError::UnknownCollection(n) | wit::CollError::UnknownSet(n) => {
            StorageError::UnknownName(n).into()
        }
        wit::CollError::WrongKind(kind) => {
            StorageError::Decode(format!("`{name}` is a {kind} in the state")).into()
        }
    }
}

/// The host's refusal of an over-long key (`calimero-app-host` `collection_ops::within`:
/// "a key of {len} bytes, over {max}") as the guest path's `KeyTooLong`.
fn key_too_long(message: &str) -> Option<StorageError> {
    let (len, max) = message
        .strip_prefix("a key of ")?
        .split_once(" bytes, over ")?;
    Some(StorageError::KeyTooLong {
        len: len.parse().ok()?,
        max: max.parse().ok()?,
    })
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

/// The page options the host takes; `None` (guest path, logged) for `from`/`to`, which it
/// lacks.
fn page_opts<'o>(name: &str, opts: &'o FindOpts) -> Option<(Option<&'o str>, u32, bool)> {
    if opts.from.is_some() || opts.to.is_some() {
        guest_only(name, "from/to");
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

/// Insert (or with `upsert`, insert or replace) a record given as JSON ([`to_json`]).
pub(crate) fn table_put(table: &str, record: &str, upsert: bool) -> Host<(), TableError> {
    served_by_host!(table);
    let answer = if upsert {
        wit::table_upsert(table, record)
    } else {
        wit::table_insert(table, record)
    };
    served(table, answer)
}

/// The record at `key`, as JSON.
pub(crate) fn table_get(table: &str, key: &[KeyPart]) -> Host<Option<String>> {
    served_by_host!(table);
    let key = key!(key);
    storage(table, wit::table_get(table, &key))
}

/// Remove the record at `key`; whether it was there.
pub(crate) fn table_remove(table: &str, key: &[KeyPart]) -> Host<bool> {
    served_by_host!(table);
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
    served_by_host!(table);
    let (cursor, limit, reverse) = page_opts(table, opts)?;
    let prefix = key!(prefix);
    storage(
        table,
        wit::table_find(table, index, &prefix, cursor, limit, reverse).map(page),
    )
}

/// The count `count` under `prefix`.
pub(crate) fn table_count(table: &str, count: &str, prefix: &[KeyPart]) -> Host<u64> {
    served_by_host!(table);
    let prefix = key!(prefix);
    storage(table, wit::table_count(table, count, &prefix))
}

// --- maps ----------------------------------------------------------------------------

pub(crate) fn map_get(map: &str, key: &[KeyPart]) -> Host<Option<String>> {
    served_by_host!(map);
    let key = key!(key);
    storage(map, wit::map_get(map, &key))
}

/// Set `key` to the JSON `value`; the value it replaces, as JSON.
pub(crate) fn map_insert(map: &str, key: &[KeyPart], value: &str) -> Host<Option<String>> {
    served_by_host!(map);
    let key = key!(key);
    storage(map, wit::map_insert(map, &key, value))
}

pub(crate) fn map_remove(map: &str, key: &[KeyPart]) -> Host<Option<String>> {
    served_by_host!(map);
    let key = key!(key);
    storage(map, wit::map_remove(map, &key))
}

/// A page of entries whose key starts with the whole parts `prefix`; `None` for `from`/`to`.
pub(crate) fn map_page(map: &str, prefix: &[KeyPart], opts: &FindOpts) -> Host<HostPage> {
    served_by_host!(map);
    let (cursor, limit, reverse) = page_opts(map, opts)?;
    let prefix = key!(prefix);
    storage(
        map,
        wit::map_page(map, &prefix, cursor, limit, reverse).map(page),
    )
}

// --- tallies -------------------------------------------------------------------------

pub(crate) fn tally_add(tally: &str, key: &[KeyPart], delta: i64) -> Host<()> {
    served_by_host!(tally);
    let key = key!(key);
    storage(tally, wit::tally_add(tally, &key, delta))
}

pub(crate) fn tally_get(tally: &str, key: &[KeyPart]) -> Host<i64> {
    served_by_host!(tally);
    let key = key!(key);
    storage(tally, wit::tally_get(tally, &key))
}
