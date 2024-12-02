use std::borrow::Cow;
use std::marker::PhantomData;
use std::time;

use bs58::decode::Result as Bs58Result;
use candid::CandidType;
use ed25519_dalek::{Verifier, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::repr::{self, LengthMismatch, Repr, ReprBytes, ReprTransmute};
use crate::types::{
    Application, ApplicationMetadata, ApplicationSource, Capability, IntoResult, SignerId,
};
use crate::{ContextRequestKind, RequestKind};

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

// From implementation for SignerId
impl From<ICSignerId> for SignerId {
    fn from(value: ICSignerId) -> Self {
        value.rt().expect("infallible conversion")
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
pub struct ICContextId(Identity);

impl ICContextId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }

    pub fn as_bytes(&self) -> [u8; 32] {
        self.0.as_bytes()
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
pub struct ICApplicationId(Identity);

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
pub struct ICBlobId(Identity);

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
pub struct ICMembersRequest {
    pub context_id: ICContextId,
    pub offset: usize,
    pub length: usize,
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
pub struct ICPContextRequest {
    pub context_id: ICContextId,
    pub kind: ICPContextRequestKind,
}

#[derive(CandidType, Copy, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ICCapability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

// From implementation for Capability
impl From<ICCapability> for Capability {
    fn from(value: ICCapability) -> Self {
        match value {
            ICCapability::ManageApplication => Capability::ManageApplication,
            ICCapability::ManageMembers => Capability::ManageMembers,
            ICCapability::Proxy => Capability::Proxy,
        }
    }
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub enum ICPContextRequestKind {
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
pub enum ICPRequestKind {
    Context(ICPContextRequest),
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub struct ICPRequest {
    pub kind: ICPRequestKind,
    pub signer_id: ICSignerId,
    pub timestamp_ms: u64,
}

impl ICPRequest {
    pub fn new(signer_id: ICSignerId, kind: ICPRequestKind) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "This is never expected to overflow"
        )]
        let timestamp_ms = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis() as u64;

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
    #[error("serialization error: {0}")]
    SerializationError(String),
    #[error("deserialization error: {0}")]
    DeserializationError(String),
}

#[derive(Deserialize, Debug, Clone)]
struct Phantom<T>(#[serde(skip)] PhantomData<T>);

impl<T> CandidType for Phantom<T> {
    fn _ty() -> candid::types::Type {
        candid::types::TypeInner::Null.into()
    }

    fn idl_serialize<S>(&self, serializer: S) -> Result<(), S::Error>
    where
        S: candid::types::Serializer,
    {
        serializer.serialize_null(())
    }
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ICPSigned<T> {
    payload: Vec<u8>,
    signature: Vec<u8>,
    _phantom: Phantom<T>,
}

impl<T: CandidType + Serialize + DeserializeOwned> ICPSigned<T> {
    pub fn new<R, F>(payload: T, sign: F) -> Result<Self, ICPSignedError<R::Error>>
    where
        R: IntoResult<ed25519_dalek::Signature>,
        F: FnOnce(&[u8]) -> R,
    {
        let bytes = candid::encode_one(payload)
            .map_err(|e| ICPSignedError::SerializationError(e.to_string()))?;

        let signature = sign(&bytes)
            .into_result()
            .map_err(ICPSignedError::DerivationError)?;

        Ok(Self {
            payload: bytes,
            signature: signature.to_vec(),
            _phantom: Phantom(PhantomData),
        })
    }

    pub fn parse<R, F>(&self, f: F) -> Result<T, ICPSignedError<R::Error>>
    where
        R: IntoResult<ICSignerId>,
        F: FnOnce(&T) -> R,
    {
        let parsed: T = candid::decode_one(&self.payload)
            .map_err(|e| ICPSignedError::DeserializationError(e.to_string()))?;

        let signer_id = f(&parsed)
            .into_result()
            .map_err(ICPSignedError::DerivationError)?;

        let key = signer_id
            .rt::<VerifyingKey>()
            .map_err(|_| ICPSignedError::InvalidPublicKey)?;

        let signature_bytes: [u8; 64] =
            self.signature.as_slice().try_into().map_err(|_| {
                ICPSignedError::SignatureError(ed25519_dalek::ed25519::Error::new())
            })?;
        let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes);

        key.verify(&self.payload, &signature)
            .map_err(|_| ICPSignedError::InvalidSignature)?;

        Ok(parsed)
    }
}

impl From<&Capability> for ICCapability {
    fn from(value: &Capability) -> Self {
        match value {
            Capability::ManageApplication => ICCapability::ManageApplication,
            Capability::ManageMembers => ICCapability::ManageMembers,
            Capability::Proxy => ICCapability::Proxy,
        }
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

impl From<RequestKind<'_>> for ICPRequestKind {
    fn from(value: RequestKind<'_>) -> Self {
        match value {
            RequestKind::Context(context_request) => ICPRequestKind::Context(ICPContextRequest {
                context_id: context_request
                    .context_id
                    .rt()
                    .expect("infallible conversion"),
                kind: match context_request.kind {
                    ContextRequestKind::Add {
                        author_id,
                        application,
                    } => ICPContextRequestKind::Add {
                        author_id: author_id.rt().expect("infallible conversion"),
                        application: application.into(),
                    },
                    ContextRequestKind::UpdateApplication { application } => {
                        ICPContextRequestKind::UpdateApplication {
                            application: application.into(),
                        }
                    }
                    ContextRequestKind::AddMembers { members } => {
                        ICPContextRequestKind::AddMembers {
                            members: members
                                .into_iter()
                                .map(|m| m.rt().expect("infallible conversion"))
                                .collect(),
                        }
                    }
                    ContextRequestKind::RemoveMembers { members } => {
                        ICPContextRequestKind::RemoveMembers {
                            members: members
                                .into_iter()
                                .map(|m| m.rt().expect("infallible conversion"))
                                .collect(),
                        }
                    }
                    ContextRequestKind::Grant { capabilities } => ICPContextRequestKind::Grant {
                        capabilities: capabilities
                            .into_iter()
                            .map(|(id, cap)| (id.rt().expect("infallible conversion"), cap.into()))
                            .collect(),
                    },
                    ContextRequestKind::Revoke { capabilities } => ICPContextRequestKind::Revoke {
                        capabilities: capabilities
                            .into_iter()
                            .map(|(id, cap)| (id.rt().expect("infallible conversion"), cap.into()))
                            .collect(),
                    },
                    ContextRequestKind::UpdateProxyContract => {
                        ICPContextRequestKind::UpdateProxyContract
                    }
                },
            }),
        }
    }
}
