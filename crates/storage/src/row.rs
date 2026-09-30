//! One physical row per entity.
//!
//! The storage layer thinks in two logical keys per entity: `Key::Index(id)`
//! for its index record and `Key::Entry(id)` for its data. Stored as two rows,
//! that cost a second key (64 bytes on a node, plus RocksDB's per-entry
//! overhead) and a copy of `Sha256(data)` in the index row, for every entity.
//!
//! This module keeps the two logical keys and stores them in ONE row, at the
//! physical key of `Key::Index(id)`:
//!
//! ```text
//! row   = flags(1) ‖ [varint len ‖ index(len)] ‖ [data]
//! flags = MAGIC | HAS_INDEX | HAS_DATA | OWN_DERIVED | RAW_INDEX
//! index = EntityIndex with `own_hash` omitted when OWN_DERIVED
//!       | the bytes verbatim, when RAW_INDEX (a value that is not an
//!         `EntityIndex` — only tests write one)
//! data  = the rest of the row
//! ```
//!
//! The part layout — flags, and where each part starts — is defined once in
//! [`calimero_prelude::row`], so readers that cannot depend on this crate find
//! the data the same way. This module adds what the parts MEAN.
//!
//! ```text
//! ```
//!
//! `own_hash` is `Sha256(data)` for every entity the storage layer writes, so
//! it is derived rather than stored whenever that holds. It does not always
//! hold at every instant: the value and the hash are separate logical writes,
//! so between them the row carries new data under the old hash. A data write
//! that would change a derived hash therefore pins the old one explicitly
//! first, and the index write that follows derives it again. Every read
//! returns exactly the bytes the caller last wrote to that logical key.
//!
//! The encoding is canonical: [`decode`] refuses every form [`encode`] never
//! produces (an explicit `own_hash` that could have been derived, unknown flag
//! bits, trailing bytes after a raw index), so a byte-exact round trip
//! identifies an entity row, which tombstone GC relies on.

use borsh::BorshDeserialize;
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::index::{EntityIndex, SlimIndex};
use crate::store::Key;

use calimero_prelude::row::{put_varint, HAS_DATA, HAS_INDEX, MAGIC, OWN_DERIVED, RAW_INDEX};

/// The logical contents of one entity row.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Row {
    /// What `Key::Index(id)` holds: an `EntityIndex`, borsh-encoded.
    pub index: Option<Vec<u8>>,
    /// What `Key::Entry(id)` holds.
    pub data: Option<Vec<u8>>,
}

impl Row {
    /// Whether neither logical key holds anything, so the row can be removed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_none() && self.data.is_none()
    }

    /// The decoded index record, if the row has one that decodes.
    #[must_use]
    pub fn entity_index(&self) -> Option<EntityIndex> {
        EntityIndex::try_from_slice(self.index.as_deref()?).ok()
    }
}

fn digest(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Encodes `row`. The caller removes the physical row instead when
/// [`Row::is_empty`].
#[must_use]
pub fn encode(row: &Row) -> Vec<u8> {
    let mut flags = MAGIC;
    let mut index_part = Vec::new();
    if let Some(index_bytes) = &row.index {
        flags |= HAS_INDEX;
        match EntityIndex::try_from_slice(index_bytes) {
            Ok(index) => {
                let derived = row
                    .data
                    .as_deref()
                    .is_some_and(|data| index.own_hash() == digest(data));
                if derived {
                    flags |= OWN_DERIVED;
                }
                index
                    .serialize_slim(&mut index_part, derived)
                    .expect("writing to a Vec cannot fail");
            }
            Err(_) => {
                flags |= RAW_INDEX;
                index_part.extend_from_slice(index_bytes);
            }
        }
    }
    let mut out = vec![flags];
    if row.index.is_some() {
        put_varint(&mut out, index_part.len() as u64);
        out.extend_from_slice(&index_part);
    }
    if let Some(data) = &row.data {
        out[0] |= HAS_DATA;
        out.extend_from_slice(data);
    }
    out
}

/// Inverse of [`encode`]. `None` for anything `encode` would not produce.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<Row> {
    let parts = calimero_prelude::row::split(bytes)?;
    let derived = parts.flags & OWN_DERIVED != 0;
    let raw = parts.flags & RAW_INDEX != 0;
    if (derived && !(parts.index.is_some() && parts.data.is_some()))
        || (raw && (derived || parts.index.is_none()))
    {
        return None;
    }
    let data = parts.data.map(<[u8]>::to_vec);
    let index = match parts.index {
        None => None,
        Some(head) if raw => {
            // A raw index is only ever something that is not an `EntityIndex`.
            if EntityIndex::try_from_slice(head).is_ok() {
                return None;
            }
            Some(head.to_vec())
        }
        Some(mut head) => {
            let slim = SlimIndex::deserialize(&mut head, derived).ok()?;
            if !head.is_empty() {
                return None;
            }
            let entity = slim.finish(data.as_deref().map(digest))?;
            Some(borsh::to_vec(&entity).ok()?)
        }
    };
    Some(Row { index, data })
}

/// Reads `key` through `raw`, which reads a physical row by key.
///
/// `Key::Index` and `Key::Entry` resolve to their part of the entity row; every
/// other key passes through.
pub fn read(key: Key, raw: impl Fn(Key) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    match key {
        Key::Index(id) => load(id, &raw).and_then(|row| row.index),
        Key::Entry(id) => load(id, &raw).and_then(|row| row.data),
        other => raw(other),
    }
}

/// Writes `value` to `key`: a read-modify-write of the entity row for
/// `Key::Index` / `Key::Entry`, a plain write otherwise. Returns what the
/// physical write returned (whether it overwrote a row).
pub fn write(
    key: Key,
    value: &[u8],
    raw_read: impl Fn(Key) -> Option<Vec<u8>>,
    raw_write: impl Fn(Key, &[u8]) -> bool,
) -> bool {
    let (id, is_index) = match key {
        Key::Index(id) => (id, true),
        Key::Entry(id) => (id, false),
        other => return raw_write(other, value),
    };
    let mut row = load(id, &raw_read).unwrap_or_default();
    if is_index {
        row.index = Some(value.to_vec());
    } else {
        // `encode` re-derives `own_hash` against the new data, so a hash that
        // no longer matches it is kept explicitly — the index reads back as
        // written.
        row.data = Some(value.to_vec());
    }
    raw_write(Key::Index(id), &encode(&row))
}

/// Removes `key`. For `Key::Index` / `Key::Entry` only that part of the entity
/// row goes; the row itself is removed once both parts are gone. Returns
/// whether the logical key held anything.
pub fn remove(
    key: Key,
    raw_read: impl Fn(Key) -> Option<Vec<u8>>,
    raw_write: impl Fn(Key, &[u8]) -> bool,
    raw_remove: impl Fn(Key) -> bool,
) -> bool {
    let (id, is_index) = match key {
        Key::Index(id) => (id, true),
        Key::Entry(id) => (id, false),
        other => return raw_remove(other),
    };
    let Some(mut row) = load(id, &raw_read) else {
        return false;
    };
    let existed = if is_index {
        row.index.take().is_some()
    } else {
        row.data.take().is_some()
    };
    if row.is_empty() {
        let _ignored = raw_remove(Key::Index(id));
    } else {
        let _ignored = raw_write(Key::Index(id), &encode(&row));
    }
    existed
}

fn load(id: Id, raw: &impl Fn(Key) -> Option<Vec<u8>>) -> Option<Row> {
    // A row that does not decode is not an entity row this layer wrote; surface
    // it as an index value so the caller's own decode reports the corruption
    // instead of the entity silently reading as absent.
    raw(Key::Index(id)).map(|bytes| {
        decode(&bytes).unwrap_or(Row {
            index: Some(bytes),
            data: None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::Metadata;

    fn index_with(id: Id, own: [u8; 32]) -> Vec<u8> {
        let mut index = EntityIndex::minimal_for_test(id);
        index.set_own_hash(own);
        borsh::to_vec(&index).unwrap()
    }

    #[test]
    fn round_trips_every_shape() {
        let id = Id::new([9; 32]);
        let data = b"hello".to_vec();
        let shapes = [
            Row { index: Some(index_with(id, digest(&data))), data: Some(data.clone()) },
            Row { index: Some(index_with(id, [3; 32])), data: Some(data.clone()) },
            Row { index: Some(index_with(id, [0; 32])), data: None },
            Row { index: None, data: Some(data.clone()) },
            Row { index: None, data: Some(Vec::new()) },
            Row { index: Some(b"not an index".to_vec()), data: Some(data) },
        ];
        for row in shapes {
            let bytes = encode(&row);
            assert_eq!(decode(&bytes).as_ref(), Some(&row), "{row:?}");
            assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
        }
    }

    #[test]
    fn derived_hash_costs_nothing() {
        let id = Id::new([9; 32]);
        let data = vec![7; 100];
        let derived = encode(&Row { index: Some(index_with(id, digest(&data))), data: Some(data.clone()) });
        let explicit = encode(&Row { index: Some(index_with(id, [1; 32])), data: Some(data) });
        assert_eq!(explicit.len() - derived.len(), 32);
    }

    #[test]
    fn refuses_non_canonical_rows() {
        let id = Id::new([9; 32]);
        let data = b"x".to_vec();
        // An explicit `own_hash` equal to `Sha256(data)`: derivable, so refused.
        let mut index = EntityIndex::minimal_for_test(id);
        index.set_own_hash(digest(&data));
        let slim = |derived| {
            let mut part = Vec::new();
            index.serialize_slim(&mut part, derived).unwrap();
            part
        };
        let mut bytes = vec![MAGIC | HAS_INDEX | HAS_DATA];
        put_varint(&mut bytes, slim(false).len() as u64);
        bytes.extend_from_slice(&slim(false));
        bytes.extend_from_slice(&data);
        assert_eq!(decode(&bytes), None);
        // Derived with no data to derive from.
        let mut bytes = vec![MAGIC | HAS_INDEX | OWN_DERIVED];
        put_varint(&mut bytes, slim(true).len() as u64);
        bytes.extend_from_slice(&slim(true));
        assert_eq!(decode(&bytes), None);
        assert_eq!(decode(&[MAGIC]), None);
        assert_eq!(decode(&[0xB2, 0, 0]), None);
        let _ = Metadata::default();
    }

    #[test]
    fn logical_keys_are_independent() {
        use std::cell::RefCell;
        use std::collections::BTreeMap;
        let store = RefCell::new(BTreeMap::<[u8; 32], Vec<u8>>::new());
        let rd = |k: Key| store.borrow().get(&k.to_bytes()).cloned();
        let wr = |k: Key, v: &[u8]| store.borrow_mut().insert(k.to_bytes(), v.to_vec()).is_some();
        let rm = |k: Key| store.borrow_mut().remove(&k.to_bytes()).is_some();
        let id = Id::new([4; 32]);
        let old = b"old".to_vec();
        let new = b"new!".to_vec();

        let _ = write(Key::Entry(id), &old, rd, wr);
        let idx = index_with(id, digest(&old));
        let _ = write(Key::Index(id), &idx, rd, wr);
        assert_eq!(store.borrow().len(), 1, "one physical row");
        // New data under the old hash: the index must still read back verbatim.
        let _ = write(Key::Entry(id), &new, rd, wr);
        assert_eq!(read(Key::Index(id), rd), Some(idx.clone()));
        assert_eq!(read(Key::Entry(id), rd), Some(new.clone()));
        assert!(remove(Key::Entry(id), rd, wr, rm));
        assert_eq!(read(Key::Index(id), rd), Some(idx));
        assert_eq!(read(Key::Entry(id), rd), None);
        assert!(remove(Key::Index(id), rd, wr, rm));
        assert!(store.borrow().is_empty());
    }
}
