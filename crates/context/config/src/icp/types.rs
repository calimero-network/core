use std::borrow::Cow;
use std::marker::PhantomData;

use candid::CandidType;
use ed25519_dalek::{Verifier, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use thiserror::Error as ThisError;

use super::repr::ICRepr;
use crate::repr::{Repr, ReprTransmute};
use crate::types::{
    Application, ApplicationId, ApplicationMetadata, ApplicationSource, BlobId, Capability,
    ContextId, ContextIdentity, IntoResult, SignerId,
};
use crate::{ContextRequest, ContextRequestKind, RequestKind};

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICApplication {
    pub id: ICRepr<ApplicationId>,
    pub blob: ICRepr<BlobId>,
    pub size: u64,
    pub source: String,
    pub metadata: Vec<u8>,
}

impl From<Application<'_>> for ICApplication {
    fn from(value: Application<'_>) -> Self {
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

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ICContextRequest {
    pub context_id: ICRepr<ContextId>,
    pub kind: ICContextRequestKind,
}

impl<'a> From<ContextRequest<'a>> for ICContextRequest {
    fn from(value: ContextRequest<'a>) -> Self {
        Self {
            context_id: value.context_id.rt().expect("infallible conversion"),
            kind: value.kind.into(),
        }
    }
}

#[derive(CandidType, Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
pub enum ICCapability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub enum ICContextRequestKind {
    Add {
        author_id: ICRepr<ContextIdentity>,
        application: ICApplication,
    },
    UpdateApplication {
        application: ICApplication,
    },
    AddMembers {
        members: Vec<ICRepr<ContextIdentity>>,
    },
    RemoveMembers {
        members: Vec<ICRepr<ContextIdentity>>,
    },
    Grant {
        capabilities: Vec<(ICRepr<ContextIdentity>, ICCapability)>,
    },
    Revoke {
        capabilities: Vec<(ICRepr<ContextIdentity>, ICCapability)>,
    },
    UpdateProxyContract,
}

impl From<ContextRequestKind<'_>> for ICContextRequestKind {
    fn from(value: ContextRequestKind<'_>) -> Self {
        match value {
            ContextRequestKind::Add {
                author_id,
                application,
            } => ICContextRequestKind::Add {
                author_id: author_id.rt().expect("infallible conversion"),
                application: application.into(),
            },
            ContextRequestKind::UpdateApplication { application } => {
                ICContextRequestKind::UpdateApplication {
                    application: application.into(),
                }
            }
            ContextRequestKind::AddMembers { members } => ICContextRequestKind::AddMembers {
                members: members
                    .into_owned()
                    .into_iter()
                    .map(|m| m.rt().expect("infallible conversion"))
                    .collect(),
            },
            ContextRequestKind::RemoveMembers { members } => ICContextRequestKind::RemoveMembers {
                members: members
                    .into_owned()
                    .into_iter()
                    .map(|m| m.rt().expect("infallible conversion"))
                    .collect(),
            },
            ContextRequestKind::Grant { capabilities } => ICContextRequestKind::Grant {
                capabilities: capabilities
                    .into_owned()
                    .into_iter()
                    .map(|(id, cap)| (id.rt().expect("infallible conversion"), cap.into()))
                    .collect(),
            },
            ContextRequestKind::Revoke { capabilities } => ICContextRequestKind::Revoke {
                capabilities: capabilities
                    .into_owned()
                    .into_iter()
                    .map(|(id, cap)| (id.rt().expect("infallible conversion"), cap.into()))
                    .collect(),
            },
            ContextRequestKind::UpdateProxyContract => ICContextRequestKind::UpdateProxyContract,
        }
    }
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub enum ICRequestKind {
    Context(ICContextRequest),
}

impl<'a> From<RequestKind<'a>> for ICRequestKind {
    fn from(value: RequestKind<'a>) -> Self {
        match value {
            RequestKind::Context(context) => ICRequestKind::Context(context.into()),
        }
    }
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ICRequest {
    pub kind: ICRequestKind,
    pub signer_id: ICRepr<SignerId>,
    pub nonce: u64,
}

impl ICRequest {
    pub fn new(signer_id: SignerId, kind: ICRequestKind, nonce: u64) -> Self {
        Self {
            signer_id: ICRepr::new(signer_id),
            kind,
            nonce,
        }
    }
}

#[derive(Debug, ThisError)]
pub enum ICSignedError<E> {
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
pub struct ICSigned<T> {
    payload: Vec<u8>,
    signature: Vec<u8>,
    _phantom: Phantom<T>,
}

impl<T: CandidType + DeserializeOwned> ICSigned<T> {
    pub fn new<R, F>(payload: T, sign: F) -> Result<Self, ICSignedError<R::Error>>
    where
        R: IntoResult<ed25519_dalek::Signature>,
        F: FnOnce(&[u8]) -> R,
    {
        let bytes = candid::encode_one(payload)
            .map_err(|e| ICSignedError::SerializationError(e.to_string()))?;

        let signature = sign(&bytes)
            .into_result()
            .map_err(ICSignedError::DerivationError)?;

        Ok(Self {
            payload: bytes,
            signature: signature.to_vec(),
            _phantom: Phantom(PhantomData),
        })
    }

    pub fn parse<R, F>(&self, f: F) -> Result<T, ICSignedError<R::Error>>
    where
        R: IntoResult<SignerId>,
        F: FnOnce(&T) -> R,
    {
        let parsed: T = candid::decode_one(&self.payload)
            .map_err(|e| ICSignedError::DeserializationError(e.to_string()))?;

        let signer_id = f(&parsed)
            .into_result()
            .map_err(ICSignedError::DerivationError)?;

        let key = signer_id
            .rt::<VerifyingKey>()
            .map_err(|_| ICSignedError::InvalidPublicKey)?;

        let signature = ed25519_dalek::Signature::from_slice(&self.signature)?;

        key.verify(&self.payload, &signature)
            .map_err(|_| ICSignedError::InvalidSignature)?;

        Ok(parsed)
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
