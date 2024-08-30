use std::{fmt, ops::Deref};

use near_sdk::{bs58, near};
use thiserror::Error;

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd)]
#[near(serializers = [borsh, json])]
#[serde(transparent)]
pub struct Repr<T> {
    #[serde(with = "serde_bytes", bound = "T: ReprBytes")]
    inner: T,
}

impl<T> Repr<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: ReprBytes> fmt::Display for Repr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&bs58::encode(self.inner.as_bytes()).into_string())
    }
}

impl<T> Deref for Repr<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug, Error)]
pub enum Error<E> {
    #[error("decode error: {0}")]
    DecodeError(E),
    #[error("invalid base58: {0}")]
    InvalidBase58(#[from] bs58::decode::Error),
}

pub trait ReprBytes: Sized {
    type EncodeBytes<'a>: AsRef<[u8]>
    where
        Self: 'a;
    type DecodeBytes: bs58::decode::DecodeTarget;
    type Error: std::error::Error;

    fn as_bytes(&self) -> Self::EncodeBytes<'_>;

    fn from_bytes<F>(f: F) -> Result<Self, Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>;
}

#[derive(Copy, Clone, Error)]
#[error("insufficient length")]
pub struct InsufficientLength {
    _priv: (),
}

impl fmt::Debug for InsufficientLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<const N: usize> ReprBytes for [u8; N] {
    type EncodeBytes<'a> = &'a Self;
    type DecodeBytes = Self;

    type Error = InsufficientLength;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self
    }

    fn from_bytes<F>(f: F) -> Result<Self, Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        let mut bytes = [0; N];

        let len = f(&mut bytes).map_err(Error::InvalidBase58)?;

        if len != N {
            return Err(Error::DecodeError(InsufficientLength { _priv: () }));
        }

        Ok(bytes)
    }
}

impl ReprBytes for Vec<u8> {
    type EncodeBytes<'b> = &'b [u8];
    type DecodeBytes = Vec<u8>;

    type Error = std::convert::Infallible;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self
    }

    fn from_bytes<F>(f: F) -> Result<Self, Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        let mut bytes = Vec::new();

        let len = f(&mut bytes)?;

        assert_eq!(len, bytes.len());

        Ok(bytes)
    }
}

mod serde_bytes {
    use near_sdk::bs58;
    use near_sdk::serde::{de, ser, Deserialize};

    use super::{Error, ReprBytes};

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
        T: ReprBytes,
    {
        let encoded = bs58::encode(value.as_bytes()).into_string();
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        D: de::Deserializer<'de>,
        T: ReprBytes,
    {
        let encoded = <&'de str>::deserialize(deserializer)?;
        T::from_bytes(|bytes| bs58::decode(encoded).onto(bytes)).map_err(|e| match e {
            Error::DecodeError(err) => de::Error::custom(err),
            Error::InvalidBase58(err) => de::Error::custom(err),
        })
    }
}
