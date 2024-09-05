use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T, E> = std::result::Result<T, Error<E>>;

#[derive(
    Eq,
    Ord,
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
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

impl<E> Error<E> {
    pub fn map<F, O>(self, f: F) -> Error<O>
    where
        F: FnOnce(E) -> O,
    {
        match self {
            Error::DecodeError(e) => Error::DecodeError(f(e)),
            Error::InvalidBase58(e) => Error::InvalidBase58(e),
        }
    }
}

pub trait ReprBytes: Sized {
    type EncodeBytes<'a>: AsRef<[u8]>
    where
        Self: 'a;
    type DecodeBytes: bs58::decode::DecodeTarget;
    type Error: std::error::Error;

    fn as_bytes(&self) -> Self::EncodeBytes<'_>;

    fn from_bytes<F>(f: F) -> Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>;
}

pub trait ReprTransmute<'a>: ReprBytes + 'a {
    fn rt<O: ReprBytes<DecodeBytes = Self::EncodeBytes<'a>>>(&'a self) -> Result<O, O::Error>;
}

impl<'a, T: 'a> ReprTransmute<'a> for T
where
    T: ReprBytes<EncodeBytes<'a>: AsRef<[u8]>>,
{
    fn rt<O: ReprBytes<DecodeBytes = Self::EncodeBytes<'a>>>(&'a self) -> Result<O, O::Error> {
        O::from_bytes(|buf| {
            *buf = self.as_bytes();
            Ok(buf.as_ref().len())
        })
    }
}

impl<T: ReprBytes> ReprBytes for Repr<T> {
    type EncodeBytes<'a> = T::EncodeBytes<'a> where T: 'a;
    type DecodeBytes = T::DecodeBytes;

    type Error = T::Error;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.inner.as_bytes()
    }

    fn from_bytes<F>(f: F) -> Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Ok(Repr {
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

impl fmt::Debug for LengthMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
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
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        let mut bytes = [0; N];

        let len = f(&mut bytes).map_err(Error::InvalidBase58)?;

        if len != N {
            return Err(Error::DecodeError(LengthMismatch {
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
    type EncodeBytes<'b> = &'b [u8] where T: 'b;
    type DecodeBytes = Vec<u8>;

    type Error = std::convert::Infallible;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.as_ref()
    }

    fn from_bytes<F>(f: F) -> Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        let mut bytes = Vec::new();

        let len = f(&mut bytes)?;

        assert_eq!(len, bytes.len());

        Ok(bytes.into())
    }
}

mod serde_bytes {
    use std::borrow::Cow;

    use serde::{de, ser, Deserialize};

    use super::{Error, ReprBytes};

    pub fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: ReprBytes,
        S: ser::Serializer,
    {
        let encoded = bs58::encode(value.as_bytes()).into_string();

        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: ReprBytes,
        D: de::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Container<'a>(#[serde(borrow)] Cow<'a, str>);

        let encoded = Container::deserialize(deserializer)?;

        T::from_bytes(|bytes| bs58::decode(&*encoded.0).onto(bytes)).map_err(|e| match e {
            Error::DecodeError(err) => de::Error::custom(err),
            Error::InvalidBase58(err) => de::Error::custom(err),
        })
    }
}
