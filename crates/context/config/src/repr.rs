use core::convert::Infallible;
use core::error::Error as CoreError;
use core::fmt::{Debug, Display, Formatter};
use core::ops::Deref;
use core::result::Result as CoreResult;
use std::borrow::Cow;
use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use bs58::decode::{DecodeTarget, Error as Bs58Error, Result as Bs58Result};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T, E> = CoreResult<T, ReprError<E>>;

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct Repr<T> {
    #[serde(with = "serde_bytes", bound = "T: ReprBytes")]
    inner: T,
}

impl<T> Repr<T> {
    pub const fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: ReprBytes> Display for Repr<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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
#[non_exhaustive]
pub enum ReprError<E> {
    #[error("decode error: {0}")]
    DecodeError(E),
    #[error("invalid base58: {0}")]
    InvalidBase58(#[from] Bs58Error),
}

impl<E> ReprError<E> {
    pub fn map<F, O>(self, f: F) -> ReprError<O>
    where
        F: FnOnce(E) -> O,
    {
        match self {
            Self::DecodeError(e) => ReprError::DecodeError(f(e)),
            Self::InvalidBase58(e) => ReprError::InvalidBase58(e),
        }
    }
}

pub trait ReprBytes: Sized {
    type EncodeBytes<'a>: AsRef<[u8]>
    where
        Self: 'a;
    type DecodeBytes: DecodeTarget + AsRef<[u8]>;

    type Error: CoreError;

    fn as_bytes(&self) -> Self::EncodeBytes<'_>;

    fn from_bytes<F>(f: F) -> Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>;
}

pub trait ReprTransmute<'a>: ReprBytes + 'a {
    fn rt<O: ReprBytes<DecodeBytes = Self::EncodeBytes<'a>>>(&'a self) -> Result<O, O::Error>;
}

impl<'a, T> ReprTransmute<'a> for T
where
    T: 'a + ReprBytes<EncodeBytes<'a>: AsRef<[u8]>>,
{
    fn rt<O>(&'a self) -> Result<O, O::Error>
    where
        O: ReprBytes<DecodeBytes = T::EncodeBytes<'a>>,
    {
        O::from_bytes(|buf| {
            *buf = self.as_bytes();
            Ok(buf.as_ref().len())
        })
    }
}

impl<T: ReprBytes> ReprBytes for Repr<T> {
    type EncodeBytes<'a>
        = T::EncodeBytes<'a>
    where
        T: 'a;
    type DecodeBytes = T::DecodeBytes;

    type Error = T::Error;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.inner.as_bytes()
    }

    fn from_bytes<F>(f: F) -> Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        Ok(Self {
            inner: ReprBytes::from_bytes(f)?,
        })
    }
}

#[derive(Copy, Clone, Error)]
#[error("insufficient length, found: {found}, expected: {expected}")]
pub struct LengthMismatch {
    found: usize,
    expected: usize,
    _priv: (),
}

impl Debug for LengthMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl<const N: usize> ReprBytes for [u8; N] {
    type EncodeBytes<'a> = Self;
    type DecodeBytes = Self;

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        *self
    }

    fn from_bytes<F>(f: F) -> Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        let mut bytes = [0; N];

        let len = f(&mut bytes).map_err(ReprError::InvalidBase58)?;

        if len != N {
            return Err(ReprError::DecodeError(LengthMismatch {
                found: len,
                expected: N,
                _priv: (),
            }));
        }

        Ok(bytes)
    }
}

pub trait DynSizedByteSlice: AsRef<[u8]> + From<Vec<u8>> {}

impl DynSizedByteSlice for Vec<u8> {}
impl DynSizedByteSlice for Box<[u8]> {}
impl DynSizedByteSlice for Cow<'_, [u8]> {}

impl<T> ReprBytes for T
where
    T: DynSizedByteSlice,
{
    type EncodeBytes<'b>
        = &'b [u8]
    where
        T: 'b;
    type DecodeBytes = Vec<u8>;

    type Error = Infallible;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.as_ref()
    }

    fn from_bytes<F>(f: F) -> Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        let mut bytes = Vec::new();
        let _ = f(&mut bytes)?;
        Ok(bytes.into())
    }
}

mod serde_bytes {
    use std::borrow::Cow;

    use serde::de::{Deserializer, Error as SerdeError};
    use serde::ser::Serializer;
    use serde::Deserialize;

    use super::{ReprBytes, ReprError};

    pub fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: ReprBytes,
        S: Serializer,
    {
        let encoded = bs58::encode(value.as_bytes()).into_string();

        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: ReprBytes,
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(transparent)]
        struct Container<'a>(#[serde(borrow)] Cow<'a, str>);

        let encoded = Container::deserialize(deserializer)?;

        T::from_bytes(|bytes| bs58::decode(&*encoded.0).onto(bytes)).map_err(|e| match e {
            ReprError::DecodeError(err) => SerdeError::custom(err),
            ReprError::InvalidBase58(err) => SerdeError::custom(err),
        })
    }
}
