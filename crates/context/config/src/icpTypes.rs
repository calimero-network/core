use std::borrow::Cow;
use std::collections::HashMap;

use candid::CandidType;
use ed25519_dalek::{Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::guard::Guard;
use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::{
    Application, ApplicationId, ApplicationMetadata, ApplicationSource, BlobId, Capability,
    ContextId, ContextIdentity, SignerId,
};

#[derive(
    CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Hash,
)]
pub struct Identity(pub [u8; 32]);

#[derive(
    CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy,
)]
pub struct ICSignerId(pub Identity);

impl ICSignerId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

pub type ICContextIdentity = ICSignerId;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ICContextId(pub Identity);

impl ICContextId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICApplicationId(pub Identity);

impl ICApplicationId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICHasMemberRequest {
    pub context_id: ICContextId,
    pub identity: ICContextIdentity,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICMembersRequest {
    pub context_id: ICContextId,
    pub offset: usize,
    pub length: usize,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICPrivilegesRequest {
    pub context_id: ICContextId,
    pub identities: ICContextIdentity,
}

#[derive(CandidType, Debug)]
pub struct ICMutate {
    pub signing_key: [u8; 32],
    pub nonce: u64,
    pub kind: RequestKind,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ICBlobId(pub Identity);

impl ICBlobId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
}

// Update the From implementations
impl From<SignerId> for ICSignerId {
    fn from(value: SignerId) -> Self {
        ICSignerId(Identity(value.as_bytes()))
    }
}
impl From<ICSignerId> for ContextIdentity {
    fn from(sign_id: ICSignerId) -> Self {
        sign_id.0 .0.rt().expect("Infallible conversion")
    }
}

impl From<Repr<SignerId>> for ICSignerId {
    fn from(value: Repr<SignerId>) -> Self {
        ICSignerId(Identity(value.as_bytes()))
    }
}

impl From<ICSignerId> for SignerId {
    fn from(value: ICSignerId) -> Self {
        value.0 .0.rt().expect("Infallible conversion")
    }
}

// Similar From implementations for other types
impl From<ContextId> for ICContextId {
    fn from(value: ContextId) -> Self {
        ICContextId(Identity(value.as_bytes()))
    }
}

impl From<Repr<ContextId>> for ICContextId {
    fn from(value: Repr<ContextId>) -> Self {
        ICContextId(Identity(value.as_bytes()))
    }
}

impl From<ICContextId> for ContextId {
    fn from(value: ICContextId) -> Self {
        value.0 .0.rt().expect("Infallible conversion")
    }
}

impl From<Repr<ContextIdentity>> for ICContextIdentity {
    fn from(value: Repr<ContextIdentity>) -> Self {
        ICSignerId(Identity(value.as_bytes()))
    }
}

impl From<&[Repr<ContextIdentity>]> for ICContextIdentity {
    fn from(value: &[Repr<ContextIdentity>]) -> Self {
        let mut bytes = [0; 32];
        bytes.copy_from_slice(&value[0].as_bytes());
        ICSignerId(Identity(bytes))
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

// Conversions for ApplicationId
impl From<ApplicationId> for ICApplicationId {
    fn from(value: ApplicationId) -> Self {
        ICApplicationId(Identity(value.as_bytes()))
    }
}

impl From<Repr<ApplicationId>> for ICApplicationId {
    fn from(value: Repr<ApplicationId>) -> Self {
        ICApplicationId(Identity(value.as_bytes()))
    }
}

impl From<ICApplicationId> for ApplicationId {
    fn from(value: ICApplicationId) -> Self {
        value.0 .0.rt().expect("Infallible conversion")
    }
}

// Conversions for BlobId
impl From<BlobId> for ICBlobId {
    fn from(value: BlobId) -> Self {
        ICBlobId(Identity(value.as_bytes()))
    }
}

impl From<Repr<BlobId>> for ICBlobId {
    fn from(value: Repr<BlobId>) -> Self {
        ICBlobId(Identity(value.as_bytes()))
    }
}

impl From<ICBlobId> for BlobId {
    fn from(value: ICBlobId) -> Self {
        value.0 .0.rt().expect("Infallible conversion")
    }
}

// Conversions for Application
impl From<Application<'_>> for ICApplication {
    fn from(value: Application) -> Self {
        ICApplication {
            id: ICApplicationId(Identity(value.id.as_bytes())),
            blob: ICBlobId(Identity(value.blob.as_bytes())),
            size: value.size,
            source: value.source.0.into_owned(),
            metadata: value.metadata.0.into_inner().into_owned().to_vec(),
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
            VerifyingKey::from_bytes(&signer_id.0 .0).map_err(|_| "invalid public key")?;

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

// Add these conversions for ICCapability
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
