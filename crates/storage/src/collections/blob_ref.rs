//! An immutable reference to a content-addressed blob.
//!
//! `BlobRef` is a **value, not a CRDT**. Blobs are content-addressed, so two
//! different ids are two different files rather than two versions of one value;
//! merge semantics belong to whatever slot holds the reference (an
//! [`LwwRegister`](super::LwwRegister), a map value), not to the reference.
//!
//! It carries `size` so a reader can budget a transfer, or render a progress
//! bar, before opening anything.

use borsh::{BorshDeserialize, BorshSerialize};

/// A reference to a blob: its content-addressed id and its length in bytes.
#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobRef {
    blob_id: [u8; 32],
    size: u64,
}

impl BlobRef {
    /// Build a reference to a blob of `size` bytes.
    #[must_use]
    pub const fn new(blob_id: [u8; 32], size: u64) -> Self {
        Self { blob_id, size }
    }

    /// The blob's content-addressed id.
    #[must_use]
    pub const fn blob_id(&self) -> [u8; 32] {
        self.blob_id
    }

    /// The blob's length in bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_ref_round_trips_through_borsh() {
        let original = BlobRef::new([7u8; 32], 4096);
        let bytes = borsh::to_vec(&original).expect("serialize");
        let decoded: BlobRef = borsh::from_slice(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
        assert_eq!(decoded.blob_id(), [7u8; 32]);
        assert_eq!(decoded.size(), 4096);
    }

    #[test]
    fn blob_refs_with_different_ids_are_not_equal() {
        assert_ne!(BlobRef::new([1u8; 32], 10), BlobRef::new([2u8; 32], 10));
    }
}
