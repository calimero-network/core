use core::fmt::{self, Display, Formatter};
use core::ops::Deref;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use url::{ParseError, Url};

use crate::blobs::BlobId;
use crate::hash::{Hash, HashError};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
// todo! define macros that construct newtypes
// todo! wrapping Hash<N> with this interface
pub struct ApplicationId(Hash);

impl From<[u8; 32]> for ApplicationId {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl Deref for ApplicationId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<ApplicationId> for Hash {
    fn from(key: ApplicationId) -> Self {
        key.0
    }
}

impl From<Hash> for ApplicationId {
    fn from(key: Hash) -> Self {
        Self(key)
    }
}

impl ApplicationId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for ApplicationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<ApplicationId> for String {
    fn from(id: ApplicationId) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&ApplicationId> for String {
    fn from(id: &ApplicationId) -> Self {
        id.as_str().to_owned()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[error(transparent)]
pub struct InvalidApplicationId(HashError);

impl FromStr for ApplicationId {
    type Err = InvalidApplicationId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidApplicationId)?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApplicationSource(Url);

impl FromStr for ApplicationSource {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl From<Url> for ApplicationSource {
    fn from(value: Url) -> Self {
        Self(value)
    }
}

impl From<ApplicationSource> for Url {
    fn from(value: ApplicationSource) -> Self {
        value.0
    }
}

impl Display for ApplicationSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Application {
    pub id: ApplicationId,
    pub blob: BlobId,
    pub size: u64,
    pub source: ApplicationSource,
    pub metadata: Vec<u8>,
}

impl Application {
    #[must_use]
    pub const fn new(
        id: ApplicationId,
        blob: BlobId,
        size: u64,
        source: ApplicationSource,
        metadata: Vec<u8>,
    ) -> Self {
        Self {
            id,
            blob,
            size,
            source,
            metadata,
        }
    }
}
