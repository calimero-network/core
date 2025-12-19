use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::{
    blobs::BlobId, common::DIGEST_SIZE, context::ContextId, identity::PublicKey,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobRequest {
    pub blob_id: BlobId,
    pub context_id: ContextId,

    /// Optional authentication.
    /// If None, only public blobs (Application Bundles) can be accessed.
    pub auth: Option<BlobAuth>,
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
