use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::blobs::BlobId;
use crate::hash::{Error as HashError, Hash};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
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

impl ApplicationId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for ApplicationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<ApplicationId> for String {
    fn from(id: ApplicationId) -> Self {
        id.as_str().to_string()
    }
}

impl From<&ApplicationId> for String {
    fn from(id: &ApplicationId) -> Self {
        id.as_str().to_string()
    }
}

#[derive(Clone, Copy, Debug, Error)]
#[error(transparent)]
pub struct InvalidApplicationId(HashError);

impl FromStr for ApplicationId {
    type Err = InvalidApplicationId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidApplicationId)?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApplicationSource(url::Url);

impl FromStr for ApplicationSource {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl From<url::Url> for ApplicationSource {
    fn from(value: url::Url) -> Self {
        Self(value)
    }
}

impl From<ApplicationSource> for url::Url {
    fn from(value: ApplicationSource) -> Self {
        value.0
    }
}

impl fmt::Display for ApplicationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Application {
    pub id: ApplicationId,
    pub blob: BlobId,
    pub version: Option<semver::Version>,
    pub source: ApplicationSource,
    pub metadata: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Release {
    pub version: semver::Version,
    pub notes: String,
    pub path: String,
    pub hash: String,
}
