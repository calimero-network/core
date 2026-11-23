// We get this warning as we allow to reimport self crate (`calimero_primitives`) inside the tests with a `borsh` feature.
#![cfg_attr(test, allow(unused_extern_crates))]

use core::fmt::{self, Display};
use core::ops::Deref;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::common::DIGEST_SIZE;
use crate::hash::{Hash, HashError};

/// A unique identifier for a Context, derived from a cryptographic hash.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, PartialOrd, Ord)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
// todo! define macros that construct newtypes
// todo! wrapping Hash<N> with this interface
pub struct ContextId(Hash);

impl From<[u8; DIGEST_SIZE]> for ContextId {
    fn from(id: [u8; DIGEST_SIZE]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; DIGEST_SIZE]> for ContextId {
    fn as_ref(&self) -> &[u8; DIGEST_SIZE] {
        &self.0
    }
}

impl Deref for ContextId {
    type Target = [u8; DIGEST_SIZE];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContextId {
    /// The all-zero id, which some callers use to mean "no context".
    #[must_use]
    pub fn zero() -> Self {
        Self::from([0_u8; DIGEST_SIZE])
    }

    // Returns ContextID represented as a 32-byte array.
    pub fn digest(&self) -> &[u8; DIGEST_SIZE] {
        &self.0
    }
}

impl fmt::Display for ContextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<ContextId> for String {
    fn from(id: ContextId) -> Self {
        id.0.to_string()
    }
}

impl From<&ContextId> for String {
    fn from(id: &ContextId) -> Self {
        id.0.to_string()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[error(transparent)]
pub struct InvalidContextId(HashError);

impl FromStr for ContextId {
    type Err = InvalidContextId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidContextId)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    #[test]
    fn test_context_id_invalid_hex() {
        // "Invalid!" contains characters hex does not allow, so it fails on the
        // alphabet rather than the length.
        let invalid_encoded_context_id = "Invalid!";
        let result = ContextId::from_str(invalid_encoded_context_id);
        assert!(matches!(
            result,
            Err(InvalidContextId(HashError::InvalidHex))
        ));
    }
}
