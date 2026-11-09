//! Host collections (PoC): the schemas the state ABI declares, and the key and value
//! encodings the Rust SDK writes, so `calimero:app/collections` stores exactly the keys and
//! bytes `Table`, `UnorderedMap` and `Tally` store today (D10).
//!
//! Layouts (as `calimero_sdk::table` and `calimero_sdk::collections` write them):
//!
//! - table: `(table, "r", key..)` record, `(table, "i", index, fields.., key..)` and
//!   `(table, "u", name, fields..)` hold the record's storage key, `(table, "c", name,
//!   fields..)` a count changed only with `storage.add`; set names in keys are camelCase;
//! - map (`unorderedMap`): `(map, "m", key..)` values, `(map, "n")` their count;
//! - tally: `(tally, "t", key..)` integers changed only with `storage.add`.
//!
//! The key encoder and the value canonicalizer are copies of the SDK's
//! (`calimero_sdk::encoding::{key, value}`): the SDK depends on this crate under `testing`, so
//! this crate cannot depend on it. The tests pin both against the SDK.

use std::collections::BTreeMap;

use ciborium::Value;

use crate::core::{MAX_KEY_BYTES, MAX_RUN_BYTES};

#[cfg(test)]
mod tests;

/// The largest page a find or page returns; a larger `limit` is cut to it.
pub const MAX_COLLECTION_LIMIT: u32 = 1_000;
/// The longest key a collection builds: one under the host's limit, so the bounds derived
/// from it (`last ‖ 0x00`, `prefix ‖ 0xFF`) still fit (the SDK's `MAX_SDK_KEY_BYTES`).
pub const MAX_COLLECTION_KEY_BYTES: usize = MAX_KEY_BYTES - 1;

/// The longest cursor a find or page accepts: the base64url (unpadded) of a
/// [`MAX_COLLECTION_KEY_BYTES`] key. A longer one is refused before it is decoded.
pub const MAX_CURSOR_CHARS: usize = MAX_COLLECTION_KEY_BYTES.div_ceil(3) * 4;
/// The deepest nesting of a guest record or map value (arrays, maps, tags), checked on the
/// raw bytes before they are parsed.
pub const MAX_CBOR_DEPTH: usize = 64;
/// The most CBOR items (every array or map element, map key and tag counts) a guest
/// record or map value may hold, checked on the raw bytes before they are parsed: the
/// parsed tree costs some 50 bytes of host heap per item, so this caps one call's tree at a
/// few MiB whatever the input. (The guest path stores any value; the host path refuses a
/// value with more items as `bad-record`.)
pub const MAX_CBOR_ITEMS: usize = 65_536;

/// Fuel of every collection call.
///
/// Calibrated against the guest code a host call replaces (PoC, measured 2026-11-09 by the
/// engine test `a_host_table_insert_costs_between_half_and_all_of_the_sdk_path`, release
/// `wasm32-wasip2` guest, wasmtime 50): `Table::insert` through the SDK of a 3-field record
/// (`id: u64`, `owner`, `note`) with one index and one count costs 28,077 guest fuel over an
/// empty method. The same insert through `table-insert` reads or writes 4 entries (record
/// lookup, record, index, count) and 199 bytes of keys and values, so it is charged
/// 5,000 + 199 x 10 + 4 x 3,500 = 20,990 host fuel, 0.75x the SDK path (the target is
/// 0.5x-1x). Every byte the guest hands over (names, records, values, keys, prefixes,
/// cursors) is charged [`COLLECTION_BYTE_FUEL`] too, whether or not it is then parsed
/// (final review fix wave): the table name and the 28-byte record add 34 x 10, so the
/// insert is now charged 21,330, 0.76x the 28,005 the SDK path took in the same rerun.
/// The guest still pays its own side of the call (encoding the record and the
/// canonical ABI copy): 12,250 fuel in that measurement.
pub const COLLECTION_BASE_FUEL: u64 = 5_000;
/// Fuel per byte of key or value read or written (see [`COLLECTION_BASE_FUEL`]).
pub const COLLECTION_BYTE_FUEL: u64 = 10;
/// Fuel per stored entry read or written (see [`COLLECTION_BASE_FUEL`]).
pub const COLLECTION_ENTRY_FUEL: u64 = 3_500;

/// The fuel of one collection call that read or wrote `bytes` of keys and values over
/// `entries` stored entries. Saturates.
#[must_use]
pub fn collection_fuel(bytes: usize, entries: usize) -> u64 {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    let entries = u64::try_from(entries).unwrap_or(u64::MAX);
    COLLECTION_BASE_FUEL
        .saturating_add(COLLECTION_BYTE_FUEL.saturating_mul(bytes))
        .saturating_add(COLLECTION_ENTRY_FUEL.saturating_mul(entries))
}

/// One collection of the app state, from the ABI's `state.collections`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionSchema {
    Table(TableSchema),
    /// `"unorderedMap"`.
    Map,
    /// `"tally"`.
    Tally,
    /// Any other kind; the host serves none of its calls.
    Other(String),
}

impl CollectionSchema {
    /// The kind as the ABI writes it.
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::Table(_) => "table",
            Self::Map => "unorderedMap",
            Self::Tally => "tally",
            Self::Other(kind) => kind,
        }
    }
}

/// A table's declared key, indexes, unique constraints and counts. Field names are the
/// record's CBOR (camelCase) names; set names are as written in `#[table(..)]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableSchema {
    pub key: Vec<String>,
    pub indexes: BTreeMap<String, Vec<String>>,
    pub unique: BTreeMap<String, Vec<String>>,
    pub counts: BTreeMap<String, Vec<String>>,
}

/// One element of a key tuple. Mirrors the WIT `key-part`: `int(s64)` and `uint(u64)` are
/// both `Int`, as the SDK keeps every integer as one 128-bit part.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyPart {
    None,
    Bool(bool),
    Int(i128),
    Str(String),
    Bytes(Vec<u8>),
}

/// A key and its value. Mirrors the WIT `entry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollEntry {
    pub key: Vec<KeyPart>,
    pub value: Vec<u8>,
}

/// One page of a find. Mirrors the WIT `page`; `next` is the base64url of the last
/// returned storage key, present when more follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollPage {
    pub items: Vec<CollEntry>,
    pub next: Option<String>,
}

/// A collection call's soft error. Mirrors the WIT `coll-error`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollError {
    /// The record's key (`"key"`) or a unique entry (its camelCase name) is taken.
    Duplicate(String),
    /// A record, value, key or prefix the call cannot use.
    BadRecord(String),
    /// A cursor not from this collection, index and prefix.
    BadCursor,
    /// No collection of this name in the state.
    UnknownCollection(String),
    /// The table has no index, unique or count of this name (the name in the error).
    UnknownSet(String),
    /// The collection is of another kind (named in the error).
    WrongKind(String),
}

// --- keys (a copy of `calimero_sdk::encoding::key`) ----------------------------------

const TAG_NONE: u8 = 0x00;
const TAG_BOOL: u8 = 0x10;
const TAG_INT: u8 = 0x20;
const TAG_STR: u8 = 0x30;
const TAG_BYTES: u8 = 0x40;

/// Encode a tuple of parts, order-preserving.
pub(crate) fn encode_key(parts: &[KeyPart]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        match part {
            KeyPart::None => out.push(TAG_NONE),
            KeyPart::Bool(b) => {
                out.push(TAG_BOOL);
                out.push(u8::from(*b));
            }
            KeyPart::Int(i) => {
                out.push(TAG_INT);
                // Flipping the sign bit makes two's complement sort as unsigned.
                let flipped = (*i as u128) ^ (1u128 << 127);
                out.extend_from_slice(&flipped.to_be_bytes());
            }
            KeyPart::Str(s) => {
                out.push(TAG_STR);
                escaped(s.as_bytes(), &mut out);
            }
            KeyPart::Bytes(b) => {
                out.push(TAG_BYTES);
                escaped(b, &mut out);
            }
        }
    }
    out
}

/// The length [`encode_key`] gives `parts`, computed without encoding them, so an over-long
/// key is refused before anything of its size is allocated.
pub(crate) fn encoded_len(parts: &[KeyPart]) -> usize {
    let escaped = |b: &[u8]| 1 + b.len() + b.iter().filter(|&&x| x == 0).count() + 2;
    parts
        .iter()
        .map(|part| match part {
            KeyPart::None => 1,
            KeyPart::Bool(_) => 2,
            KeyPart::Int(_) => 17,
            KeyPart::Str(s) => escaped(s.as_bytes()),
            KeyPart::Bytes(b) => escaped(b),
        })
        .fold(0usize, usize::saturating_add)
}

/// `0x00` becomes `0x00 0xFF`; the payload ends with `0x00 0x00`.
fn escaped(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        out.push(b);
        if b == 0 {
            out.push(0xFF);
        }
    }
    out.extend_from_slice(&[0x00, 0x00]);
}

/// Decode a tuple encoded by [`encode_key`]; `None` when the bytes are not one.
pub(crate) fn decode_key(mut bytes: &[u8]) -> Option<Vec<KeyPart>> {
    let mut parts = Vec::new();
    while let Some((&tag, rest)) = bytes.split_first() {
        let (part, rest) = match tag {
            TAG_NONE => (KeyPart::None, rest),
            TAG_BOOL => {
                let (&b, rest) = rest.split_first()?;
                let b = match b {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                (KeyPart::Bool(b), rest)
            }
            TAG_INT => {
                let (int, rest) = rest.split_first_chunk::<16>()?;
                let flipped = u128::from_be_bytes(*int) ^ (1u128 << 127);
                (KeyPart::Int(flipped as i128), rest)
            }
            TAG_STR => {
                let (raw, rest) = unescaped(rest)?;
                (KeyPart::Str(String::from_utf8(raw).ok()?), rest)
            }
            TAG_BYTES => {
                let (raw, rest) = unescaped(rest)?;
                (KeyPart::Bytes(raw), rest)
            }
            _ => return None,
        };
        parts.push(part);
        bytes = rest;
    }
    Some(parts)
}

/// The payload of an escaped part and what follows its `0x00 0x00` end.
fn unescaped(bytes: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let b = *bytes.get(i)?;
        if b != 0 {
            out.push(b);
            i += 1;
            continue;
        }
        match *bytes.get(i + 1)? {
            0x00 => return Some((out, bytes.get(i + 2..)?)),
            0xFF => {
                out.push(0);
                i += 2;
            }
            _ => return None,
        }
    }
}

/// `room_id` → `roomId`; `r#type` → `type` (a copy of the SDK macros' `camel`): how a set
/// name as written becomes the name in its keys.
pub(crate) fn camel(snake: &str) -> String {
    let snake = snake.strip_prefix("r#").unwrap_or(snake);
    let lead = snake.len() - snake.trim_start_matches('_').len();
    let mut out = String::from(&snake[..lead]);
    let mut upper = false;
    for c in snake[lead..].chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

// --- values (a copy of `calimero_sdk::encoding::value`) ------------------------------

/// One CBOR item, read whole: trailing bytes are refused.
pub(crate) fn read_value(bytes: &[u8]) -> Result<Value, String> {
    let mut rest = bytes;
    let value: Value = ciborium::from_reader(&mut rest).map_err(|e| format!("not CBOR: {e}"))?;
    if !rest.is_empty() {
        return Err("bytes after the CBOR value".into());
    }
    Ok(value)
}

/// One CBOR item from the guest: refused over [`MAX_RUN_BYTES`] (it could never be stored)
/// and, on the raw bytes before anything is parsed, past [`MAX_CBOR_DEPTH`] or
/// [`MAX_CBOR_ITEMS`] or with a length its bytes cannot hold, so a small input never
/// becomes a large tree.
pub(crate) fn read_guest_value(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_RUN_BYTES {
        return Err(format!(
            "a value of {} bytes, over {MAX_RUN_BYTES}",
            bytes.len()
        ));
    }
    Shape {
        bytes,
        pos: 0,
        items: 0,
    }
    .top()?;
    read_value(bytes)
}

/// A walk over raw CBOR that allocates nothing: depth, item count and declared lengths.
struct Shape<'a> {
    bytes: &'a [u8],
    pos: usize,
    items: usize,
}

impl Shape<'_> {
    fn top(&mut self) -> Result<(), String> {
        if self.item(0)? {
            return Err("a CBOR break outside an indefinite item".into());
        }
        Ok(())
    }

    fn left(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.bytes.get(self.pos).ok_or("truncated CBOR")?;
        self.pos += 1;
        Ok(b)
    }

    fn skip(&mut self, n: u64) -> Result<(), String> {
        let n = usize::try_from(n)
            .ok()
            .filter(|&n| n <= self.left())
            .ok_or("a CBOR length past the end of the value")?;
        self.pos += n;
        Ok(())
    }

    /// The head's argument; `None` for an indefinite length.
    fn arg(&mut self, info: u8) -> Result<Option<u64>, String> {
        Ok(Some(match info {
            0..=23 => u64::from(info),
            24..=27 => {
                let mut v = 0u64;
                for _ in 0..(1u32 << (info - 24)) {
                    v = (v << 8) | u64::from(self.byte()?);
                }
                v
            }
            31 => return Ok(None),
            _ => return Err("malformed CBOR".into()),
        }))
    }

    /// One item at `depth`; `true` when it was a break code.
    fn item(&mut self, depth: usize) -> Result<bool, String> {
        if depth > MAX_CBOR_DEPTH {
            return Err(format!("CBOR nested deeper than {MAX_CBOR_DEPTH}"));
        }
        let b = self.byte()?;
        if b == 0xFF {
            return Ok(true);
        }
        self.items += 1;
        if self.items > MAX_CBOR_ITEMS {
            return Err(format!("more than {MAX_CBOR_ITEMS} CBOR items"));
        }
        let (major, info) = (b >> 5, b & 0x1F);
        match (major, self.arg(info)?) {
            (0 | 1 | 7, Some(_)) => {}
            (2 | 3, Some(n)) => self.skip(n)?,
            (2 | 3, None) => loop {
                // Chunks of definite strings of the same type, up to a break.
                let chunk = self.byte()?;
                if chunk == 0xFF {
                    break;
                }
                match self.arg(chunk & 0x1F)? {
                    Some(n) if chunk >> 5 == major => self.skip(n)?,
                    _ => return Err("malformed CBOR string chunk".into()),
                }
            },
            (4, Some(n)) => self.children(n, 1, depth)?,
            (5, Some(n)) => self.children(n, 2, depth)?,
            (4 | 5, None) => while !self.item(depth + 1)? {},
            (6, Some(_)) => {
                if self.item(depth + 1)? {
                    return Err("a CBOR tag around a break".into());
                }
            }
            _ => return Err("malformed CBOR".into()),
        }
        Ok(false)
    }

    /// `n` entries of `per` items each; every item takes at least one byte, so a count the
    /// remaining bytes cannot hold is refused before any is read.
    fn children(&mut self, n: u64, per: u64, depth: usize) -> Result<(), String> {
        let total = n.saturating_mul(per);
        if total > u64::try_from(self.left()).unwrap_or(u64::MAX) {
            return Err("a CBOR length past the end of the value".into());
        }
        for _ in 0..total {
            if self.item(depth + 1)? {
                return Err("a CBOR break inside a definite item".into());
            }
        }
        Ok(())
    }
}

/// `value` in deterministic CBOR: map keys sorted by their encoded bytes, recursively;
/// NaN and duplicate map keys refused.
pub(crate) fn canonical(value: Value) -> Result<Value, String> {
    Ok(match value {
        Value::Float(f) if f.is_nan() => return Err("NaN".into()),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(canonical).collect::<Result<_, _>>()?)
        }
        Value::Tag(tag, inner) => Value::Tag(tag, Box::new(canonical(*inner)?)),
        Value::Map(entries) => {
            let mut keyed = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = canonical(k)?;
                keyed.push((write_value(&k)?, k, canonical(v)?));
            }
            keyed.sort_by(|a, b| a.0.cmp(&b.0));
            if keyed.windows(2).any(|w| w[0].0 == w[1].0) {
                return Err("a map key twice".into());
            }
            Value::Map(keyed.into_iter().map(|(_, k, v)| (k, v)).collect())
        }
        other => other,
    })
}

pub(crate) fn write_value(value: &Value) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|e| format!("encode: {e}"))?;
    Ok(out)
}

/// `bytes` from the guest ([`read_guest_value`]) re-encoded as the SDK's `value::to_vec`
/// writes the value they hold.
pub(crate) fn canonical_value(bytes: &[u8]) -> Result<Vec<u8>, String> {
    write_value(&canonical(read_guest_value(bytes)?)?)
}

/// A record field as a key part, by CBOR type, as the SDK's `ToKeyPart` makes it.
pub(crate) fn field_part(record: &[(Value, Value)], field: &str) -> Result<KeyPart, String> {
    let value = record
        .iter()
        .find(|(k, _)| k.as_text() == Some(field))
        .map(|(_, v)| v)
        .ok_or_else(|| format!("no field `{field}`"))?;
    Ok(match value {
        Value::Null => KeyPart::None,
        Value::Bool(b) => KeyPart::Bool(*b),
        Value::Integer(i) if fits_wit(i128::from(*i)) => KeyPart::Int(i128::from(*i)),
        Value::Integer(_) => return Err(format!("field `{field}` is outside s64 and u64")),
        Value::Text(t) => KeyPart::Str(t.clone()),
        Value::Bytes(b) => KeyPart::Bytes(b.clone()),
        _ => return Err(format!("field `{field}` cannot be a key part")),
    })
}

/// Whether an integer key part has a WIT form (`int(s64)` or `uint(u64)`). One without
/// is never stored: a find or page returning its key could not answer it.
pub(crate) fn fits_wit(i: i128) -> bool {
    i64::try_from(i).is_ok() || u64::try_from(i).is_ok()
}

/// Refuse a key with an integer part [`fits_wit`] does not accept.
pub(crate) fn check_wit_key(parts: &[KeyPart]) -> Result<(), String> {
    if parts
        .iter()
        .any(|p| matches!(p, KeyPart::Int(i) if !fits_wit(*i)))
    {
        return Err("a key integer outside s64 and u64".into());
    }
    Ok(())
}
