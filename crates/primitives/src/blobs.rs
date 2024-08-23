use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hash::{Error as HashError, Hash};

#[derive(Eq, Copy, Hash, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlobId(Hash);

impl From<[u8; 32]> for BlobId {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl Deref for BlobId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BlobId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<BlobId> for String {
    fn from(id: BlobId) -> Self {
        id.as_str().to_string()
    }
}

impl From<&BlobId> for String {
    fn from(id: &BlobId) -> Self {
        id.as_str().to_string()
    }
}

#[derive(Debug, Error)]
#[error(transparent)]
pub struct InvalidBlobId(HashError);

impl FromStr for BlobId {
    type Err = InvalidBlobId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidBlobId)?))
    }
}
