#[cfg(test)]
#[path = "tests/alias.rs"]
mod tests;

use core::cmp::Ordering as CmpOrdering;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::str::{from_utf8, from_utf8_unchecked, FromStr};
use std::fmt;

use serde::{de, ser, Deserialize, Serialize};
use thiserror::Error;

use crate::application::ApplicationId;
use crate::context::ContextId;
use crate::identity::PublicKey;

const MAX_ALIAS_LEN: usize = 50;

// Compile time assertion to ensure that the `MAX_ALIAS_LEN` fits within `u8`.
const _: () = {
    // Assert that MAX_ALIAS_LEN can fit within an 8-bit unsigned integer (0-255).
    if MAX_ALIAS_LEN > u8::MAX as usize {
        // This panic will trigger a compilation error with the provided message.
        panic!("MAX_ALIAS_LEN must be a value that fits in 8 bits.");
    }
};

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
    str: [u8; MAX_ALIAS_LEN],
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
    #[error("exceeds maximum length of {} characters", MAX_ALIAS_LEN)]
    TooLong,
}

impl<T> Alias<T> {
    #[must_use]
    pub fn as_str(&self) -> &str {
        let bytes = &self.str[..self.len as usize];
        unsafe { from_utf8_unchecked(bytes) }
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
        if s.len() > MAX_ALIAS_LEN {
            return Err(InvalidAlias::TooLong);
        }

        let mut str = [0; MAX_ALIAS_LEN];
        str[..s.len()].copy_from_slice(s.as_bytes());

        // NOTE: This conversion should never return an error because the assert above
        // ensures `s.len()` is less than `MAX_ALIAS_LEN` (50), which always fits in a u8.
        let len_u8 = u8::try_from(s.len()).map_err(|_| InvalidAlias::TooLong)?;

        Ok(Self {
            str,
            len: len_u8,
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
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.str.cmp(&other.str)
    }
}

impl<T> PartialOrd for Alias<T> {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
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
                write!(f, "an alias of at most {MAX_ALIAS_LEN} characters")
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
                let Ok(s) = from_utf8(v) else {
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
