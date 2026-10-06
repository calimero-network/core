use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::{
    blobs::BlobId, common::DIGEST_SIZE, context::ContextId, identity::PublicKey,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const MAX_BLOB_RESPONSE_FRAME_BYTES: usize = 4 * 1024; // a `BlobResponse` is a few dozen bytes of JSON

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobRequest {
    pub blob_id: BlobId,
    pub context_id: ContextId,

    /// Optional authentication.
    /// If None, only public blobs (Application Bundles) can be accessed.
    pub auth: Option<BlobAuth>,
}

/// Sole message of [`crate::stream::CALIMERO_BLOB_ANNOUNCE_PROTOCOL`]: the
/// sender has the blob and it belongs to this context.
///
/// Serialised with `serde_json`, matching [`BlobRequest`] on the transfer
/// protocol. The receiver acts only on an announcement whose `auth` is signed,
/// for the sending peer, by a member of `context_id`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BlobAnnouncement {
    pub blob_id: BlobId,
    pub context_id: ContextId,
    /// Size in bytes, so a receiver can decline an oversized blob before
    /// opening a transfer stream for it.
    pub size: u64,
    /// The announcer's membership proof, built like a signed [`BlobRequest`]'s.
    pub auth: BlobAuth,
}

/// What a probe learned about one peer's copy of a blob.
///
/// A probe is a [`BlobRequest`] whose chunks are never read, so this is exactly
/// the information in the [`BlobResponse`] header the responder sends before it
/// streams anything — no more, and nothing inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobProbe {
    /// The peer does not hold it, will not serve it, or never answered. The
    /// protocol answers "not held" and "not authorised" identically, so a
    /// caller cannot — and must not try to — tell them apart.
    Absent,
    /// The peer holds it and would serve it.
    Held {
        /// Size in bytes as the holder reported it. `None` only if it answered
        /// `found` without a size, which no in-tree responder does; a caller
        /// that needs the number must then ask someone else rather than
        /// substitute a guess.
        size: Option<u64>,
    },
}

impl BlobProbe {
    /// Whether this peer is worth fetching from.
    #[must_use]
    pub const fn is_held(self) -> bool {
        matches!(self, Self::Held { .. })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobResponse {
    pub found: bool,

    // Total size if found
    pub size: Option<u64>,
}

// Use binary format for efficient chunk transfer
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BlobChunk {
    pub data: Vec<u8>,
}

/// Authentication data for a blob request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BlobAuth {
    /// The public key of the requester (must be a member of the context).
    pub public_key: PublicKey,

    /// Ed25519 signature over `BlobAuthPayload`.
    #[serde(with = "signature_serde")]
    pub signature: [u8; 64],

    /// Unix timestamp in seconds to prevent replay attacks.
    pub timestamp: u64,
}

/// The data that is actually signed and verified.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BlobAuthPayload {
    pub blob_id: [u8; DIGEST_SIZE],
    pub context_id: [u8; DIGEST_SIZE],
    pub timestamp: u64,
    /// The requester's transport `PeerId` bytes, so a holder that receives the
    /// request cannot present it to another holder as its own.
    pub requester: Vec<u8>,
}

/// Helper module to serialize Signature
mod signature_serde {
    use super::*;
    use serde::de::Error;

    pub fn serialize<S>(bytes: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Use serde_bytes behavior
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = serde::Deserialize::deserialize(deserializer)?;
        if bytes.len() != 64 {
            return Err(D::Error::custom(format!(
                "expected 64 bytes for signature, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

/// Bytes a caller lets its blob transfers receive in total, shared by every
/// transfer it hands a clone to, so falling back to another holder draws on the same bytes.
#[derive(Clone, Debug)]
pub struct ByteBudget {
    limit: u64,
    received: Arc<AtomicU64>,
}

impl ByteBudget {
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            received: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Count `bytes` as received, and say whether they still fit the limit:
    /// bytes that arrived were received whether or not they are kept.
    #[must_use]
    pub fn take(&self, bytes: u64) -> bool {
        let before = self
            .received
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |received| {
                Some(received.saturating_add(bytes))
            })
            .unwrap_or_else(|received| received);
        before.saturating_add(bytes) <= self.limit
    }

    #[must_use]
    pub fn left(&self) -> u64 {
        self.limit.saturating_sub(self.received())
    }

    #[must_use]
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod byte_budget_tests {
    use super::ByteBudget;

    #[test]
    fn clones_count_into_one_budget_and_an_overdraw_is_still_received() {
        let budget = ByteBudget::new(10);
        let other_holder = budget.clone();

        assert!(budget.take(6));
        assert_eq!(other_holder.left(), 4);
        assert!(!other_holder.take(5), "only 4 bytes are left");
        assert_eq!(budget.received(), 11);
        assert_eq!(budget.left(), 0);
        assert!(!budget.take(0), "nothing fits once the limit is passed");
    }
}
