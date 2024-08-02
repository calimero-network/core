use std::ops::Deref;
use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::blobs::BlobId;
use crate::hash::{Error as HashError, Hash};

#[derive(Eq, Copy, Hash, Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Error)]
#[error(transparent)]
pub struct InvalidApplicationId(HashError);

impl FromStr for ApplicationId {
    type Err = InvalidApplicationId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidApplicationId)?))
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ApplicationSource(#[serde(with = "http_serde::uri")] http::Uri);

impl FromStr for ApplicationSource {
    type Err = http::uri::InvalidUri;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl From<http::Uri> for ApplicationSource {
    fn from(value: http::Uri) -> Self {
        Self(value)
    }
}

impl From<ApplicationSource> for http::Uri {
    fn from(value: ApplicationSource) -> Self {
        value.0
    }
}

impl fmt::Display for ApplicationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Application {
    pub id: ApplicationId,
    pub blob: BlobId,
    pub version: Option<semver::Version>,
    pub source: ApplicationSource,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Release {
    pub version: semver::Version,
    pub notes: String,
    pub path: String,
    pub hash: String,
}
