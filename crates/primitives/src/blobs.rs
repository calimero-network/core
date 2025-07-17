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
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
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
        f.pad(self.as_str())
    }
}

impl From<BlobId> for String {
    fn from(id: BlobId) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&BlobId> for String {
    fn from(id: &BlobId) -> Self {
        id.as_str().to_owned()
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
