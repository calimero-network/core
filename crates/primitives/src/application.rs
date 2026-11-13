#[cfg(test)]
#[path = "tests/application.rs"]
mod tests;

use core::fmt::{self, Display, Formatter};
use core::ops::Deref;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use url::{ParseError, Url};

use crate::hash::{Hash, HashError};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, Ord, PartialOrd)]
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

impl AsRef<[u8; 32]> for ApplicationId {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for ApplicationId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ApplicationId {
    /// Sentinel value: no application / uninitialized application ID (all-zero hash).
    #[must_use]
    pub const fn zero() -> Self {
        Self(Hash::zero())
    }

    /// Bundle ids are version-stable: the package and signing key together
    /// decide which app a bundle IS, so every later version reuses the pair.
    #[cfg(feature = "borsh")]
    pub fn for_bundle(package: &str, signer_id: &str) -> eyre::Result<Self> {
        Ok(Self::from(*Hash::hash_borsh(&(package, signer_id))?))
    }
}

impl Display for ApplicationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<ApplicationId> for String {
    fn from(id: ApplicationId) -> Self {
        id.0.to_string()
    }
}

impl From<&ApplicationId> for String {
    fn from(id: &ApplicationId) -> Self {
        id.0.to_string()
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
