//! The writes a written-once entry keeps.
//!
//! Of every authentic write made to a written-once entry, every node keeps the
//! lowest by `(nonce, SHA-256 of bytes, signature)`, and the lowest whose bytes
//! differ from it. One kept write is the entry's value. Two seal the entry: its
//! stored bytes become a conflict record holding both signed writes, so any node
//! can verify the seal, and nodes that kept the same writes store the same bytes.

use core::cmp::Ordering;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::constants::MAX_LEAF_VALUE_SIZE;
use crate::entities::{SignatureData, StorageType};
use crate::error::StorageError;
use crate::hash_meter::{Digest, Sha256};

const CONFLICT_TAG: &[u8] = b"calimero.written-once.conflict.v1"; // leads a conflict record
const RECORD_OVERHEAD: usize = 512; // bounds a record's tag and its writes' lengths and signatures

/// The most bytes a written-once write may carry, so a record of two fits one sync leaf.
pub(crate) const MAX_VALUE_BYTES: usize = (MAX_LEAF_VALUE_SIZE - RECORD_OVERHEAD) / 2;

/// One authentic write: the bytes and the signature over them.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignedWrite {
    pub(crate) data: Vec<u8>,
    pub(crate) signature_data: SignatureData,
}

impl SignedWrite {
    fn order(&self, other: &Self) -> Ordering {
        let hash = |bytes: &[u8]| -> [u8; 32] { Sha256::digest(bytes).into() };
        self.signature_data
            .nonce
            .cmp(&other.signature_data.nonce)
            .then_with(|| hash(&self.data).cmp(&hash(&other.data)))
            .then_with(|| self.signature_data.cmp(&other.signature_data))
    }
}

/// What a written-once entry stores once an authentic write has settled against it.
#[derive(Debug)]
pub(crate) enum Settled {
    /// The entry already keeps that write, or keeps lower ones.
    Unchanged,
    /// Store these bytes, stamped with the first kept write's signature.
    Store {
        data: Vec<u8>,
        signature_data: SignatureData,
    },
}

/// Settle the authentic `incoming` write against the entry's `stored` bytes and
/// `stored_type` stamp. Refuses other bytes while the stored write is unsigned.
pub(crate) fn settle(
    stored_type: &StorageType,
    stored: Vec<u8>,
    incoming: SignedWrite,
) -> Result<Settled, StorageError> {
    let held = match conflict_writes(stored_type, &stored) {
        Some((first, second)) => vec![first, second],
        None => match stored_type
            .signature_data()
            .filter(|sig| sig.signature != [0; 64])
        {
            Some(signature_data) => vec![SignedWrite {
                data: stored,
                signature_data: *signature_data,
            }],
            None if stored == incoming.data => return Ok(Settled::Unchanged),
            None => {
                return Err(StorageError::ActionNotAllowed(
                    "a written-once entry whose write is not yet signed cannot be changed"
                        .to_owned(),
                ))
            }
        },
    };
    let mut writes = held.clone();
    writes.push(incoming);
    let keeps = kept(writes);
    if keeps == held {
        return Ok(Settled::Unchanged);
    }
    let signature_data = keeps[0].signature_data;
    Ok(Settled::Store {
        data: settled_bytes(keeps)?,
        signature_data,
    })
}

/// The bytes an entry stores for its kept writes: the one value, or the record of two.
pub(crate) fn settled_bytes(kept: Vec<SignedWrite>) -> Result<Vec<u8>, StorageError> {
    let mut kept = kept.into_iter();
    let (Some(first), second) = (kept.next(), kept.next()) else {
        return Ok(Vec::new());
    };
    let Some(second) = second else {
        return Ok(first.data);
    };
    let mut bytes = CONFLICT_TAG.to_vec();
    (first, second)
        .serialize(&mut bytes)
        .map_err(StorageError::SerializationError)?;
    Ok(bytes)
}

/// The writes an entry keeps of `writes`: the lowest, and the lowest whose bytes differ.
pub(crate) fn kept(mut writes: Vec<SignedWrite>) -> Vec<SignedWrite> {
    writes.sort_by(SignedWrite::order);
    let mut writes = writes.into_iter();
    let Some(first) = writes.next() else {
        return Vec::new();
    };
    let second = writes.find(|write| write.data != first.data);
    core::iter::once(first).chain(second).collect()
}

/// The two writes `data` records, if it is the conflict record of an entry stamped
/// `storage_type`: canonical, and its first write's signature the one the stamp carries.
pub(crate) fn conflict_writes(
    storage_type: &StorageType,
    data: &[u8],
) -> Option<(SignedWrite, SignedWrite)> {
    if !may_be_written_once(storage_type) {
        return None;
    }
    let (first, second): (SignedWrite, SignedWrite) =
        borsh::from_slice(data.strip_prefix(CONFLICT_TAG)?).ok()?;
    let canonical = first.order(&second).is_lt()
        && first.data != second.data
        && second.signature_data.signature != [0; 64];
    (canonical && storage_type.signature_data() == Some(&first.signature_data))
        .then_some((first, second))
}

/// The two plain writes a conflict record stands for, each as the bytes and the
/// stamp it was signed with; `None` for any other bytes.
#[must_use]
pub fn conflict_parts(
    storage_type: &StorageType,
    data: &[u8],
) -> Option<[(Vec<u8>, StorageType); 2]> {
    let (first, second) = conflict_writes(storage_type, data)?;
    Some([first, second].map(|write| {
        (
            write.data,
            with_signature(storage_type, write.signature_data),
        )
    }))
}

/// `storage_type` carrying `signature` in place of its own.
pub(crate) fn with_signature(storage_type: &StorageType, signature: SignatureData) -> StorageType {
    let mut stamped = storage_type.clone();
    if let StorageType::User { signature_data, .. }
    | StorageType::Shared { signature_data, .. }
    | StorageType::SharedMember { signature_data, .. } = &mut stamped
    {
        *signature_data = Some(signature);
    }
    stamped
}

/// Whether an entry stamped `storage_type` can be written once: an immutable owned
/// entry, or a writer-set cell's, whose writers decide it at apply.
fn may_be_written_once(storage_type: &StorageType) -> bool {
    match storage_type {
        StorageType::User { rules, .. } => rules.immutable,
        StorageType::Shared { .. } | StorageType::SharedMember { .. } => true,
        StorageType::Public | StorageType::Frozen => false,
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use calimero_primitives::identity::PublicKey;

    use super::{settled_bytes, SignedWrite, MAX_LEAF_VALUE_SIZE, MAX_VALUE_BYTES};
    use crate::entities::SignatureData;

    #[test]
    fn a_record_of_two_writes_at_the_cap_fits_one_sync_leaf() {
        let write = |byte: u8| SignedWrite {
            data: vec![byte; MAX_VALUE_BYTES],
            signature_data: SignatureData {
                signature: [byte; 64],
                nonce: u64::MAX,
                signer: Some(PublicKey::from([byte; 32])),
                on_behalf: Some(AccountId::from([byte; 32])),
            },
        };
        let record = settled_bytes(vec![write(1), write(2)]).expect("a record encodes");
        assert!(
            record.len() <= MAX_LEAF_VALUE_SIZE,
            "{} bytes",
            record.len()
        );
    }
}
