use crate::common::DIGEST_SIZE;
use core::fmt::{self, Display, Formatter};
use core::ops::Deref;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::hash::{Hash, HashError};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Ord, Hash, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub struct BlobId(Hash);

impl BlobId {
    // Returns BlobId represented as a 32-byte array.
    pub fn digest(&self) -> &[u8; DIGEST_SIZE] {
        &self.0
    }
}

impl From<[u8; 32]> for BlobId {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; 32]> for BlobId {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for BlobId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for BlobId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<BlobId> for String {
    fn from(id: BlobId) -> Self {
        id.0.to_base58()
    }
}

impl From<&BlobId> for String {
    fn from(id: &BlobId) -> Self {
        id.0.to_base58()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[error(transparent)]
pub struct InvalidBlobId(HashError);

impl FromStr for BlobId {
    type Err = InvalidBlobId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidBlobId)?))
    }
}

/// Core blob information
#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct BlobInfo {
    /// The unique blob ID
    pub blob_id: BlobId,
    /// Size of the blob in bytes
    pub size: u64,
}

/// Detailed blob metadata
#[derive(Debug, Serialize, Deserialize)]
pub struct BlobMetadata {
    pub blob_id: BlobId,
    pub size: u64,
    pub hash: [u8; 32],
    pub mime_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_id_roundtrip() {
        let blob_id = BlobId::from([1; DIGEST_SIZE]);
        let encoded = blob_id.to_string();
        // Same base58 vector as the other 32-byte Hash newtypes for `[1; 32]`.
        let expected = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
        assert_eq!(encoded, expected);
        assert_eq!(BlobId::from_str(&encoded).unwrap(), blob_id);
    }

    #[test]
    fn test_blob_id_digest_matches_source_bytes() {
        let bytes = [7; DIGEST_SIZE];
        let blob_id = BlobId::from(bytes);
        assert_eq!(blob_id.digest(), &bytes);
        assert_eq!(blob_id.as_ref(), &bytes);
    }

    #[test]
    fn test_blob_id_invalid_base58() {
        let result = BlobId::from_str("Invalid!");
        assert!(matches!(
            result,
            Err(InvalidBlobId(HashError::DecodeError(_)))
        ));
    }

    #[test]
    fn test_blob_id_json_is_base58_string() {
        let blob_id = BlobId::from([1; DIGEST_SIZE]);
        let json = serde_json::to_string(&blob_id).unwrap();
        assert_eq!(json, "\"4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi\"");
        let parsed: BlobId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, blob_id);
    }
}
