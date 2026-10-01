//! One physical row per entity.
//!
//! The storage layer thinks in two logical keys per entity: `Key::Index(id)`
//! for its index record and `Key::Entry(id)` for its data. Stored as two rows,
//! that cost a second key (64 bytes on a node, plus RocksDB's per-entry
//! overhead) and a copy of `Sha256(data)` in the index row, for every entity.
//!
//! This module keeps the two logical keys and stores them in ONE row, at the
//! physical key of `Key::Index(id)`, which is the id behind a tag (see
//! [`Key::to_bytes`]) — so the row does not store the id:
//!
//! ```text
//! row   = flags(1) ‖ [varint len ‖ index(len)] ‖ [data]
//! flags = MAGIC | HAS_INDEX | HAS_DATA | OWN_DERIVED | RAW_INDEX | ID_ELIDED
//! index = EntityIndex without its id, and with `own_hash` omitted when
//!         OWN_DERIVED
//!       | the bytes verbatim, when RAW_INDEX (a value that is not this
//!         entity's `EntityIndex` — only tests write one)
//! data  = the rest of the row, without its trailing id when ID_ELIDED
//! ```
//!
//! A map entry serializes as its item followed by its `Element`, whose stored
//! form is the entity's id — 32 bytes the key already names. When the data
//! ends with the id, the row leaves them out (ID_ELIDED) and a read appends
//! them again. This is a property of the bytes, not of what they mean, so it
//! is lossless for any value.
//!
//! The part layout — flags, and where each part starts — is defined once in
//! [`calimero_prelude::row`], so readers that cannot depend on this crate find
//! the data the same way. This module adds what the parts MEAN.
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
//! produces (an explicit `own_hash` that could have been derived, an id kept
//! that could have been elided, a wrong magic, trailing bytes after a raw
//! index), so a byte-exact round trip
//! identifies an entity row, which tombstone GC relies on.

use borsh::BorshDeserialize;
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::index::{EntityIndex, SlimIndex};
use crate::store::Key;

use calimero_prelude::row::{
    put_varint, HAS_DATA, HAS_INDEX, ID_ELIDED, ID_LEN, MAGIC, OWN_DERIVED, RAW_INDEX,
};

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

/// Encodes `row`, the row of entity `id`. The caller removes the physical row
/// instead when [`Row::is_empty`].
#[must_use]
pub fn encode(id: Id, row: &Row) -> Vec<u8> {
    let mut flags = MAGIC;
    let mut index_part = Vec::new();
    let mut elided = false;
    if let Some(index_bytes) = &row.index {
        flags |= HAS_INDEX;
        // The slim form leaves the id to the key, so an index naming another
        // entity is kept verbatim.
        match EntityIndex::try_from_slice(index_bytes)
            .ok()
            .filter(|index| index.id() == id)
        {
            Some(index) => {
                elided = row
                    .data
                    .as_deref()
                    .is_some_and(|data| data.ends_with(id.as_bytes()));
                if elided {
                    flags |= ID_ELIDED;
                }
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
            None => {
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
        let kept = if elided {
            data.len() - ID_LEN
        } else {
            data.len()
        };
        out.extend_from_slice(&data[..kept]);
    }
    out
}

/// Inverse of [`encode`]: `bytes` read as the row of entity `id`. `None` for
/// anything `encode` would not produce.
#[must_use]
pub fn decode(id: Id, bytes: &[u8]) -> Option<Row> {
    let parts = calimero_prelude::row::split(bytes)?;
    let derived = parts.flags & OWN_DERIVED != 0;
    let raw = parts.flags & RAW_INDEX != 0;
    if (derived && !(parts.index.is_some() && parts.data.is_some()))
        || (raw && (derived || parts.index.is_none()))
    {
        return None;
    }
    let elided = parts.flags & ID_ELIDED != 0;
    let data = parts
        .full_data(id.as_bytes())
        .map(std::borrow::Cow::into_owned);
    let index = match parts.index {
        None => None,
        Some(head) if raw => {
            // A raw index is only ever something that is not this entity's
            // `EntityIndex`.
            if EntityIndex::try_from_slice(head).is_ok_and(|index| index.id() == id) {
                return None;
            }
            Some(head.to_vec())
        }
        Some(mut head) => {
            let slim = SlimIndex::deserialize(&mut head, derived, id).ok()?;
            if !head.is_empty() {
                return None;
            }
            let entity = slim.finish(data.as_deref().map(digest))?;
            // `encode` elides exactly when the data ends with the id.
            if data
                .as_deref()
                .is_some_and(|data| data.ends_with(id.as_bytes()))
                != elided
            {
                return None;
            }
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
    raw_write(Key::Index(id), &encode(id, &row))
}

/// Writes both logical keys of entity `id` at once: `index` to `Key::Index(id)`
/// and `data` to `Key::Entry(id)`. Together they are the whole row, so nothing
/// is read back — the row is composed and written once. Returns what the
/// physical write returned (whether it overwrote a row).
pub fn write_entity(
    id: Id,
    index: &[u8],
    data: &[u8],
    raw_write: impl Fn(Key, &[u8]) -> bool,
) -> bool {
    let row = Row {
        index: Some(index.to_vec()),
        data: Some(data.to_vec()),
    };
    raw_write(Key::Index(id), &encode(id, &row))
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
        let _ignored = raw_write(Key::Index(id), &encode(id, &row));
    }
    existed
}

fn load(id: Id, raw: &impl Fn(Key) -> Option<Vec<u8>>) -> Option<Row> {
    // A row that does not decode is not an entity row this layer wrote; surface
    // it as an index value so the caller's own decode reports the corruption
    // instead of the entity silently reading as absent.
    raw(Key::Index(id)).map(|bytes| {
        decode(id, &bytes).unwrap_or(Row {
            index: Some(bytes),
            data: None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::Metadata;
    use crate::store::KEY_LEN;

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
            Row {
                index: Some(index_with(id, digest(&data))),
                data: Some(data.clone()),
            },
            Row {
                index: Some(index_with(id, [3; 32])),
                data: Some(data.clone()),
            },
            Row {
                index: Some(index_with(id, [0; 32])),
                data: None,
            },
            Row {
                index: None,
                data: Some(data.clone()),
            },
            Row {
                index: None,
                data: Some(Vec::new()),
            },
            Row {
                index: Some(b"not an index".to_vec()),
                data: Some(data),
            },
        ];
        let shapes = shapes.into_iter().chain([
            // Another entity's index is kept verbatim.
            Row {
                index: Some(index_with(Id::new([8; 32]), [3; 32])),
                data: Some(b"x".to_vec()),
            },
        ]);
        for row in shapes {
            let bytes = encode(id, &row);
            assert_eq!(decode(id, &bytes).as_ref(), Some(&row), "{row:?}");
            assert_eq!(encode(id, &decode(id, &bytes).unwrap()), bytes);
        }
    }

    #[test]
    fn derived_hash_costs_nothing() {
        let id = Id::new([9; 32]);
        let data = vec![7; 100];
        let derived = encode(
            id,
            &Row {
                index: Some(index_with(id, digest(&data))),
                data: Some(data.clone()),
            },
        );
        let explicit = encode(
            id,
            &Row {
                index: Some(index_with(id, [1; 32])),
                data: Some(data),
            },
        );
        assert_eq!(explicit.len() - derived.len(), 32);
    }

    #[test]
    fn an_entry_keeps_its_id_once() {
        let id = Id::new([9; 32]);
        let entry = [&b"item"[..], id.as_bytes()].concat();
        let full = Row {
            index: Some(index_with(id, digest(&entry))),
            data: Some(entry.clone()),
        };
        let bytes = encode(id, &full);
        assert_eq!(decode(id, &bytes), Some(full.clone()));
        assert_eq!(
            calimero_prelude::row::data(id.as_bytes(), &bytes).as_deref(),
            Some(&entry[..])
        );
        // The same data as another entity's row keeps every byte.
        let other_id = Id::new([8; 32]);
        let other = Row {
            index: Some(index_with(other_id, digest(&entry))),
            data: Some(entry),
        };
        assert_eq!(encode(other_id, &other).len() - bytes.len(), 32);
        assert_eq!(decode(other_id, &encode(other_id, &other)), Some(other));
        // Data that is only the id elides to nothing and comes back whole.
        let bare = Row {
            index: Some(index_with(id, [1; 32])),
            data: Some(id.as_bytes().to_vec()),
        };
        assert_eq!(decode(id, &encode(id, &bare)), Some(bare));
    }

    #[test]
    fn refuses_an_id_left_in_or_elided_wrongly() {
        let id = Id::new([9; 32]);
        let index = index_with(id, [1; 32]);
        let entry = [&b"item"[..], id.as_bytes()].concat();
        let canonical = encode(
            id,
            &Row {
                index: Some(index.clone()),
                data: Some(entry),
            },
        );
        // Clearing the flag and putting the id back is a second encoding.
        let mut kept = canonical.clone();
        kept[0] &= !ID_ELIDED;
        kept.extend_from_slice(id.as_bytes());
        assert_eq!(decode(id, &kept), None);
        // Claiming an elision the data did not have: the restored data would
        // end with the id twice, which `encode` never writes.
        let mut doubled = canonical;
        doubled.extend_from_slice(id.as_bytes());
        assert!(decode(id, &doubled).is_some_and(|row| encode(id, &row) == doubled));
        let plain = encode(
            id,
            &Row {
                index: Some(index),
                data: Some(b"item".to_vec()),
            },
        );
        let mut claimed = plain;
        claimed[0] |= ID_ELIDED;
        assert!(decode(id, &claimed).is_none_or(|row| encode(id, &row) == claimed));
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
        assert_eq!(decode(id, &bytes), None);
        // Derived with no data to derive from.
        let mut bytes = vec![MAGIC | HAS_INDEX | OWN_DERIVED];
        put_varint(&mut bytes, slim(true).len() as u64);
        bytes.extend_from_slice(&slim(true));
        assert_eq!(decode(id, &bytes), None);
        assert_eq!(decode(id, &[MAGIC]), None);
        assert_eq!(decode(id, &[0xB2, 0, 0]), None);
        // This entity's own index, written raw.
        let mut raw = vec![MAGIC | HAS_INDEX | RAW_INDEX];
        let own = borsh::to_vec(&index).unwrap();
        put_varint(&mut raw, own.len() as u64);
        raw.extend_from_slice(&own);
        assert_eq!(decode(id, &raw), None);
        let _ = Metadata::default();
    }

    #[test]
    fn logical_keys_are_independent() {
        use std::cell::RefCell;
        use std::collections::BTreeMap;
        let store = RefCell::new(BTreeMap::<[u8; KEY_LEN], Vec<u8>>::new());
        let rd = |k: Key| store.borrow().get(&k.to_bytes()).cloned();
        let wr = |k: Key, v: &[u8]| {
            store
                .borrow_mut()
                .insert(k.to_bytes(), v.to_vec())
                .is_some()
        };
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

    #[test]
    fn a_combined_write_is_the_two_writes_without_reads() {
        use std::cell::{Cell, RefCell};
        use std::collections::BTreeMap;
        let id = Id::new([4; 32]);
        let entry = [&b"item"[..], id.as_bytes()].concat();
        // Derived, explicit, and another entity's (raw) index.
        for idx in [
            index_with(id, digest(&entry)),
            index_with(id, [1; 32]),
            index_with(Id::new([5; 32]), [1; 32]),
        ] {
            let store = RefCell::new(BTreeMap::<[u8; KEY_LEN], Vec<u8>>::new());
            let reads = Cell::new(0);
            let rd = |k: Key| {
                reads.set(reads.get() + 1);
                store.borrow().get(&k.to_bytes()).cloned()
            };
            let wr = |k: Key, v: &[u8]| {
                store
                    .borrow_mut()
                    .insert(k.to_bytes(), v.to_vec())
                    .is_some()
            };
            // Over an existing row whose parts both change.
            let _ = write(Key::Entry(id), b"old", rd, wr);
            let _ = write(Key::Index(id), &index_with(id, [7; 32]), rd, wr);
            let _ = write(Key::Entry(id), &entry, rd, wr);
            let _ = write(Key::Index(id), &idx, rd, wr);
            let sequential = store.borrow().clone();

            let _ = write(Key::Index(id), &index_with(id, [7; 32]), rd, wr);
            let _ = write(Key::Entry(id), b"old", rd, wr);
            reads.set(0);
            assert!(write_entity(id, &idx, &entry, wr));
            assert_eq!(reads.get(), 0, "nothing is read back");
            assert_eq!(*store.borrow(), sequential);
            assert_eq!(read(Key::Index(id), rd), Some(idx));
            assert_eq!(read(Key::Entry(id), rd), Some(entry.clone()));
        }
    }
}
