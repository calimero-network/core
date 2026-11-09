//! Helpers tables and collections share: checked keys, paged reads, counts and clears.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::encoding::key::{self, KeyPart};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{check_key, RangeCursor, Store, MAX_RANGE_BATCH, MAX_SDK_KEY_BYTES};
use crate::table::{FindOpts, DEFAULT_LIMIT, MAX_LIMIT};

/// One key-value entry read from the store.
pub(crate) type Entry = (Vec<u8>, Vec<u8>);

/// `parts` encoded, refused over [`MAX_SDK_KEY_BYTES`] (D50).
pub(crate) fn encode(parts: &[KeyPart]) -> Result<Vec<u8>, StorageError> {
    let encoded = key::encode(parts);
    check_key(&encoded)?;
    Ok(encoded)
}

/// The key `(name, kind, rest..)` of a collection, checked.
pub(crate) fn collection_key(
    name: &str,
    kind: &str,
    rest: &[KeyPart],
) -> Result<Vec<u8>, StorageError> {
    let mut parts = vec![KeyPart::Str(name.into()), KeyPart::Str(kind.into())];
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
pub(crate) fn clear_prefix<S: Store>(store: &mut S, prefix: &[u8]) -> u64 {
    let (start, end) = bounds(prefix);
    let mut removed = 0;
    loop {
        // A fresh cursor per batch: the previous batch is gone, so this one starts after it.
        let batch = store.range(&start, &end, false).next(MAX_RANGE_BATCH);
        if batch.is_empty() {
            return removed;
        }
        for (k, _) in batch {
            store.delete(&k);
            removed += 1;
        }
    }
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
    check_key(base)?;
    let limit = opts.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    if limit == 0 {
        return Ok((Vec::new(), None));
    }
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

/// The parts of a stored key after its `base` bytes; `Decode` when they are not a key.
pub(crate) fn key_after(stored: &[u8], base: &[u8]) -> Result<Vec<KeyPart>, StorageError> {
    stored
        .strip_prefix(base)
        .and_then(key::decode)
        .ok_or_else(|| StorageError::Decode("not a key of this collection".into()))
}
