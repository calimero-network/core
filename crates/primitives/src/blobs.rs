use core::fmt::{Display, Formatter};
use core::ops::Deref;
use core::str::FromStr;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::hash::{Hash, HashError};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
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
