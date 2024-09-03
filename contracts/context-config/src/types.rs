use core::fmt;
use std::borrow::Cow;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use near_sdk::serde::Deserialize;
use near_sdk::{bs58, near, serde, serde_json};
use thiserror::Error;

use crate::repr::{self, Repr, ReprBytes};

#[derive(Debug)]
#[near(serializers = [borsh, json])]
pub struct Application<'a> {
    pub id: Repr<ApplicationId>,
    pub blob: Repr<BlobId>,
    #[serde(borrow)]
    pub source: ApplicationSource<'a>,
    pub metadata: ApplicationMetadata<'a>,
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
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

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
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

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
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

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
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

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
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

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
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

#[derive(Eq, Ord, Debug, Default, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh, json])]
pub struct ApplicationSource<'a>(#[serde(borrow)] pub Cow<'a, str>);

impl ApplicationSource<'_> {
    pub fn to_owned(self) -> ApplicationSource<'static> {
        ApplicationSource(Cow::Owned(self.0.into_owned()))
    }
}

#[derive(Eq, Ord, Debug, Default, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh, json])]
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

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd)]
#[near(serializers = [json])]
pub enum Capability {
    ManageApplication,
    ManageMembers,
}

#[derive(Eq, Debug, Clone, PartialEq)]
#[near(serializers = [json])]
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
    #[error("failed to derive verifing key: {0}")]
    KeyDerivationError(E),
}

impl<E: fmt::Display> fmt::Debug for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<T: serde::Serialize> Signed<T> {
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
impl<'a, T: Deserialize<'a>> Signed<T> {
    pub fn parse<E>(
        &'a self,
        f: impl FnOnce(&T) -> Result<VerifyingKey, E>,
    ) -> Result<T, Error<E>> {
        let parsed = serde_json::from_slice(&self.payload)?;
        let key = f(&parsed).map_err(Error::KeyDerivationError)?;
        key.verify(&self.payload, &self.signature)
            .map_or(Err(Error::InvalidSignature), |_| Ok(parsed))
    }
}
