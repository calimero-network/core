use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub struct Alias(String);

#[derive(Clone, Copy, Debug, Error)]
#[error("Invalid alias: {0}")]
pub struct InvalidAlias(&'static str);

impl Alias {
    const MAX_LENGTH: usize = 50;

    #[must_use]
    pub fn new(s: String) -> Option<Self> {
        if s.len() <= Self::MAX_LENGTH {
            Some(Self(s))
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Alias {
    type Err = InvalidAlias;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_owned()).ok_or(InvalidAlias(
            "alias exceeds maximum length of 50 characters",
        ))
    }
}

impl fmt::Display for Alias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<Alias> for String {
    fn from(alias: Alias) -> Self {
        alias.0
    }
}

impl From<&Alias> for String {
    fn from(alias: &Alias) -> Self {
        alias.0.to_owned()
    }
}
