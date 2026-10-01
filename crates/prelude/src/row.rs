//! Layout of an entity row: one physical storage row holding both an entity's
//! index record and its data.
//!
//! `calimero-storage` owns the meaning of the index part and does all the
//! encoding (`calimero_storage::row`). This module only knows where the parts
//! are, so readers that cannot depend on the storage crate — the SDK's
//! migration read, node code scanning raw state — find an entity's data the
//! same way the storage layer does.
//!
//! ```text
//! row   = flags(1) ‖ [varint len ‖ index(len)] ‖ [data]
//! ```
//!
//! The index part is present iff `HAS_INDEX`, and the data (the rest of the
//! row) iff `HAS_DATA`. An entity's row lives at
//! [`ENTITY_KEY_TAG`](crate::constants::ENTITY_KEY_TAG) `‖ id`; the id is not
//! stored in the row, so every reader passes it in.

/// High bits every entity row starts with. Child-trie rows start with `0xB2`
/// or `0xA2`, so the first byte alone tells an entity row apart.
pub const MAGIC: u8 = 0xE0;
/// Mask selecting [`MAGIC`]'s bits.
pub const MAGIC_MASK: u8 = 0xE0;
/// The row holds an index record.
pub const HAS_INDEX: u8 = 0x01;
/// The row holds data.
pub const HAS_DATA: u8 = 0x02;
/// The index record's `own_hash` is `Sha256(data)` and is not stored.
pub const OWN_DERIVED: u8 = 0x04;
/// The index part is stored verbatim: it is not an `EntityIndex`.
pub const RAW_INDEX: u8 = 0x08;
/// The data ended with the entity's own id, which is left out: the row's key
/// names the id, and a read appends it again.
pub const ID_ELIDED: u8 = 0x10;
/// Length of an entity id.
pub const ID_LEN: usize = 32;

// With ID_ELIDED every bit of the flags byte has a meaning: a further flag
// needs a new MAGIC.

/// The parts of an entity row, borrowed from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RowParts<'a> {
    /// The flags byte.
    pub flags: u8,
    /// The index part, as stored (slim or raw, per `flags`).
    pub index: Option<&'a [u8]>,
    /// The data as stored: without its trailing id when [`ID_ELIDED`].
    pub data: Option<&'a [u8]>,
}

impl<'a> RowParts<'a> {
    /// The data as written: with `id`, the entity the row belongs to, appended
    /// again when [`ID_ELIDED`] left it out.
    #[must_use]
    pub fn full_data(&self, id: &[u8; ID_LEN]) -> Option<std::borrow::Cow<'a, [u8]>> {
        let data = self.data?;
        Some(if self.flags & ID_ELIDED != 0 {
            std::borrow::Cow::Owned([data, &id[..]].concat())
        } else {
            std::borrow::Cow::Borrowed(data)
        })
    }
}

/// Splits `row` into its parts. `None` when it is not an entity row: a wrong
/// magic, neither part, or a length that overruns the row.
#[must_use]
pub fn split(row: &[u8]) -> Option<RowParts<'_>> {
    let (&flags, mut rest) = row.split_first()?;
    if flags & MAGIC_MASK != MAGIC {
        return None;
    }
    let has_index = flags & HAS_INDEX != 0;
    let has_data = flags & HAS_DATA != 0;
    if !has_index && !has_data {
        return None;
    }
    let index = if has_index {
        let len = usize::try_from(take_varint(&mut rest)?).ok()?;
        if rest.len() < len {
            return None;
        }
        let (head, tail) = rest.split_at(len);
        rest = tail;
        Some(head)
    } else {
        None
    };
    if !has_data && !rest.is_empty() {
        return None;
    }
    // Only an entry's data, under an `EntityIndex`, has its id left out.
    if flags & ID_ELIDED != 0 && (!has_data || !has_index || flags & RAW_INDEX != 0) {
        return None;
    }
    Some(RowParts {
        flags,
        index,
        data: has_data.then_some(rest),
    })
}

/// The data the entity row of `id` holds, as written, if it is one and holds
/// any.
#[must_use]
pub fn data<'a>(id: &[u8; ID_LEN], row: &'a [u8]) -> Option<std::borrow::Cow<'a, [u8]>> {
    split(row)?.full_data(id)
}

/// Appends `value` as an LEB128 varint.
pub fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Reads an LEB128 varint, refusing overlong and non-minimal encodings so a
/// row has exactly one encoding.
#[must_use]
pub fn take_varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value: u64 = 0;
    for (i, &byte) in bytes.iter().enumerate().take(10) {
        let part = u64::from(byte & 0x7F);
        if i == 9 && part > 1 {
            return None;
        }
        value |= part << (7 * i);
        if byte & 0x80 == 0 {
            if i > 0 && byte == 0 {
                return None;
            }
            *bytes = &bytes[i + 1..];
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_what_it_is_given() {
        let mut row = vec![MAGIC | HAS_INDEX | HAS_DATA];
        put_varint(&mut row, 3);
        row.extend_from_slice(b"idxdata");
        let parts = split(&row).unwrap();
        assert_eq!(parts.index, Some(&b"idx"[..]));
        assert_eq!(parts.data, Some(&b"data"[..]));
        assert_eq!(
            data(&[0; ID_LEN], &[MAGIC | HAS_DATA]).as_deref(),
            Some(&[][..])
        );
        assert_eq!(split(&[MAGIC]), None);
        assert_eq!(split(&[0xB2, 1, 2]), None);
        assert_eq!(split(&[MAGIC | HAS_INDEX, 5, 1]), None);
        assert_eq!(split(&[MAGIC | HAS_INDEX, 0, 1]), None, "trailing bytes");
    }

    #[test]
    fn appends_an_elided_id() {
        let id = [7_u8; ID_LEN];
        let mut row = vec![MAGIC | HAS_INDEX | HAS_DATA | ID_ELIDED];
        put_varint(&mut row, 1);
        row.push(0);
        row.extend_from_slice(b"item");
        assert_eq!(
            data(&id, &row).as_deref(),
            Some(&[&b"item"[..], &id].concat()[..])
        );
        // Only an entry under an index has its id left out.
        assert_eq!(split(&[MAGIC | HAS_DATA | ID_ELIDED, 1]), None);
    }

    #[test]
    fn varints_are_minimal() {
        for v in [0, 1, 127, 128, 300, u64::MAX] {
            let mut out = Vec::new();
            put_varint(&mut out, v);
            let mut slice = &out[..];
            assert_eq!(take_varint(&mut slice), Some(v));
            assert!(slice.is_empty());
        }
        assert_eq!(take_varint(&mut &[0x80, 0x00][..]), None);
    }
}
