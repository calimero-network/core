use std::borrow::Cow;

use bs58::decode::Result as Bs58Result;
use calimero_context_config::repr::{self, LengthMismatch, Repr, ReprBytes, ReprTransmute};
use calimero_context_config::types::{
    Application, ApplicationMetadata, ApplicationSource, Capability, IntoResult,
};
use candid::CandidType;
use ed25519_dalek::{Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

#[derive(
    CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Hash,
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

#[derive(
    CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy,
)]
pub struct ICSignerId(Identity);

impl ICSignerId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

impl ReprBytes for ICSignerId {
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
        Identity::from_bytes(f).map(Self)
    }
}

#[derive(
    CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy,
)]
pub struct ICContextIdentity(Identity);

impl ICContextIdentity {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

impl ReprBytes for ICContextIdentity {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];
    type Error = LengthMismatch;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.0 .0
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        Identity::from_bytes(f).map(Self)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ICContextId(pub Identity);

impl ICContextId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

impl ReprBytes for ICContextId {
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
        Identity::from_bytes(f).map(Self)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICApplicationId(pub Identity);

impl ICApplicationId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

impl ReprBytes for ICApplicationId {
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
        Identity::from_bytes(f).map(Self)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICBlobId(pub Identity);

impl ICBlobId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

impl ReprBytes for ICBlobId {
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
        Identity::from_bytes(f).map(Self)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICApplication {
    pub id: ICApplicationId,
    pub blob: ICBlobId,
    pub size: u64,
    pub source: String,
    pub metadata: Vec<u8>,
}

impl From<Application<'_>> for ICApplication {
    fn from(value: Application) -> Self {
        ICApplication {
            id: value.id.rt().expect("infallible conversion"),
            blob: value.blob.rt().expect("infallible conversion"),
            size: value.size,
            source: value.source.0.into_owned(),
            metadata: value.metadata.0.into_inner().into_owned(),
        }
    }
}

impl<'a> From<ICApplication> for Application<'a> {
    fn from(value: ICApplication) -> Self {
        Application::new(
            value.id.rt().expect("infallible conversion"),
            value.blob.rt().expect("infallible conversion"),
            value.size,
            ApplicationSource(Cow::Owned(value.source)),
            ApplicationMetadata(Repr::new(Cow::Owned(value.metadata))),
        )
    }
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub struct ContextRequest {
    pub context_id: ICContextId,
    pub kind: ContextRequestKind,
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ICCapability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub enum ContextRequestKind {
    Add {
        author_id: ICContextIdentity,
        application: ICApplication,
    },
    UpdateApplication {
        application: ICApplication,
    },
    AddMembers {
        members: Vec<ICContextIdentity>,
    },
    RemoveMembers {
        members: Vec<ICContextIdentity>,
    },
    Grant {
        capabilities: Vec<(ICContextIdentity, ICCapability)>,
    },
    Revoke {
        capabilities: Vec<(ICContextIdentity, ICCapability)>,
    },
    UpdateProxyContract,
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub enum RequestKind {
    Context(ContextRequest),
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub struct Request {
    pub kind: RequestKind,
    pub signer_id: ICSignerId,
    pub timestamp_ms: u64,
}

impl Request {
    pub fn new(signer_id: ICSignerId, kind: RequestKind) -> Self {
        Self {
            signer_id,
            kind,
            timestamp_ms: 0, // Default timestamp for tests
        }
    }

    #[cfg(test)]
    pub fn new_with_time(signer_id: ICSignerId, kind: RequestKind, timestamp_ms: u64) -> Self {
        Self {
            signer_id,
            kind,
            timestamp_ms,
        }
    }
}

#[derive(Debug, ThisError)]
pub enum ICPSignedError<E> {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("json error: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("derivation error: {0}")]
    DerivationError(E),
    #[error("invalid public key")]
    InvalidPublicKey,
    #[error("signature error: {0}")]
    SignatureError(#[from] ed25519_dalek::ed25519::Error),
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ICPSigned<T> {
    pub payload: T,
    pub signature: Vec<u8>,
}

impl<T: CandidType + Serialize + Clone> ICPSigned<T> {
    pub fn new<E, R, F>(payload: T, sign: F) -> Result<Self, ICPSignedError<E>>
    where
        R: IntoResult<Vec<u8>, Error = E>,
        F: FnOnce(&[u8]) -> R,
    {
        let bytes = serde_json::to_vec(&payload)?;

        let signature = sign(&bytes)
            .into_result()
            .map_err(ICPSignedError::DerivationError)?;

        Ok(Self { payload, signature })
    }

    pub fn parse<R, F>(&self, f: F) -> Result<T, ICPSignedError<R::Error>>
    where
        R: IntoResult<ICSignerId>,
        F: FnOnce(&T) -> R,
    {
        let bytes = serde_json::to_vec(&self.payload)?;

        let signer_id = f(&self.payload)
            .into_result()
            .map_err(ICPSignedError::DerivationError)?;

        let key = signer_id
            .rt::<VerifyingKey>()
            .map_err(|_| ICPSignedError::InvalidPublicKey)?;

        let signature = ed25519_dalek::Signature::from_slice(&self.signature)?;
        key.verify(&bytes, &signature)
            .map_err(|_| ICPSignedError::InvalidSignature)?;

        Ok(self.payload.clone())
    }
}

impl From<Capability> for ICCapability {
    fn from(value: Capability) -> Self {
        match value {
            Capability::ManageApplication => ICCapability::ManageApplication,
            Capability::ManageMembers => ICCapability::ManageMembers,
            Capability::Proxy => ICCapability::Proxy,
        }
    }
}

impl From<ICCapability> for Capability {
    fn from(value: ICCapability) -> Self {
        match value {
            ICCapability::ManageApplication => Capability::ManageApplication,
            ICCapability::ManageMembers => Capability::ManageMembers,
            ICCapability::Proxy => Capability::Proxy,
        }
    }
}
