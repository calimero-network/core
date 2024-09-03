use core::fmt;
use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::repr::{self, Repr, ReprBytes, ReprTransmute};

#[derive(
    Eq,
    Ord,
    Clone,
    Debug,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct Application<'a> {
    pub id: Repr<ApplicationId>,
    pub blob: Repr<BlobId>,
    #[serde(borrow)]
    pub source: ApplicationSource<'a>,
    pub metadata: ApplicationMetadata<'a>,
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct Identity([u8; 32]);

impl ReprBytes for Identity {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct SignerId(Identity);

impl ReprBytes for SignerId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ContextId(Identity);

impl ReprBytes for ContextId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ContextIdentity(Identity);

impl ReprBytes for ContextIdentity {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct BlobId(Identity);

impl ReprBytes for BlobId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ApplicationId(Identity);

impl ReprBytes for ApplicationId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(
    Eq,
    Ord,
    Debug,
    Default,
    Clone,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct ApplicationSource<'a>(#[serde(borrow)] pub Cow<'a, str>);

impl ApplicationSource<'_> {
    pub fn to_owned(self) -> ApplicationSource<'static> {
        ApplicationSource(Cow::Owned(self.0.into_owned()))
    }
}

#[derive(
    Eq,
    Ord,
    Debug,
    Default,
    Clone,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct ApplicationMetadata<'a>(#[serde(borrow)] pub Repr<Cow<'a, [u8]>>);

impl ApplicationMetadata<'_> {
    pub fn to_owned(self) -> ApplicationMetadata<'static> {
        ApplicationMetadata(Repr::new(Cow::Owned(self.0.into_inner().into_owned())))
    }
}

impl ReprBytes for Signature {
    type EncodeBytes<'a> = [u8; 64];
    type DecodeBytes = [u8; 64];

    type Error = repr::LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.to_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(|b| Self::from_bytes(&b))
    }
}

#[derive(Debug, Error)]
pub enum VerificationKeyParseError {
    #[error(transparent)]
    LengthMismatch(repr::LengthMismatch),
    #[error("invalid key: {0}")]
    InvalidVerificationKey(ed25519_dalek::SignatureError),
}

impl ReprBytes for VerifyingKey {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = VerificationKeyParseError;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.to_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        use VerificationKeyParseError::{InvalidVerificationKey, LengthMismatch};

        let bytes = Self::DecodeBytes::from_bytes(f).map_err(|e| e.map(LengthMismatch))?;

        Self::from_bytes(&bytes).map_err(|e| repr::Error::DecodeError(InvalidVerificationKey(e)))
    }
}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Capability {
    ManageApplication,
    ManageMembers,
}

#[derive(Eq, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signed<T> {
    payload: Repr<Box<[u8]>>,
    signature: Repr<Signature>,

    #[serde(skip)]
    _priv: std::marker::PhantomData<T>,
}

#[derive(Error)]
pub enum Error<E> {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("failed to parse JSON payload: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("failed to derive key: {0}")]
    KeyDerivationError(E),
    #[error(transparent)]
    VerificationKeyParseError(#[from] repr::Error<VerificationKeyParseError>),
}

impl<E: fmt::Display> fmt::Debug for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<T: Serialize> Signed<T> {
    pub fn new(payload: &T, sign: impl FnOnce(&[u8]) -> Signature) -> serde_json::Result<Self> {
        let payload = serde_json::to_vec(&payload)?;

        let signature = sign(&payload);

        Ok(Self {
            payload: Repr::new(payload.into_boxed_slice()),
            signature: Repr::new(signature),
            _priv: Default::default(),
        })
    }
}

pub trait IntoResult<T> {
    type Error;

    fn into_result(self) -> Result<T, Self::Error>;
}

impl<T> IntoResult<T> for T {
    type Error = std::convert::Infallible;

    fn into_result(self) -> Result<T, Self::Error> {
        Ok(self)
    }
}

impl<T, E> IntoResult<T> for Result<T, E> {
    type Error = E;

    fn into_result(self) -> Result<T, Self::Error> {
        self
    }
}

impl<'a, T: Deserialize<'a>> Signed<T> {
    pub fn parse<R: IntoResult<SignerId>>(
        &'a self,
        f: impl FnOnce(&T) -> R,
    ) -> Result<T, Error<R::Error>> {
        let parsed = serde_json::from_slice(&self.payload)?;

        let bytes = f(&parsed)
            .into_result()
            .map_err(Error::KeyDerivationError)?;

        let key = bytes
            .rt::<VerifyingKey>()
            .map_err(Error::VerificationKeyParseError)?;

        key.verify(&self.payload, &self.signature)
            .map_or(Err(Error::InvalidSignature), |_| Ok(parsed))
    }
}
