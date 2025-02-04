#[cfg(test)]
#[path = "tests/alias.rs"]
mod tests;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Alias(String);

#[derive(Clone, Copy, Debug, Error)]
#[error("Invalid alias: {0}")]
pub struct InvalidAlias(&'static str);

impl Alias {
    const MAX_LENGTH: usize = 50;

    #[must_use]
    pub fn new(s: String) -> Option<Self> {
        (s.len() <= Self::MAX_LENGTH).then(|| Self(s))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Alias {
    type Err = InvalidAlias;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_owned()
            .try_into()
            .map_err(|_| InvalidAlias("alias exceeds maximum length of 50 characters"))
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

impl TryFrom<String> for Alias {
    type Error = InvalidAlias;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidAlias(
            "alias exceeds maximum length of 50 characters",
        ))
    }
}

impl<'de> Deserialize<'de> for Alias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        encoded.parse().map_err(|_| {
            serde::de::Error::custom(format!(
                "alias exceeds maximum length of {} characters",
                Self::MAX_LENGTH
            ))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Identity,
    Application,
}
