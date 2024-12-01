use candid::CandidType;
use proxy_contract::types::{ICContextId, ICContextIdentity, ICSignerId};
use serde::{Deserialize, Serialize};

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub struct Request {
    pub kind: RequestKind,
    pub signer_id: ICSignerId,
    pub timestamp_ms: u64,
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub enum RequestKind {
    Context(ContextRequest),
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub struct ContextRequest {
    pub context_id: ICContextId,
    pub kind: ContextRequestKind,
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub enum ContextRequestKind {
    Add {
        author_id: ICContextIdentity,
        application: ICApplication,
    },
    AddMembers {
        members: Vec<ICContextIdentity>,
    },
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone)]
pub struct ICApplication {
    pub id: ICApplicationId,
    pub blob: ICBlobId,
    pub size: u64,
    pub source: String,
    pub metadata: Vec<u8>,
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ICApplicationId(pub [u8; 32]);

#[derive(CandidType, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ICBlobId(pub [u8; 32]);

impl ICApplicationId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl ICBlobId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}
