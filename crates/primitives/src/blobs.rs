use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hash::{Hash, HashError};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BlobId(Hash);

impl BlobId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn hash(bytes: &[u8]) -> Self {
        Self(Hash::new(bytes))
    }
}

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

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

#[derive(Clone, Copy, Debug, Error)]
#[error(transparent)]
pub struct InvalidBlobId(HashError);

impl FromStr for BlobId {
    type Err = InvalidBlobId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidBlobId)?))
    }
}
