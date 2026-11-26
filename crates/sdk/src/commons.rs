//! Helpers tables and collections share: checked keys, paged reads, counts and clears.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::encoding::key::{self, KeyPart};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{check_key, RangeCursor, Store, MAX_RANGE_BATCH, MAX_SDK_KEY_BYTES};
use crate::table::{FindOpts, Iter, DEFAULT_LIMIT, MAX_LIMIT};

/// One key-value entry read from the store.
pub(crate) type Entry = (Vec<u8>, Vec<u8>);

/// `parts` encoded, refused over [`MAX_SDK_KEY_BYTES`], one byte under the host's
/// 1,024-byte key limit.
pub(crate) fn encode(parts: &[KeyPart]) -> Result<Vec<u8>, StorageError> {
    let encoded = key::encode(parts);
    check_key(&encoded)?;
    Ok(encoded)
}

/// Where a collection's keys start.
///
/// A state field's collection starts at its name, `(name)`, so its keys are
/// `(name, kind, rest..)` as they always were. A collection that is the value of a map
/// (`UnorderedMap<K, Vector<T>>`) starts at the map's path, then `"c"` and its key encoded as
/// one bytes part: `(name, "c", key)`, and so on for each level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPath {
    label: String,
    parts: Vec<KeyPart>,
}

impl KeyPath {
    /// A state field's path: its name.
    pub fn root(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            parts: vec![KeyPart::Str(name.clone())],
            label: name,
        }
    }

    /// The path of the collection nested under the key `key` here.
    pub(crate) fn child(&self, key: &[KeyPart]) -> Self {
        let mut parts = self.parts.clone();
        parts.push(KeyPart::Str(NESTED.into()));
        parts.push(KeyPart::Bytes(key::encode(key)));
        Self {
            label: self.label.clone(),
            parts,
        }
    }

    /// The state field this path starts at, for error messages.
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    /// The key parts every key of the collection starts with.
    pub(crate) fn parts(&self) -> &[KeyPart] {
        &self.parts
    }

    /// Whether this is a nested collection's path rather than a state field's.
    pub(crate) fn is_nested(&self) -> bool {
        self.parts.len() > 1
    }
}

/// The kind part under which a map of collections keeps each key's collection.
pub(crate) const NESTED: &str = "c";

/// The key `(path.., kind, rest..)` of a collection, checked.
pub(crate) fn collection_key(
    path: &KeyPath,
    kind: &str,
    rest: &[KeyPart],
) -> Result<Vec<u8>, StorageError> {
    let mut parts = path.parts().to_vec();
    parts.push(KeyPart::Str(kind.into()));
    parts.extend_from_slice(rest);
    encode(&parts)
}

/// The range covering every key that starts with `prefix`: tags never reach `0xFF`.
pub(crate) fn bounds(prefix: &[u8]) -> (Vec<u8>, Vec<u8>) {
    (prefix.to_vec(), [prefix, &[0xFF]].concat())
}

/// The non-negative count at `key` (absent is 0), as `Store::add` keeps it; `name` names it
/// in the error.
pub(crate) fn read_count<S: Store>(store: &S, key: &[u8], name: &str) -> Result<u64, StorageError> {
    let n = match store.get(key) {
        Some(bytes) => value::int_from_slice(&bytes)?,
        None => 0,
    };
    u64::try_from(n).map_err(|_| StorageError::BadCounter(name.into()))
}

/// Delete every key under `prefix`, a batch of at most [`MAX_RANGE_BATCH`] at a time, and
/// return how many there were.
pub(crate) fn clear_prefix<S: Store>(store: &mut S, prefix: &[u8]) -> Result<u64, StorageError> {
    let (start, end) = bounds(prefix);
    let mut removed = 0;
    loop {
        // A fresh cursor per batch: the previous batch is gone, so this one starts after it.
        let batch = store.range(&start, &end, false).next(MAX_RANGE_BATCH);
        if batch.is_empty() {
            return Ok(removed);
        }
        for (k, _) in batch {
            store.delete(&k)?;
            removed += 1;
        }
    }
}

/// Delete at most `max` keys under `prefix`, in key order, a batch at a time; returns how
/// many were deleted and whether none is left.
pub(crate) fn delete_some<S: Store>(
    store: &mut S,
    prefix: &[u8],
    max: u64,
) -> Result<(u64, bool), StorageError> {
    let (start, end) = bounds(prefix);
    let mut removed = 0;
    while removed < max {
        // A fresh cursor per batch: the previous batch is gone, so this one starts after it.
        let want = (max - removed).min(u64::from(MAX_RANGE_BATCH)) as u32;
        let batch = store.range(&start, &end, false).next(want);
        if batch.is_empty() {
            return Ok((removed, true));
        }
        for (k, _) in batch {
            store.delete(&k)?;
            removed += 1;
        }
    }
    Ok((removed, store.range(&start, &end, false).next(1).is_empty()))
}

/// Read up to `limit + 1` entries under the encoded `base` and cut the page at `limit`.
///
/// `from` and `to` are further parts after `base`; the cursor is the base64url of the last
/// key of the previous page and must lie under `base`.
pub(crate) fn read_page<S: Store>(
    store: &S,
    base: &[u8],
    opts: &FindOpts,
) -> Result<(Vec<Entry>, Option<String>), StorageError> {
    let limit = opts.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    if limit == 0 {
        check_key(base)?;
        return Ok((Vec::new(), None));
    }
    let (start, end) = read_bounds(base, opts)?;
    let mut cursor = store.range(&start, &end, opts.reverse);
    let mut entries = Vec::with_capacity(limit + 1);
    while entries.len() <= limit {
        let want = (limit + 1 - entries.len()).min(MAX_RANGE_BATCH as usize);
        let batch = cursor.next(want as u32);
        if batch.is_empty() {
            break;
        }
        entries.extend(batch);
    }
    let next = if entries.len() > limit {
        entries.truncate(limit);
        entries.last().map(|(k, _)| URL_SAFE_NO_PAD.encode(k))
    } else {
        None
    };
    Ok((entries, next))
}

/// The `[start, end)` a read under `base` covers with `opts`' `from`, `to` and `cursor`.
fn read_bounds(base: &[u8], opts: &FindOpts) -> Result<(Vec<u8>, Vec<u8>), StorageError> {
    check_key(base)?;
    let (mut start, mut end) = bounds(base);
    if let Some(from) = &opts.from {
        start = [base, &key::encode(from)].concat();
        check_key(&start)?;
    }
    if let Some(to) = &opts.to {
        end = [base, &key::encode(to)].concat();
        check_key(&end)?;
    }
    if let Some(cursor) = &opts.cursor {
        let last = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| StorageError::BadCursor)?;
        if last.len() > MAX_SDK_KEY_BYTES || !last.starts_with(base) {
            return Err(StorageError::BadCursor);
        }
        if opts.reverse {
            end = last;
        } else {
            // The smallest key after `last`.
            start = [last.as_slice(), &[0x00]].concat();
        }
    }
    Ok((start, end))
}

/// Every entry under `base` within `opts`' bounds, decoded, through one cursor: one range
/// open, then one host call per batch of `opts.limit` (1,000 unset), with no entry read
/// twice and none read ahead of the batch.
pub(crate) fn iter_entries<'a, S: Store, T: 'a>(
    store: &'a S,
    base: &[u8],
    opts: &FindOpts,
    mut decode: impl FnMut(Entry) -> Result<T, StorageError> + 'a,
) -> Iter<'a, T> {
    let batch = opts
        .limit
        .unwrap_or(MAX_RANGE_BATCH)
        .clamp(1, MAX_RANGE_BATCH);
    let (start, end) = match read_bounds(base, opts) {
        Ok(b) => b,
        Err(e) => return Iter::failed(e),
    };
    let reverse = opts.reverse;
    let mut cursor: Option<S::Range> = None;
    Iter::from_batches(move || {
        let cursor = cursor.get_or_insert_with(|| store.range(&start, &end, reverse));
        cursor.next(batch).into_iter().map(&mut decode).collect()
    })
}

/// The parts of a stored key after its `base` bytes; `Decode` when they are not a key.
pub(crate) fn key_after(stored: &[u8], base: &[u8]) -> Result<Vec<KeyPart>, StorageError> {
    stored
        .strip_prefix(base)
        .and_then(key::decode)
        .ok_or_else(|| StorageError::Decode("not a key of this collection".into()))
}
