#[cfg(test)]
#[path = "tests/alias.rs"]
mod tests;

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;

use serde::{de, ser, Deserialize, Serialize};
use thiserror::Error;

use crate::application::ApplicationId;
use crate::context::ContextId;
use crate::identity::PublicKey;

const MAX_LENGTH: usize = 50;
const _: [(); { (usize::BITS - MAX_LENGTH.leading_zeros()) > 8 } as usize] = [
    /* MAX_LENGTH must be a 8-bit number */
];

pub trait ScopedAlias {
    type Scope;
}

impl ScopedAlias for ContextId {
    type Scope = ();
}

impl ScopedAlias for PublicKey {
    type Scope = ContextId;
}

impl ScopedAlias for ApplicationId {
    type Scope = ();
}

pub struct Alias<T> {
    str: [u8; MAX_LENGTH],
    len: u8,
    _pd: PhantomData<T>,
}

impl<T> Copy for Alias<T> {}
impl<T> Clone for Alias<T> {
    fn clone(&self) -> Self {
        *self
    }
}

#[derive(Copy, Clone, Debug, Error)]
#[error("invalid alias: {}")]
pub enum InvalidAlias {
    #[error("exceeds maximum length of {} characters", MAX_LENGTH)]
    TooLong,
}

impl<T> Alias<T> {
    #[must_use]
    pub fn as_str(&self) -> &str {
        let bytes = &self.str[..self.len as usize];
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
}

impl<T> AsRef<str> for Alias<T> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<T> FromStr for Alias<T> {
    type Err = InvalidAlias;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() > MAX_LENGTH {
            return Err(InvalidAlias::TooLong);
        }

        let mut str = [0; MAX_LENGTH];
        str[..s.len()].copy_from_slice(s.as_bytes());

        Ok(Self {
            str,
            // safety: we guarantee this is 8-bit, where MAX_LENGTH is defined
            len: s.len() as u8,
            _pd: PhantomData,
        })
    }
}

impl<T> fmt::Display for Alias<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl<T> fmt::Debug for Alias<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Alias").field(&self.as_str()).finish()
    }
}

impl<T> Eq for Alias<T> {}

impl<T> PartialEq for Alias<T> {
    fn eq(&self, other: &Self) -> bool {
        self.str == other.str
    }
}

impl<T> Ord for Alias<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.str.cmp(&other.str)
    }
}

impl<T> PartialOrd for Alias<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Serialize for Alias<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de, T> Deserialize<'de> for Alias<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct AliasVisitor<T>(PhantomData<T>);

        impl<T> de::Visitor<'_> for AliasVisitor<T> {
            type Value = Alias<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "an alias of at most {} characters", MAX_LENGTH)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Alias::from_str(v).map_err(de::Error::custom)
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let Ok(s) = std::str::from_utf8(v) else {
                    return Err(de::Error::invalid_value(de::Unexpected::Bytes(v), &self));
                };

                Alias::from_str(s).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(AliasVisitor(PhantomData))
    }
}

impl<T> Hash for Alias<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}
