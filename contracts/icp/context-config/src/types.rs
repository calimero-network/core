use std::borrow::Cow;
use std::collections::HashMap;

use calimero_context_config::repr::{Repr, ReprBytes, ReprTransmute};
use calimero_context_config::types::{
    Application, ApplicationId, ApplicationMetadata, ApplicationSource, BlobId, ContextId,
    IntoResult, SignerId,
};
use candid::CandidType;
use ed25519_dalek::{Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::guard::Guard;

#[derive(
    CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy,
)]
pub struct ICSignerId(pub [u8; 32]);

impl ICSignerId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

// Make ICContextIdentity a type alias for ICSignerId, just like NEAR does
pub type ICContextIdentity = ICSignerId;

// From original type to ICP type
impl From<SignerId> for ICSignerId {
    fn from(value: SignerId) -> Self {
        ICSignerId(value.as_bytes())
    }
}

// From Repr to ICP type
impl From<Repr<SignerId>> for ICSignerId {
    fn from(value: Repr<SignerId>) -> Self {
        ICSignerId(value.as_bytes())
    }
}

// From ICP type back to original
impl From<ICSignerId> for SignerId {
    fn from(value: ICSignerId) -> Self {
        value.0.rt().expect("Infallible conversion")
    }
}
impl IntoResult<SignerId> for ICSignerId {
    type Error = &'static str;

    fn into_result(self) -> Result<SignerId, Self::Error> {
        Ok(self.into())
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ICContextId(pub [u8; 32]);

impl ICContextId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

// Implement conversions like we did for SignerId
impl From<ContextId> for ICContextId {
    fn from(value: ContextId) -> Self {
        ICContextId(value.as_bytes())
    }
}

impl From<Repr<ContextId>> for ICContextId {
    fn from(value: Repr<ContextId>) -> Self {
        ICContextId(value.as_bytes())
    }
}

impl From<ICContextId> for ContextId {
    fn from(value: ICContextId) -> Self {
        value.0.rt().expect("Infallible conversion")
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICApplicationId(pub [u8; 32]);

impl ICApplicationId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICBlobId(pub [u8; 32]);

impl ICBlobId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICApplication {
    pub id: ICApplicationId,
    pub blob: ICBlobId,
    pub size: u64,
    pub source: String,
    pub metadata: Vec<u8>,
}

// Conversions for ApplicationId
impl From<ApplicationId> for ICApplicationId {
    fn from(value: ApplicationId) -> Self {
        ICApplicationId(value.as_bytes())
    }
}

impl From<Repr<ApplicationId>> for ICApplicationId {
    fn from(value: Repr<ApplicationId>) -> Self {
        ICApplicationId(value.as_bytes())
    }
}

impl From<ICApplicationId> for ApplicationId {
    fn from(value: ICApplicationId) -> Self {
        value.0.rt().expect("Infallible conversion")
    }
}

// Conversions for BlobId
impl From<BlobId> for ICBlobId {
    fn from(value: BlobId) -> Self {
        ICBlobId(value.as_bytes())
    }
}

impl From<Repr<BlobId>> for ICBlobId {
    fn from(value: Repr<BlobId>) -> Self {
        ICBlobId(value.as_bytes())
    }
}

impl From<ICBlobId> for BlobId {
    fn from(value: ICBlobId) -> Self {
        value.0.rt().expect("Infallible conversion")
    }
}

// Conversions for Application
impl From<Application<'_>> for ICApplication {
    fn from(value: Application) -> Self {
        ICApplication {
            id: (*value.id).into(),
            blob: (*value.blob).into(),
            size: value.size,
            source: value.source.0.into_owned(),
            metadata: value.metadata.0.into_inner().into_owned(),
        }
    }
}

impl<'a> From<ICApplication> for Application<'a> {
    fn from(value: ICApplication) -> Self {
        Application::new(
            Repr::new(value.id.into()),
            Repr::new(value.blob.into()),
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

    #[cfg(not(test))]
    pub fn new_with_time(signer_id: ICSignerId, kind: RequestKind) -> Self {
        Self {
            signer_id,
            kind,
            timestamp_ms: ic_cdk::api::time(),
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

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ICPSigned<T: CandidType + Serialize> {
    pub payload: T,
    pub signature: Vec<u8>,
}

impl<T: CandidType + Serialize> ICPSigned<T> {
    pub fn parse<F>(&self, f: F) -> Result<&T, &'static str>
    where
        F: FnOnce(&T) -> &ICSignerId,
    {
        // Get the signer's public key from the payload
        let signer_id = f(&self.payload);

        // Convert signer_id to VerifyingKey (public key)
        let verifying_key =
            VerifyingKey::from_bytes(&signer_id.0).map_err(|_| "invalid public key")?;

        // Serialize the payload for verification
        let message =
            candid::encode_one(&self.payload).map_err(|_| "failed to serialize payload")?;

        // Convert signature bytes to ed25519::Signature
        let signature = ed25519_dalek::Signature::from_slice(&self.signature)
            .map_err(|_| "invalid signature format")?;

        // Verify the signature
        verifying_key
            .verify(&message, &signature)
            .map_err(|_| "invalid signature")?;

        Ok(&self.payload)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct Context {
    pub application: Guard<ICApplication>,
    pub members: Guard<Vec<ICContextIdentity>>,
    pub proxy: Guard<String>,
}

pub struct ContextConfigs {
    pub contexts: HashMap<ICContextId, Context>,
    pub next_proxy_id: u64,
}

impl Default for ContextConfigs {
    fn default() -> Self {
        Self {
            contexts: HashMap::new(),
            next_proxy_id: 0,
        }
    }
}
