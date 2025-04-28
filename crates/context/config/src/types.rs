use core::convert::Infallible;
use core::fmt;
use core::fmt::{Debug, Display, Formatter};
use core::marker::PhantomData;
use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use bs58::decode::Result as Bs58Result;
use ed25519_dalek::{Signature, SignatureError, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::repr::{self, LengthMismatch, Repr, ReprBytes, ReprTransmute};

pub type Revision = u64;

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Deserialize,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[non_exhaustive]
pub struct Application<'a> {
    pub id: Repr<ApplicationId>,
    pub blob: Repr<BlobId>,
    pub size: u64,
    #[serde(borrow)]
    pub source: ApplicationSource<'a>,
    pub metadata: ApplicationMetadata<'a>,
}

impl<'a> Application<'a> {
    #[must_use]
    pub const fn new(
        id: Repr<ApplicationId>,
        blob: Repr<BlobId>,
        size: u64,
        source: ApplicationSource<'a>,
        metadata: ApplicationMetadata<'a>,
    ) -> Self {
        Application {
            id,
            blob,
            size,
            source,
            metadata,
        }
    }
}

#[derive(
    Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize, Hash,
)]
pub struct Identity([u8; 32]);

impl ReprBytes for Identity {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(Self)
    }
}

impl From<[u8; 32]> for Identity {
    fn from(value: [u8; 32]) -> Self {
        Identity(value)
    }
}

#[derive(
    Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize, Hash,
)]
pub struct SignerId(Identity);

impl ReprBytes for SignerId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ContextId(Identity);

impl ReprBytes for ContextId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(
    Eq,
    Ord,
    Debug,
    Clone,
    PartialEq,
    PartialOrd,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct ContextStorageEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ContextIdentity(Identity);

impl ReprBytes for ContextIdentity {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct BlobId(Identity);

impl ReprBytes for BlobId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ApplicationId(Identity);

impl ReprBytes for ApplicationId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Default,
    Deserialize,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[expect(clippy::exhaustive_structs, reason = "Exhaustive")]
pub struct ApplicationSource<'a>(#[serde(borrow)] pub Cow<'a, str>);

impl ApplicationSource<'_> {
    #[must_use]
    pub fn to_owned(self) -> ApplicationSource<'static> {
        ApplicationSource(Cow::Owned(self.0.into_owned()))
    }
}

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Debug,
    Default,
    Deserialize,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
#[expect(clippy::exhaustive_structs, reason = "Exhaustive")]
pub struct ApplicationMetadata<'a>(#[serde(borrow)] pub Repr<Cow<'a, [u8]>>);

impl ApplicationMetadata<'_> {
    #[must_use]
    pub fn to_owned(self) -> ApplicationMetadata<'static> {
        ApplicationMetadata(Repr::new(Cow::Owned(self.0.into_inner().into_owned())))
    }
}

impl ReprBytes for Signature {
    type EncodeBytes<'a> = [u8; 64];
    type DecodeBytes = [u8; 64];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.to_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(|b| Self::from_bytes(&b))
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ProposalId(Identity);

impl ReprBytes for ProposalId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        ReprBytes::from_bytes(f).map(Self)
    }
}

#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum VerificationKeyParseError {
    #[error(transparent)]
    LengthMismatch(LengthMismatch),
    #[error("invalid key: {0}")]
    InvalidVerificationKey(SignatureError),
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
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        use VerificationKeyParseError::{InvalidVerificationKey, LengthMismatch};

        let bytes = Self::DecodeBytes::from_bytes(f).map_err(|e| e.map(LengthMismatch))?;

        Self::from_bytes(&bytes)
            .map_err(|e| repr::ReprError::DecodeError(InvalidVerificationKey(e)))
    }
}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

#[derive(Eq, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signed<T> {
    payload: Repr<Box<[u8]>>,
    signature: Repr<Signature>,

    #[serde(skip)]
    _priv: PhantomData<T>,
}

#[derive(ThisError)]
#[non_exhaustive]
pub enum ConfigError<E> {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("json error: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("derivation error: {0}")]
    DerivationError(E),
    #[error(transparent)]
    VerificationKeyParseError(#[from] repr::ReprError<VerificationKeyParseError>),
}

impl<E: Display> Debug for ConfigError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl<T: Serialize> Signed<T> {
    pub fn new<R, F>(payload: &T, sign: F) -> Result<Self, ConfigError<R::Error>>
    where
        R: IntoResult<Signature>,
        F: FnOnce(&[u8]) -> R,
    {
        let payload = serde_json::to_vec(&payload)?.into_boxed_slice();

        let signature = sign(&payload)
            .into_result()
            .map_err(ConfigError::DerivationError)?;

        Ok(Self {
            payload: Repr::new(payload),
            signature: Repr::new(signature),
            _priv: PhantomData,
        })
    }
}

pub trait IntoResult<T> {
    type Error;

    fn into_result(self) -> Result<T, Self::Error>;
}

impl<T> IntoResult<T> for T {
    type Error = Infallible;

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
    pub fn parse<R, F>(&'a self, f: F) -> Result<T, ConfigError<R::Error>>
    where
        R: IntoResult<SignerId>,
        F: FnOnce(&T) -> R,
    {
        let parsed = serde_json::from_slice(&self.payload)?;

        let bytes = f(&parsed)
            .into_result()
            .map_err(ConfigError::DerivationError)?;

        let key = bytes
            .rt::<VerifyingKey>()
            .map_err(ConfigError::VerificationKeyParseError)?;

        key.verify(&self.payload, &self.signature)
            .map_or(Err(ConfigError::InvalidSignature), |()| Ok(parsed))
    }
}
