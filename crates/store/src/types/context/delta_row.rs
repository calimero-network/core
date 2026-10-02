//! The on-disk encoding of a [`ContextDagDelta`] row in `Column::Delta`.
//!
//! This is a LOCAL storage format. Nothing here is signed, hashed into a delta
//! id, or sent to a peer: the wire DTOs and signature payloads are built from
//! the decoded fields, never from these bytes. That is what makes it safe to
//! encode the row more compactly than borsh does.
//!
//! # Layout (version 1)
//!
//! ```text
//! TAG (0xCD) ‖ VERSION (0x01) ‖ flags (1 byte)
//! ‖ varint(parents.len()) ‖ parents (32 B each)
//! ‖ varint(actions.len()) ‖ actions
//! ‖ hlc (16 B: u64 time LE ‖ u64 id LE, borsh)
//! ‖ [checkpoint_root_hash: 32 B]                       if flags & CHECKPOINT_ROOT_HASH
//! ‖ [varint(events.len()) ‖ events]                    if flags & EVENTS
//! ‖ [author_id: 32 B]                                  if flags & AUTHOR_ID
//! ‖ [varint(blob.len()) ‖ governance_position_blob]    if flags & GOVERNANCE_POSITION
//! ‖ [delta_signature: 64 B]                            if flags & DELTA_SIGNATURE
//! ‖ [borsh(delegation), to the end of the row]         if flags & DELEGATION
//! ```
//!
//! `flags` bit 0 is `applied`; bits 1–6 say which optional field is present;
//! bit 7 is reserved and must be zero. A varint is unsigned LEB128 in its
//! shortest form. The decoder is strict: a non-minimal varint, a reserved flag
//! bit, a length past the end of the row, or a trailing byte is refused.
//!
//! What this drops compared with the borsh encoding it replaced:
//! - the delta id (32 B): it is the second half of the row's key, and every
//!   reader already holds the key;
//! - one presence byte per optional field and the `applied` byte: all of them
//!   are bits of `flags`;
//! - three of the four bytes of every borsh `u32` length (parents, actions,
//!   events, governance position), for any length below 128.
//!
//! # Rows written before this format
//!
//! Those are plain borsh of the old struct, which led with the 32-byte delta id.
//! [`decode`] still reads them. A row is taken as version 1 only when it starts
//! with `TAG ‖ VERSION` *and* the rest parses as a complete version-1 row;
//! anything else is decoded as the old layout. An old row is misread only if
//! its delta id — a SHA-256 output — happens to begin with those two bytes AND
//! everything after them also forms a well-formed version-1 row ending exactly
//! at the last byte. Nothing rewrites old rows: they stay in the old layout
//! until DAG compaction or context deletion removes them, and a row is only
//! ever written back in version 1.

use std::io::{Error as IoError, ErrorKind, Result as IoResult};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::Delegation;
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;

use super::ContextDagDelta;

/// First byte of every row this module writes.
const TAG: u8 = 0xCD;
/// Second byte: the layout version that follows.
const VERSION_1: u8 = 0x01;

const APPLIED: u8 = 1 << 0;
const CHECKPOINT_ROOT_HASH: u8 = 1 << 1;
const EVENTS: u8 = 1 << 2;
const AUTHOR_ID: u8 = 1 << 3;
const GOVERNANCE_POSITION: u8 = 1 << 4;
const DELTA_SIGNATURE: u8 = 1 << 5;
const DELEGATION: u8 = 1 << 6;
const KNOWN_FLAGS: u8 = APPLIED
    | CHECKPOINT_ROOT_HASH
    | EVENTS
    | AUTHOR_ID
    | GOVERNANCE_POSITION
    | DELTA_SIGNATURE
    | DELEGATION;

/// Bytes of a borsh `HybridTimestamp`: `u64` time ‖ `u64` id.
const HLC_LEN: usize = 16;

/// Encode `row` in the current (version 1) layout.
pub(super) fn encode(row: &ContextDagDelta) -> IoResult<Vec<u8>> {
    let mut flags = 0;
    let mut set = |bit: u8, on: bool| {
        if on {
            flags |= bit;
        }
    };
    set(APPLIED, row.applied);
    set(CHECKPOINT_ROOT_HASH, row.checkpoint_root_hash.is_some());
    set(EVENTS, row.events.is_some());
    set(AUTHOR_ID, row.author_id.is_some());
    set(GOVERNANCE_POSITION, row.governance_position_blob.is_some());
    set(DELTA_SIGNATURE, row.delta_signature.is_some());
    set(DELEGATION, row.delegation.is_some());

    let mut out = Vec::with_capacity(
        3 + 5
            + 32 * row.parents.len()
            + 5
            + row.actions.len()
            + HLC_LEN
            + 32
            + row.events.as_ref().map_or(0, |e| 5 + e.len())
            + 32
            + row
                .governance_position_blob
                .as_ref()
                .map_or(0, |g| 5 + g.len())
            + 64,
    );
    out.extend_from_slice(&[TAG, VERSION_1, flags]);
    put_varint(&mut out, row.parents.len());
    for parent in &row.parents {
        out.extend_from_slice(parent);
    }
    put_bytes(&mut out, &row.actions);
    row.hlc.serialize(&mut out)?;
    if let Some(hash) = &row.checkpoint_root_hash {
        out.extend_from_slice(hash);
    }
    if let Some(events) = &row.events {
        put_bytes(&mut out, events);
    }
    if let Some(author) = &row.author_id {
        out.extend_from_slice(AsRef::<[u8; 32]>::as_ref(author));
    }
    if let Some(blob) = &row.governance_position_blob {
        put_bytes(&mut out, blob);
    }
    if let Some(signature) = &row.delta_signature {
        out.extend_from_slice(signature);
    }
    if let Some(delegation) = &row.delegation {
        delegation.serialize(&mut out)?;
    }
    Ok(out)
}

/// Decode a stored row, in the current layout or the one before it.
pub(super) fn decode(bytes: &[u8]) -> IoResult<ContextDagDelta> {
    let Some(rest) = bytes.strip_prefix(&[TAG, VERSION_1]) else {
        return decode_legacy(bytes);
    };
    match decode_v1(rest) {
        Ok(row) => Ok(row),
        // An old row whose delta id happens to start with the tag.
        Err(v1_error) => decode_legacy(bytes).map_err(|_| v1_error),
    }
}

fn decode_v1(bytes: &[u8]) -> IoResult<ContextDagDelta> {
    let mut r = Reader(bytes);
    let flags = r.byte()?;
    if flags & !KNOWN_FLAGS != 0 {
        return Err(invalid("delta row: reserved flag bit set"));
    }
    let has = |bit: u8| flags & bit != 0;

    let parent_count = r.varint()?;
    if parent_count > r.0.len() / 32 {
        return Err(invalid("delta row: parent count exceeds the row"));
    }
    let mut parents = Vec::with_capacity(parent_count);
    for _ in 0..parent_count {
        parents.push(r.array::<32>()?);
    }
    let actions = r.bytes()?.to_vec();
    let hlc = HybridTimestamp::try_from_slice(r.take(HLC_LEN)?)?;
    let checkpoint_root_hash = has(CHECKPOINT_ROOT_HASH)
        .then(|| r.array::<32>())
        .transpose()?;
    let events = has(EVENTS)
        .then(|| r.bytes().map(<[u8]>::to_vec))
        .transpose()?;
    let author_id = has(AUTHOR_ID)
        .then(|| r.array::<32>().map(PublicKey::from))
        .transpose()?;
    let governance_position_blob = has(GOVERNANCE_POSITION)
        .then(|| r.bytes().map(<[u8]>::to_vec))
        .transpose()?;
    let delta_signature = has(DELTA_SIGNATURE).then(|| r.array::<64>()).transpose()?;
    // Last, and borsh's `from_slice` refuses trailing bytes.
    let delegation = if has(DELEGATION) {
        Some(borsh::from_slice::<Delegation>(r.0)?)
    } else if r.0.is_empty() {
        None
    } else {
        return Err(invalid("delta row: trailing bytes"));
    };

    Ok(ContextDagDelta {
        parents,
        actions,
        hlc,
        applied: has(APPLIED),
        checkpoint_root_hash,
        events,
        author_id,
        governance_position_blob,
        delta_signature,
        delegation,
    })
}

/// The borsh layout every row had before version 1. Read-only: nothing writes
/// it any more. Field order is the format; do not reorder.
#[derive(BorshDeserialize)]
#[cfg_attr(test, derive(BorshSerialize))]
struct LegacyRow {
    // Decoded only to step over it: the key carries the delta id.
    #[allow(dead_code, reason = "read past, never used; the key carries it")]
    delta_id: [u8; 32],
    parents: Vec<[u8; 32]>,
    actions: Vec<u8>,
    hlc: HybridTimestamp,
    applied: bool,
    checkpoint_root_hash: Option<[u8; 32]>,
    events: Option<Vec<u8>>,
    author_id: Option<PublicKey>,
    governance_position_blob: Option<Vec<u8>>,
    delta_signature: Option<[u8; 64]>,
    delegation: Option<Delegation>,
}

fn decode_legacy(bytes: &[u8]) -> IoResult<ContextDagDelta> {
    let LegacyRow {
        delta_id: _,
        parents,
        actions,
        hlc,
        applied,
        checkpoint_root_hash,
        events,
        author_id,
        governance_position_blob,
        delta_signature,
        delegation,
    } = borsh::from_slice(bytes)?;
    Ok(ContextDagDelta {
        parents,
        actions,
        hlc,
        applied,
        checkpoint_root_hash,
        events,
        author_id,
        governance_position_blob,
        delta_signature,
        delegation,
    })
}

fn invalid(message: &'static str) -> IoError {
    IoError::new(ErrorKind::InvalidData, message)
}

fn put_varint(out: &mut Vec<u8>, value: usize) {
    let mut value = value as u64;
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_varint(out, bytes.len());
    out.extend_from_slice(bytes);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> IoResult<&'a [u8]> {
        if n > self.0.len() {
            return Err(invalid("delta row: field runs past the end of the row"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn byte(&mut self) -> IoResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> IoResult<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// An unsigned LEB128 length in its shortest form. It never exceeds the
    /// row, so ten bytes (a full `u64`) is a generous cap.
    fn varint(&mut self) -> IoResult<usize> {
        let mut value: u64 = 0;
        for i in 0..10 {
            let b = self.byte()?;
            let low = u64::from(b & 0x7F);
            if i == 9 && low > 1 {
                return Err(invalid("delta row: varint overflows u64"));
            }
            value |= low << (7 * i);
            if b & 0x80 == 0 {
                if b == 0 && i > 0 {
                    return Err(invalid("delta row: varint not in shortest form"));
                }
                return usize::try_from(value)
                    .map_err(|_| invalid("delta row: varint overflows usize"));
            }
        }
        Err(invalid("delta row: varint longer than ten bytes"))
    }

    fn bytes(&mut self) -> IoResult<&'a [u8]> {
        let len = self.varint()?;
        self.take(len)
    }
}

#[cfg(test)]
mod tests;
